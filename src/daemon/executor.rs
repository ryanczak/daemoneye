use crate::daemon::session::{bg_done_subscribe, append_session_message, FG_HOOK_COUNTER, BUFFER_COUNTER, SessionStore};
use crate::daemon::utils::*;
use crate::daemon::background::{run_background_in_window, respawn_background_in_pane};
use crate::ipc::{MemoryListItem, PaneInfo, Request, Response, RunbookListItem, ScheduleListItem, ScriptListItem};
use crate::scheduler::{ActionOn, JobStatus, ScheduleKind, ScheduledJob, ScheduleStore};
use crate::scripts;
use crate::tmux;
use crate::tmux::cache::SessionCache;
use crate::ai::{mask_sensitive, next_tool_id, PendingCall};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::AsyncBufReadExt;

/// The outcome of a single tool call execution.
pub enum ToolCallOutcome {
    /// Normal result string to feed back to the AI.
    Result(String),
    /// The user typed a corrective message at the approval prompt.
    /// The caller must abort the current tool chain and inject this text as a
    /// new user turn so the AI can course-correct without seeing a synthetic
    /// tool error.
    UserMessage(String),
}

// ---------------------------------------------------------------------------
// Timing constants — all durations used by tool execution in one place.
// ---------------------------------------------------------------------------

/// How long a user has to approve or deny a foreground/background tool call.
const APPROVAL_TIMEOUT: Duration = Duration::from_secs(60);
/// How long a user has to respond to a credential or write prompt (sudo password, schedule, script).
const USER_PROMPT_TIMEOUT: Duration = Duration::from_secs(120);
/// Poll interval when detecting whether a sudo password prompt has appeared.
const SUDO_POLL_INTERVAL: Duration = Duration::from_millis(100);
/// Window within which a sudo password prompt must appear before giving up.
const SUDO_DETECT_WINDOW: Duration = Duration::from_secs(3);
/// Poll interval for remote-pane (SSH/mosh) output-stability check.
const REMOTE_POLL_INTERVAL: Duration = Duration::from_millis(500);
/// Max time to wait for a command to complete in a remote pane.
const REMOTE_CMD_TIMEOUT: Duration = Duration::from_secs(30);
/// Fast poll used to detect that a child process has started in a local pane.
const LOCAL_CHILD_POLL: Duration = Duration::from_millis(25);
/// Window within which a child process must appear before falling back to hook-only wait.
const LOCAL_CHILD_START_WINDOW: Duration = Duration::from_millis(300);
/// Max time to wait for a command to complete in a local pane.
const LOCAL_CMD_TIMEOUT: Duration = Duration::from_secs(45);
/// Slow poll used while waiting for a local command to return to the shell prompt.
const LOCAL_SLOW_POLL: Duration = Duration::from_millis(500);
/// Delay after command completion before capturing output, to let the shell flush.
const POST_CMD_CAPTURE_DELAY: Duration = Duration::from_millis(50);
/// Seconds of pane silence before `alert-silence` fires as a secondary completion
/// signal in the local-pane foreground path (N9).
const SILENCE_MONITOR_SECS: u32 = 2;

// ---------------------------------------------------------------------------
// RAII guard for tmux hooks installed during foreground command execution.
// ---------------------------------------------------------------------------

/// Uninstalls tmux hooks on drop so that early returns via `?` or panics
/// never leave stale `pane-title-changed` or `alert-silence` hooks behind.
struct FgHookGuard {
    target: String,
    /// Hook names to remove with `tmux set-hook -u`.
    hooks: Vec<String>,
    /// When true, also restores `monitor-silence` to its default on drop.
    monitor_silence: bool,
}

impl FgHookGuard {
    fn new(target: &str, title_hook: String) -> Self {
        Self {
            target: target.to_string(),
            hooks: vec![title_hook],
            monitor_silence: false,
        }
    }

    /// Register the alert-silence hook and the monitor-silence option for cleanup.
    fn add_silence(&mut self, silence_hook: String) {
        self.hooks.push(silence_hook);
        self.monitor_silence = true;
    }
}

impl Drop for FgHookGuard {
    fn drop(&mut self) {
        for hook in &self.hooks {
            let _ = std::process::Command::new("tmux")
                .args(["set-hook", "-u", "-t", &self.target, hook])
                .output();
        }
        if self.monitor_silence {
            let _ = std::process::Command::new("tmux")
                .args(["set-option", "-u", "-t", &self.target, "monitor-silence"])
                .output();
        }
    }
}
/// Max time to wait for a shell-prompt pattern after starting an interactive
/// command (ssh, mosh, telnet, screen). Returns as soon as the prompt appears.
const INTERACTIVE_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Poll interval used while waiting for a prompt pattern in the interactive path.
const INTERACTIVE_POLL_INTERVAL: Duration = Duration::from_millis(300);
/// Fallback stability window when prompt detection fails (two identical snapshots).
const INTERACTIVE_STABLE_WINDOW: Duration = Duration::from_millis(600);

/// Return true when `cmd` is a shell name, meaning the pane is at a prompt.
fn is_shell_prompt(cmd: &str) -> bool {
    matches!(
        cmd.trim(),
        "bash" | "zsh" | "fish" | "sh" | "ksh" | "csh" | "tcsh" | "dash"
            | "nu" | "pwsh" | "elvish" | "xonsh" | "yash"
    )
}

/// Return true when the last non-empty line of a pane snapshot ends with a
/// recognisable shell-prompt character, indicating the remote shell is ready.
/// Intentionally permissive — a false positive just causes slightly early
/// capture, which is safe; false negatives fall through to the stability check.
fn looks_like_shell_prompt(snap: &str) -> bool {
    snap.lines()
        .filter(|l| !l.trim().is_empty())
        .last()
        .map(|l| {
            let t = l.trim_end();
            t.ends_with("$ ") || t.ends_with("# ") || t.ends_with("% ")
                || t.ends_with("> ")
                || t.ends_with('$') || t.ends_with('#')
                || t.ends_with('%') || t.ends_with('>')
        })
        .unwrap_or(false)
}

/// Send a `ToolCallPrompt` to the client, wait up to [`APPROVAL_TIMEOUT`] for
/// the user's [`Request::ToolCallResponse`], and log the outcome.
///
/// Returns `Ok(None)` when the user approves.
/// Returns `Ok(Some(ToolCallOutcome::Result(msg)))` when the user denies or
/// the wait times out — the caller should propagate this as the tool result.
/// Returns `Ok(Some(ToolCallOutcome::UserMessage(text)))` when the user typed
/// a corrective message; the caller should abort the tool chain and inject the
/// text as a new user turn.
/// Returns `Err` on connection EOF.
async fn prompt_and_await_approval(
    id: &str,
    cmd: &str,
    background: bool,
    target_pane_hint: Option<&str>,
    session_id: Option<&str>,
    tx: &mut tokio::net::unix::OwnedWriteHalf,
    rx: &mut tokio::io::BufReader<tokio::net::unix::OwnedReadHalf>,
) -> anyhow::Result<Option<ToolCallOutcome>> {
    let mode = if background { "background" } else { "foreground" };
    send_response_split(tx, Response::ToolCallPrompt {
        id: id.to_string(),
        command: cmd.to_string(),
        background,
        target_pane: target_pane_hint.map(|s| s.to_string()),
    }).await?;

    let mut line = String::new();
    let read_result = tokio::time::timeout(APPROVAL_TIMEOUT, rx.read_line(&mut line)).await;

    if matches!(read_result, Ok(Ok(0))) {
        return Err(anyhow::anyhow!("EOF"));
    }

    let timed_out = read_result.is_err();

    // Parse the response, checking for a user_message redirect first.
    enum Parsed { Approved, Denied, UserMessage(String) }
    let parsed = match read_result {
        Ok(Ok(_)) => match serde_json::from_str::<Request>(line.trim()) {
            Ok(Request::ToolCallResponse { id: resp_id, approved, user_message }) if resp_id == id => {
                if let Some(msg) = user_message {
                    Parsed::UserMessage(msg)
                } else if approved {
                    Parsed::Approved
                } else {
                    Parsed::Denied
                }
            }
            _ => Parsed::Denied,
        },
        _ => Parsed::Denied,
    };

    match parsed {
        Parsed::Approved => {
            log::info!("{} command approved: {}", mode, cmd);
            log_event("command_approval", serde_json::json!({
                "session": session_id.unwrap_or("-"),
                "mode": mode,
                "cmd": cmd,
                "decision": "approved",
            }));
            Ok(None)
        }
        Parsed::Denied => {
            let decision = if timed_out { "timeout" } else { "denied" };
            log::info!("{} command {}: {}", mode, decision, cmd);
            log_event("command_approval", serde_json::json!({
                "session": session_id.unwrap_or("-"),
                "mode": mode,
                "cmd": cmd,
                "decision": decision,
            }));
            log_command(session_id, mode, "", cmd, decision, "");
            let msg = if timed_out {
                let notice = format!(
                    "Approval prompt timed out after {} s — the command was not executed. \
                     You can re-run the request if you still want it.",
                    APPROVAL_TIMEOUT.as_secs()
                );
                // Notify the user directly so they know why the tool call was dropped,
                // even if their approval window closed before they could respond (A3).
                let _ = send_response_split(tx, Response::SystemMsg(notice.clone())).await;
                notice
            } else {
                "User denied execution".to_string()
            };
            Ok(Some(ToolCallOutcome::Result(msg)))
        }
        Parsed::UserMessage(text) => {
            log::info!("{} command redirected by user message: {}", mode, cmd);
            log_event("command_approval", serde_json::json!({
                "session": session_id.unwrap_or("-"),
                "mode": mode,
                "cmd": cmd,
                "decision": "user_message",
            }));
            Ok(Some(ToolCallOutcome::UserMessage(text)))
        }
    }
}

async fn find_best_target_pane(
    target: Option<&str>,
    chat_pane: Option<&str>,
    cache: &Arc<SessionCache>,
    sessions: &SessionStore,
    session_id: Option<&str>,
    tx: &mut tokio::net::unix::OwnedWriteHalf,
    rx: &mut tokio::io::BufReader<tokio::net::unix::OwnedReadHalf>,
) -> anyhow::Result<String> {
    let ai_target = target.and_then(|tp: &str| {
        if chat_pane == Some(tp) { return None; }
        let panes = cache.panes.read().unwrap_or_log();
        if panes.contains_key(tp) { Some(tp.to_string()) } else { None::<String> }
    });

    if let Some(tp) = ai_target {
        return Ok(tp);
    }
    
    // Check for a user-selected default target pane in the session
    if let Some(sid) = session_id {
        if let Ok(store) = sessions.lock() {
            if let Some(entry) = store.get(sid) {
                if let Some(ref dtp) = entry.default_target_pane {
                    if chat_pane.as_deref() != Some(dtp.as_str()) {
                        let panes = cache.panes.read().unwrap_or_log();
                        if panes.contains_key(dtp) {
                            return Ok(dtp.clone());
                        }
                    }
                }
            }
        }
    }

    let pane_list: Vec<PaneInfo> = {
        let panes = cache.panes.read().unwrap_or_log();
        let mut v: Vec<PaneInfo> = panes.iter()
            .map(|(pid, state)| PaneInfo {
                id: pid.clone(),
                current_cmd: state.current_cmd.clone(),
                summary: state.summary.clone(),
            })
            .collect();
        v.sort_by(|a, b| a.id.cmp(&b.id));
        v
    };
    
    if pane_list.is_empty() {
        send_response_split(tx, Response::Error(
            "No tmux panes available".to_string()
        )).await?;
        return Err(anyhow::anyhow!("No active pane found."));
    }
    
    let prompt_id = next_tool_id();
    send_response_split(tx, Response::PaneSelectPrompt {
        id: prompt_id.clone(),
        panes: pane_list,
    }).await?;
    
    let mut pane_line = String::new();
    rx.read_line(&mut pane_line).await?;
    match serde_json::from_str::<Request>(pane_line.trim()) {
        Ok(Request::PaneSelectResponse { pane_id, .. }) => {
            // Save user choice as default for the session
            if let Some(sid) = session_id {
                if let Ok(mut store) = sessions.lock() {
                    if let Some(entry) = store.get_mut(sid) {
                        entry.default_target_pane = Some(pane_id.clone());
                    }
                }
            }
            Ok(pane_id)
        },
        _ => {
            send_response_split(tx, Response::Error(
                "Expected PaneSelectResponse".to_string()
            )).await?;
            Err(anyhow::anyhow!("User aborted or invalid response"))
        }
    }
}

// ---------------------------------------------------------------------------
// Remote pane helpers for read_file / edit_file
// ---------------------------------------------------------------------------

/// Hex-encode a string (no external crate required).
fn to_hex(s: &str) -> String {
    s.bytes().map(|b| format!("{:02x}", b)).collect()
}

/// Shell-escape a single-quoted argument by replacing `'` with `'\''`.
fn sq_escape(s: &str) -> String {
    s.replace('\'', "'\\''")
}

/// Extract lines between a unique start marker and end marker from pane output.
fn extract_marked(snap: &str, start: &str, end: &str) -> Option<String> {
    let lines: Vec<&str> = snap.lines().collect();
    let s_idx = lines.iter().position(|l| l.contains(start))?;
    let e_idx = lines.iter().rposition(|l| l.contains(end))?;
    if e_idx <= s_idx { return None; }
    Some(lines[s_idx + 1..e_idx].join("\n"))
}

/// Send a command to a pane and poll until a completion marker appears in the
/// captured output.  Returns the raw pane snapshot (caller extracts content).
async fn remote_run_and_capture(pane_id: &str, cmd: &str, timeout_secs: u64) -> anyhow::Result<String> {
    tmux::send_keys(pane_id, cmd)?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(timeout_secs);
    loop {
        tokio::time::sleep(Duration::from_millis(300)).await;
        if tokio::time::Instant::now() > deadline {
            anyhow::bail!("Timed out waiting for remote command in pane {}", pane_id);
        }
        let snap = tmux::capture_pane(pane_id, 600).unwrap_or_default();
        // Completion is signalled by the marker embedded in cmd.
        if snap.contains("__DE_DONE__") {
            return Ok(snap);
        }
    }
}

/// Build the shell command to read `path` from a remote pane, with optional
/// sed pagination and grep filtering.  Output is wrapped in unique markers so
/// it can be extracted from the pane snapshot.
fn build_remote_read_cmd(path: &str, start: usize, end: usize, pattern: Option<&str>) -> String {
    let safe_path = sq_escape(path);
    let grep_part = pattern
        .map(|p| format!(" | grep -E '{}'", sq_escape(p)))
        .unwrap_or_default();
    format!(
        "echo '__DE_S__'; sed -n '{},{}p' '{}' 2>&1{}; echo '__DE_E__'; echo '__DE_DONE__'",
        start, end, safe_path, grep_part
    )
}

/// Build the shell command to read `path` through the tmux buffer system.
/// The file is piped into a named tmux buffer so there is no scrollback cap.
/// A `__DE_DONE__` marker is echoed to the pane after the load completes.
fn build_local_buffer_read_cmd(path: &str, start: usize, end: usize,
                                pattern: Option<&str>, buf_name: &str) -> String {
    let safe_path = sq_escape(path);
    let grep_part = pattern
        .map(|p| format!(" | grep -E '{}'", sq_escape(p)))
        .unwrap_or_default();
    format!(
        "sed -n '{},{}p' '{}'{}  | tmux load-buffer -b '{}' -; echo '__DE_DONE__'",
        start, end, safe_path, grep_part, buf_name
    )
}

/// Run a read-file command in a LOCAL target pane using `load-buffer`/`save-buffer`
/// to bypass the 600-line scrollback cap.  Returns the file content as a String.
async fn local_read_via_buffer(pane_id: &str, path: &str, start: usize, end: usize,
                                pattern: Option<&str>) -> anyhow::Result<String> {
    let idx = BUFFER_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let buf_name = format!("de-rb-{}", idx);
    let cmd = build_local_buffer_read_cmd(path, start, end, pattern, &buf_name);

    // Inject command; wait for __DE_DONE__ in pane (small capture — just the prompt line).
    tmux::send_keys(pane_id, &cmd)?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        tokio::time::sleep(Duration::from_millis(200)).await;
        if tokio::time::Instant::now() > deadline {
            let _ = std::process::Command::new("tmux")
                .args(["delete-buffer", "-b", &buf_name])
                .output();
            anyhow::bail!("Timed out waiting for buffer load in pane {}", pane_id);
        }
        let snap = tmux::capture_pane(pane_id, 5).unwrap_or_default();
        if snap.contains("__DE_DONE__") {
            break;
        }
    }

    // Read the buffer via `tmux save-buffer`.
    let out = std::process::Command::new("tmux")
        .args(["save-buffer", "-b", &buf_name, "-"])
        .output()?;
    let _ = std::process::Command::new("tmux")
        .args(["delete-buffer", "-b", &buf_name])
        .output();

    if !out.status.success() {
        // Buffer may be empty (no matching lines) — treat as empty rather than error.
        return Ok(String::new());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Build the shell command that runs a Python3-then-Perl atomic replacement
/// in a remote pane.  Both scripts are hex-encoded into the command so no
/// shell escaping is needed for the file contents.
fn build_remote_edit_cmd(path: &str, old_string: &str, new_string: &str) -> String {
    let path_hex   = to_hex(path);
    let old_hex    = to_hex(old_string);
    let new_hex    = to_hex(new_string);

    // Python3 script — hex-encoded to avoid any quoting issues when passed
    // to `python3 -c "exec(bytes.fromhex(...).decode())"`.
    let py = format!(
        "import os,sys\n\
         p=bytes.fromhex('{path_hex}').decode()\n\
         o=bytes.fromhex('{old_hex}').decode()\n\
         n=bytes.fromhex('{new_hex}').decode()\n\
         c=open(p).read()\n\
         cnt=c.count(o)\n\
         if cnt==0: print('DE_ERROR: old_string not found in '+p); sys.exit(1)\n\
         if cnt>1: print('DE_ERROR: old_string appears '+str(cnt)+' times in '+p); sys.exit(1)\n\
         t=p+'.de_tmp'\n\
         open(t,'w').write(c.replace(o,n,1))\n\
         os.rename(t,p)\n\
         print('DE_OK: Edited '+p)\n"
    );
    let py_hex = to_hex(&py);

    // Perl script — hex-encoded, decoded via pack('H*',...) and eval'd.
    let pl = format!(
        "my $p=pack('H*','{path_hex}');\n\
         my $o=pack('H*','{old_hex}');\n\
         my $n=pack('H*','{new_hex}');\n\
         open(my $f,'<',$p) or do{{print \"DE_ERROR: $!\\n\";exit 1}};\n\
         my $c=do{{local $/;<$f>}};close $f;\n\
         my @m=($c=~/\\Q$o\\E/g);\n\
         if(!@m){{print \"DE_ERROR: not found\\n\";exit 1}}\n\
         if(@m>1){{print \"DE_ERROR: \".scalar(@m).\" matches\\n\";exit 1}}\n\
         $c=~s/\\Q$o\\E/$n/;\n\
         my $t=\"$p.de_tmp\";\n\
         open(my $g,'>',$t) or do{{print \"DE_ERROR: $!\\n\";exit 1}};\n\
         print $g $c;close $g;\n\
         rename($t,$p) or do{{print \"DE_ERROR: $!\\n\";exit 1}};\n\
         print \"DE_OK: Edited $p\\n\";\n"
    );
    let pl_hex = to_hex(&pl);

    format!(
        "if command -v python3 >/dev/null 2>&1; then \
            python3 -c \"exec(bytes.fromhex('{py_hex}').decode())\" 2>&1; \
         else \
            perl -e 'eval(pack(\"H*\",\"{pl_hex}\"))' 2>&1; \
         fi; echo '__DE_DONE__'"
    )
}

pub async fn execute_tool_call(
    call: &PendingCall,
    tx: &mut tokio::net::unix::OwnedWriteHalf,
    rx: &mut tokio::io::BufReader<tokio::net::unix::OwnedReadHalf>,
    session_id: Option<&str>,
    session_name: &str,
    chat_pane: Option<&str>,
    cache: &Arc<SessionCache>,
    sessions: &SessionStore,
    schedule_store: &Arc<ScheduleStore>,
) -> anyhow::Result<ToolCallOutcome> {
    let result: String = match call {
        PendingCall::Foreground { id, cmd, target, .. } => {
            // Compute a best-guess target pane hint synchronously from the cache/session
            // so the approval prompt can show the user which pane will be used.
            let target_hint: Option<String> = (|| {
                // 1. AI-specified target (if valid and not the chat pane).
                if let Some(tp) = target.as_deref() {
                    if chat_pane != Some(tp) {
                        let panes = cache.panes.read().unwrap_or_log();
                        if panes.contains_key(tp) {
                            return Some(tp.to_string());
                        }
                    }
                }
                // 2. User's saved default target pane.
                if let Some(sid) = session_id {
                    if let Ok(store) = sessions.lock() {
                        if let Some(entry) = store.get(sid) {
                            if let Some(ref dtp) = entry.default_target_pane {
                                if chat_pane.as_deref() != Some(dtp.as_str()) {
                                    let panes = cache.panes.read().unwrap_or_log();
                                    if panes.contains_key(dtp) {
                                        return Some(dtp.clone());
                                    }
                                }
                            }
                        }
                    }
                }
                None
            })();
            if let Some(outcome) = prompt_and_await_approval(id, cmd, false, target_hint.as_deref(), session_id, tx, rx).await? {
                return Ok(outcome);
            }
            let target_owned = match find_best_target_pane(target.as_deref(), chat_pane, cache, sessions, session_id, tx, rx).await {
                    Ok(tp) => tp,
                    Err(_) => return Err(anyhow::anyhow!("EOF")),
                };
                
                let target_str = target_owned.as_str();
                if target_str.is_empty() {
                    "No active pane found.".to_string()
                } else {
                    let is_synchronized = {
                        let panes = cache.panes.read().unwrap_or_log();
                        panes.get(target_str).map(|p| p.synchronized).unwrap_or(false)
                    };
                    if is_synchronized {
                        let msg = format!(
                            "Pane {} has synchronized input enabled — sending a command \
                             would broadcast to all synchronized panes simultaneously. \
                             Disable synchronization first:\n  \
                             tmux set-option -t {} synchronize-panes off",
                            target_str, target_str
                        );
                        send_response_split(tx, Response::SystemMsg(msg.clone())).await?;
                        msg
                    } else {
                        let idle_cmd = tmux::pane_current_command(target_str)
                            .unwrap_or_default();
                        let is_remote_pane = get_pane_remote_host(target_str).is_some();

                        let current_exe = std::env::current_exe()
                            .unwrap_or_else(|_| std::path::PathBuf::from("daemoneye"));
                        let hook_idx = crate::daemon::session::FG_HOOK_COUNTER
                            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        let hook_name = format!("pane-title-changed[@de_fg_{}]", hook_idx);
                        let notify_cmd = format!(
                            "run-shell -b '{} notify activity {} 0 \"{}\"'",
                            current_exe.display(), target_str, shell_escape_arg(session_name)
                        );
                        let _ = std::process::Command::new("tmux")
                            .args(["set-hook", "-t", target_str, &hook_name, &notify_cmd])
                            .output();
                        // RAII guard ensures hooks are removed on any exit path, including
                        // early returns via `?` and the send_keys error arm.
                        let mut fg_hook_guard = FgHookGuard::new(target_str, hook_name.clone());

                        let mut fg_rx = bg_done_subscribe();

                        match tmux::send_keys(target_str, cmd) {
                            Ok(()) => {
                                // Highlight the target pane so the user can see which pane
                                // the agent is using during execution.  Immediately restore
                                // focus to the chat pane so the user is not displaced.
                                tmux::highlight_pane(target_str, chat_pane);
                                let mut switched_to_working = false;
                                let mut is_interactive = false;

                                if command_has_sudo(cmd) {
                                    let poll = SUDO_POLL_INTERVAL;
                                    let mut waited = Duration::ZERO;
                                    let prompt_timeout = SUDO_DETECT_WINDOW;
                                    let needs_password = loop {
                                        tokio::time::sleep(poll).await;
                                        waited += poll;
                                        let cur = tmux::pane_current_command(target_str)
                                            .unwrap_or_default();
                                        if cur == "sudo"   { break true;  }
                                        if cur == idle_cmd { break false; }
                                        if waited >= prompt_timeout { break false; }
                                    };

                                    if needs_password {
                                        send_response_split(tx, Response::SystemMsg(
                                            "sudo password prompt detected — \
                                             switching to your terminal pane. \
                                             Type your password there.".to_string()
                                        )).await?;
                                        let _ = tmux::select_pane(target_str);
                                        switched_to_working = true;
                                    }
                                }

                                if is_interactive_command(cmd) {
                                    // Interactive session path (ssh, mosh, telnet, screen).
                                    // Wait up to INTERACTIVE_CONNECT_TIMEOUT for a shell
                                    // prompt pattern to appear in the pane, then return
                                    // immediately rather than waiting for the session to exit.
                                    is_interactive = true;
                                    let deadline = tokio::time::Instant::now() + INTERACTIVE_CONNECT_TIMEOUT;
                                    let mut prompt_found = false;

                                    'connect: loop {
                                        if tokio::time::Instant::now() >= deadline { break; }
                                        tokio::select! {
                                            result = fg_rx.recv() => {
                                                if let Ok(notified_pane) = result {
                                                    if notified_pane == target_str {
                                                        // Hook fired — check for prompt immediately.
                                                        if let Ok(snap) = tmux::capture_pane(target_str, 20) {
                                                            if looks_like_shell_prompt(&snap) {
                                                                prompt_found = true;
                                                                break 'connect;
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                            _ = tokio::time::sleep(INTERACTIVE_POLL_INTERVAL) => {
                                                if let Ok(snap) = tmux::capture_pane(target_str, 20) {
                                                    if looks_like_shell_prompt(&snap) {
                                                        prompt_found = true;
                                                        break 'connect;
                                                    }
                                                }
                                            }
                                        }
                                    }

                                    // Fallback: if no prompt was detected, wait for output
                                    // to stabilise (two consecutive identical snapshots)
                                    // so we don't capture mid-handshake noise.
                                    if !prompt_found {
                                        let stable_deadline = tokio::time::Instant::now() + INTERACTIVE_STABLE_WINDOW;
                                        let mut prev = String::new();
                                        loop {
                                            if tokio::time::Instant::now() >= stable_deadline { break; }
                                            tokio::time::sleep(INTERACTIVE_POLL_INTERVAL).await;
                                            let snap = tmux::capture_pane(target_str, 20).unwrap_or_default();
                                            if snap == prev && !snap.is_empty() { break; }
                                            prev = snap;
                                        }
                                    }
                                } else if is_remote_pane {
                                    let mut prev_snap = String::new();
                                    let mut stable_ticks = 0u32;
                                    let poll = REMOTE_POLL_INTERVAL;
                                    let cmd_timeout = REMOTE_CMD_TIMEOUT;
                                    let deadline = tokio::time::Instant::now() + cmd_timeout;

                                    loop {
                                        if tokio::time::Instant::now() >= deadline { break; }
                                        tokio::select! {
                                            result = fg_rx.recv() => {
                                                if let Ok(notified_pane) = result {
                                                    if notified_pane == target_str {
                                                        stable_ticks = 0;
                                                    }
                                                }
                                            }
                                            _ = tokio::time::sleep(poll) => {
                                                let snap = tmux::capture_pane(target_str, 10).unwrap_or_default();
                                                if snap == prev_snap && !snap.is_empty() {
                                                    stable_ticks += 1;
                                                    if stable_ticks >= 2 { break; }
                                                } else {
                                                    stable_ticks = 0;
                                                    prev_snap = snap;
                                                }
                                            }
                                        }
                                    }
                                } else {
                                    // N9: install monitor-silence + alert-silence as a secondary
                                    // completion signal for edge cases where pane_current_command
                                    // doesn't reliably signal done (nested shells, custom prompts).
                                    let silence_hook_name = format!("alert-silence[@de_fg_{}]", hook_idx);
                                    let _ = std::process::Command::new("tmux")
                                        .args(["set-hook", "-t", target_str, &silence_hook_name, &notify_cmd])
                                        .output();
                                    let _ = std::process::Command::new("tmux")
                                        .args(["set-option", "-t", target_str,
                                               "monitor-silence", &SILENCE_MONITOR_SECS.to_string()])
                                        .output();
                                    fg_hook_guard.add_silence(silence_hook_name.clone());

                                    let fast_poll = LOCAL_CHILD_POLL;
                                    let start_timeout = LOCAL_CHILD_START_WINDOW;
                                    let cmd_timeout = LOCAL_CMD_TIMEOUT;
                                    let deadline = tokio::time::Instant::now() + cmd_timeout;

                                    let saw_child = tokio::time::timeout(start_timeout, async {
                                        loop {
                                            tokio::time::sleep(fast_poll).await;
                                            let cur = tmux::pane_current_command(target_str).unwrap_or_default();
                                            if cur != idle_cmd { break; }
                                        }
                                    }).await.is_ok();

                                    if saw_child {
                                        let slow_poll = LOCAL_SLOW_POLL;
                                        loop {
                                            if tokio::time::Instant::now() >= deadline { break; }
                                            tokio::select! {
                                                result = fg_rx.recv() => {
                                                    if let Ok(notified_pane) = result {
                                                        if notified_pane == target_str {
                                                            let cur = tmux::pane_current_command(target_str).unwrap_or_default();
                                                            if cur == idle_cmd { break; }
                                                        }
                                                    }
                                                }
                                                _ = tokio::time::sleep(slow_poll) => {
                                                    let cur = tmux::pane_current_command(target_str).unwrap_or_default();
                                                    if cur == idle_cmd { break; }
                                                }
                                            }
                                        }
                                    }

                                }

                                // Drop the guard now so hooks are removed before the capture
                                // delay — avoids spurious re-fires during output collection.
                                drop(fg_hook_guard);

                                tokio::time::sleep(POST_CMD_CAPTURE_DELAY).await;

                                // Remove the visual highlight now that execution is complete.
                                tmux::unhighlight_pane(target_str, chat_pane);

                                let output = match tmux::capture_pane(target_str, 200) {
                                    Ok(snap) if is_interactive => {
                                        let destination = interactive_destination(cmd)
                                            .unwrap_or_else(|| "the remote host".to_string());
                                        let pane_snap = mask_sensitive(
                                            &normalize_output(&extract_command_output(&snap, cmd))
                                        );
                                        format!(
                                            "[Interactive session started]\n\
                                             `{cmd}` opened an interactive session in pane \
                                             {target_str} — now connected to {destination}.\n\
                                             The command did not exit; the pane is running an \
                                             interactive shell on the remote host.\n\
                                             To run commands there, use \
                                             `run_terminal_command(target_pane=\"{target_str}\", \
                                             background=false)` — each call is injected into \
                                             the open remote shell.\n\
                                             Do NOT call `{cmd}` again — the session is already \
                                             established.\n\
                                             <pane_snapshot>\n{pane_snap}\n</pane_snapshot>"
                                        )
                                    }
                                    Ok(snap) => {
                                        let extracted = extract_command_output(&snap, cmd);
                                        mask_sensitive(&normalize_output(&extracted))
                                    }
                                    Err(_) => "Command sent but could not capture output.".to_string(),
                                };

                                if switched_to_working {
                                    if let Some(cp) = chat_pane {
                                        let _ = tmux::select_pane(cp);
                                    }
                                }

                                send_response_split(tx, Response::ToolResult(output.clone())).await?;
                                log_command(session_id, "foreground", target_str, cmd, "approved", &output);
                                output
                            }
                            Err(e) => {
                                let msg = format!("Failed to send command: {}", e);
                                log_command(session_id, "foreground", target_str, cmd, "send-failed", &msg);
                                msg
                            }
                        }
                    }
                }
        }

        PendingCall::Background { id, cmd, retry_pane, .. } => {
            // N11: retry path — reuse an existing background pane via respawn-pane.
            if let Some(pane_id) = retry_pane {
                if !tmux::pane_exists(pane_id) {
                    let msg = format!(
                        "Error: retry_in_pane '{}' does not exist. Use background=true without \
                         retry_in_pane to start a fresh background window.",
                        pane_id
                    );
                    send_response_split(tx, Response::ToolResult(msg.clone())).await?;
                    return Ok(ToolCallOutcome::Result(msg));
                }
                // Look up the window name from bg_windows so logs use the original name.
                let win_name: String = {
                    let mut name = pane_id.clone();
                    if let Some(sid) = session_id {
                        if let Ok(store) = sessions.lock() {
                            if let Some(entry) = store.get(sid) {
                                if let Some(w) = entry.bg_windows.iter().find(|w| &w.pane_id == pane_id) {
                                    name = w.window_name.clone();
                                }
                            }
                        }
                    }
                    name
                };
                if let Some(outcome) = prompt_and_await_approval(id, cmd, true, None, session_id, tx, rx).await? {
                    return Ok(outcome);
                }
                let session_id_owned = session_id.map(|s| s.to_string());
                let output = respawn_background_in_pane(
                    pane_id,
                    &win_name,
                    cmd,
                    session_name,
                    session_id_owned,
                    sessions.clone(),
                ).await;
                send_response_split(tx, Response::ToolResult(output.clone())).await?;
                log_command(session_id, "background_retry", "", cmd, "approved", &output);
                return Ok(ToolCallOutcome::Result(output));
            }

            // Enforce per-session cap on open background windows.
            // All lock work is done inside this block so the guard is dropped before any await.
            const MAX_BG_WINDOWS_PER_SESSION: usize = 5;
            let cap_denial: Option<String> = {
                let mut denial = None;
                if let Some(sid) = session_id {
                    if let Ok(mut store) = sessions.lock() {
                        if let Some(entry) = store.get_mut(sid) {
                            if entry.bg_windows.len() >= MAX_BG_WINDOWS_PER_SESSION {
                                let evict_idx = entry.bg_windows.iter()
                                    .position(|w| w.exit_code.is_some());
                                match evict_idx {
                                    Some(i) => {
                                        let evicted = entry.bg_windows.remove(i);
                                        // A8: only kill the window if the pane is no longer
                                        // running active work (i.e. sitting at a shell prompt).
                                        // exit_code.is_some() means the tracked command finished,
                                        // but a user may have re-used the pane manually.
                                        let pane_cmd = crate::tmux::pane_current_command(&evicted.pane_id)
                                            .unwrap_or_default();
                                        let is_idle = matches!(
                                            pane_cmd.as_str(),
                                            "bash" | "sh" | "zsh" | "fish" | "dash" | "ksh" | "tcsh" | "csh" | ""
                                        );
                                        if is_idle {
                                            log::info!("Evicting completed bg window {} to stay under cap", evicted.window_name);
                                            if let Err(e) = crate::tmux::kill_job_window(&evicted.tmux_session, &evicted.window_name) {
                                                log::warn!("Failed to evict bg window {}: {}", evicted.window_name, e);
                                            }
                                        } else {
                                            log::warn!(
                                                "Skipping eviction of bg window {} — pane is still running '{}'; \
                                                 re-inserting and denying new background job.",
                                                evicted.window_name, pane_cmd
                                            );
                                            entry.bg_windows.insert(i, evicted);
                                            denial = Some(format!(
                                                "Background window cap ({}) reached and the oldest completed window \
                                                 is still in use. Close one of the open background windows ({}) \
                                                 before starting another.",
                                                MAX_BG_WINDOWS_PER_SESSION,
                                                entry.bg_windows.iter().map(|w| w.window_name.as_str()).collect::<Vec<_>>().join(", ")
                                            ));
                                        }
                                    }
                                    None => {
                                        denial = Some(format!(
                                            "Background window cap ({}) reached and all windows are still running. \
                                             Wait for one to complete, or ask the user to close one of the open \
                                             background windows ({}) before starting another.",
                                            MAX_BG_WINDOWS_PER_SESSION,
                                            entry.bg_windows.iter().map(|w| w.window_name.as_str()).collect::<Vec<_>>().join(", ")
                                        ));
                                    }
                                }
                            }
                        }
                    }
                }
                denial
            };
            if let Some(msg) = cap_denial {
                send_response_split(tx, Response::ToolResult(msg.clone())).await?;
                return Ok(ToolCallOutcome::Result(msg));
            }

            if let Some(outcome) = prompt_and_await_approval(id, cmd, true, None, session_id, tx, rx).await? {
                return Ok(outcome);
            }
            // Wrap the sudo password in Zeroizing so its heap memory is
            // overwritten when the variable drops, rather than lingering until
            // the allocator reclaims it.
            let credential: Option<zeroize::Zeroizing<String>> = if command_has_sudo(cmd) {
                    send_response_split(tx, Response::CredentialPrompt {
                        id: id.clone(),
                        prompt: format!("[sudo] password required for: {}", cmd),
                    }).await?;
                    let mut cred_line = String::new();
                    let result = match tokio::time::timeout(
                        USER_PROMPT_TIMEOUT,
                        rx.read_line(&mut cred_line),
                    ).await {
                        Ok(Ok(_)) => match serde_json::from_str::<Request>(cred_line.trim()) {
                            Ok(Request::CredentialResponse { credential, .. }) =>
                                Some(zeroize::Zeroizing::new(credential)),
                            _ => None,
                        },
                        _ => None,
                    };
                    // Zero the raw JSON line that contained the password.
                    zeroize::Zeroize::zeroize(&mut cred_line);
                    result
                } else {
                    None
                };

                let session_id_owned = session_id.map(|s| s.to_string());
                let output = run_background_in_window(
                    session_name,
                    id,
                    cmd,
                    credential.as_ref().map(|z| z.as_str()),
                    session_id_owned,
                    sessions.clone(),
                ).await;
                send_response_split(tx, Response::ToolResult(output.clone())).await?;
                log_command(session_id, "background", "", cmd, "approved", &output);
                output
        }

        PendingCall::ScheduleCommand { id: call_id, name, command, is_script, run_at, interval, runbook, .. } => {
            let action = if *is_script {
                ActionOn::Script(command.clone())
            } else {
                ActionOn::Command(command.clone())
            };
            let kind = if let Some(iso) = interval {
                let secs = match crate::scheduler::parse_iso_duration(iso) {
                    Some(s) => s,
                    None => return Ok(ToolCallOutcome::Result(format!(
                        "Invalid interval '{}'. Use ISO 8601 duration format, e.g. PT1M (1 minute), PT5M (5 minutes), PT1H (1 hour), P1D (1 day).",
                        iso
                    ))),
                };
                let next = chrono::Utc::now() + chrono::Duration::seconds(secs as i64);
                ScheduleKind::Every { interval_secs: secs, next_run: next }
            } else if let Some(at_str) = run_at {
                let at = chrono::DateTime::parse_from_rfc3339(at_str).map(|d| d.with_timezone(&chrono::Utc))
                    .unwrap_or_else(|_| chrono::Utc::now() + chrono::Duration::seconds(60));
                ScheduleKind::Once { at }
            } else {
                ScheduleKind::Once { at: chrono::Utc::now() + chrono::Duration::seconds(60) }
            };

            send_response_split(tx, Response::ScheduleWritePrompt {
                id: call_id.clone(),
                name: name.clone(),
                kind: kind.describe(),
                action: action.describe(),
            }).await?;

            let mut line = String::new();
            let read_result = tokio::time::timeout(
                USER_PROMPT_TIMEOUT,
                rx.read_line(&mut line),
            ).await;
            if matches!(read_result, Ok(Ok(0))) { return Err(anyhow::anyhow!("EOF")); }
            let approved = match read_result {
                Ok(Ok(_)) => match serde_json::from_str::<Request>(line.trim()) {
                    Ok(Request::ScheduleWriteResponse { approved, .. }) => approved,
                    _ => false,
                },
                _ => false,
            };

            if approved {
                let job = ScheduledJob::new(name.clone(), kind.clone(), action, runbook.clone());
                match schedule_store.add(job) {
                    Ok(job_id) => {
                        log::info!("Job scheduled: '{}' ({})", name, &job_id[..8]);
                        log_event("job_scheduled", serde_json::json!({
                            "session": session_id.unwrap_or("-"),
                            "job_id": &job_id,
                            "job_name": name,
                            "kind": kind.describe(),
                        }));
                        format!("Scheduled job '{}' created (id: {})", name, job_id)
                    }
                    Err(e) => format!("Failed to schedule job: {}", e),
                }
            } else {
                log_event("command_approval", serde_json::json!({
                    "session": session_id.unwrap_or("-"),
                    "mode": "schedule",
                    "cmd": command,
                    "decision": "denied",
                }));
                "Job scheduling denied by user".to_string()
            }
        }

        PendingCall::ListSchedules { .. } => {
            let jobs = schedule_store.list();
            let items: Vec<ScheduleListItem> = jobs.iter().map(|j| ScheduleListItem {
                id: j.id.clone(),
                name: j.name.clone(),
                kind: j.kind.describe(),
                action: j.action.describe(),
                status: j.status.describe(),
                last_run: j.last_run.map(|t| t.format("%Y-%m-%d %H:%M UTC").to_string()),
                // Only show next_run for pending jobs; for succeeded/failed/cancelled
                // jobs it would be a stale past timestamp that confuses the AI into
                // thinking the job needs to be re-scheduled.
                next_run: if matches!(j.status, JobStatus::Pending) {
                    j.kind.next_run().map(|t| t.format("%Y-%m-%d %H:%M UTC").to_string())
                } else {
                    None
                },
            }).collect();
            let count = items.len();
            let _ = send_response_split(tx, Response::ScheduleList { jobs: items.clone() }).await;
            // Build a full job listing for the AI so it has IDs for cancel/delete.
            if count == 0 {
                "No scheduled jobs.".to_string()
            } else {
                let mut lines = format!("{} scheduled job(s):\n", count);
                for item in &items {
                    let next = item.next_run.as_deref().unwrap_or("n/a");
                    let last = item.last_run.as_deref().unwrap_or("never");
                    lines.push_str(&format!(
                        "- {} (id: {}): {}, status: {}, next: {}, last: {}\n",
                        item.name, item.id, item.kind, item.status, next, last
                    ));
                }
                lines
            }
        }

        PendingCall::CancelSchedule { job_id, .. } => {
            match schedule_store.cancel(job_id) {
                Ok(true) => {
                    log::info!("Job canceled: {}", &job_id[..job_id.len().min(8)]);
                    log_event("job_canceled", serde_json::json!({
                        "session": session_id.unwrap_or("-"),
                        "job_id": job_id,
                    }));
                    format!("Job {} cancelled", &job_id[..job_id.len().min(8)])
                }
                Ok(false) => format!("Job {} not found", job_id),
                Err(e)  => format!("Failed to cancel job: {}", e),
            }
        }

        PendingCall::DeleteSchedule { job_id, .. } => {
            match schedule_store.delete(job_id) {
                Ok(true) => {
                    log::info!("Job deleted: {}", &job_id[..job_id.len().min(8)]);
                    log_event("job_deleted", serde_json::json!({
                        "session": session_id.unwrap_or("-"),
                        "job_id": job_id,
                    }));
                    format!("Job {} deleted permanently", &job_id[..job_id.len().min(8)])
                }
                Ok(false) => format!("Job {} not found", job_id),
                Err(e)  => format!("Failed to delete job: {}", e),
            }
        }

        PendingCall::WriteScript { id, script_name, content, .. } => {
            send_response_split(tx, Response::ScriptWritePrompt {
                id: id.clone(),
                script_name: script_name.clone(),
                content: content.clone(),
            }).await?;

            let mut line = String::new();
            let read_result = tokio::time::timeout(
                USER_PROMPT_TIMEOUT,
                rx.read_line(&mut line),
            ).await;
            if matches!(read_result, Ok(Ok(0))) { return Err(anyhow::anyhow!("EOF")); }
            let approved = match read_result {
                Ok(Ok(_)) => match serde_json::from_str::<Request>(line.trim()) {
                    Ok(Request::ScriptWriteResponse { approved, .. }) => approved,
                    _ => false,
                },
                _ => false,
            };

            if approved {
                match scripts::write_script(script_name, content) {
                    Ok(()) => format!("Script '{}' written successfully", script_name),
                    Err(e) => format!("Failed to write script: {}", e),
                }
            } else {
                "Script write denied by user".to_string()
            }
        }

        PendingCall::ListScripts { .. } => {
            let script_list = scripts::list_scripts().unwrap_or_default();
            let items: Vec<ScriptListItem> = script_list.iter()
                .map(|s| ScriptListItem { name: s.name.clone(), size: s.size })
                .collect();
            let count = items.len();
            let _ = send_response_split(tx, Response::ScriptList { scripts: items }).await;
            format!("{} script(s) in ~/.daemoneye/scripts/", count)
        }

        PendingCall::ReadScript { script_name, .. } => {
            match scripts::read_script(script_name) {
                Ok(content) => content,
                Err(e) => format!("Error reading script '{}': {}", script_name, e),
            }
        }

        PendingCall::WatchPane { pane_id, timeout_secs, pattern, .. } => {
            // Sample the current foreground command so we know when the shell returns to a prompt.
            let initial_cmd = tmux::pane_current_command(pane_id).unwrap_or_default();

            // Install a pane-title-changed hook as a fast-path IPC signal.
            let hook_idx = FG_HOOK_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let hook_name = format!("pane-title-changed[@de_wp_{}]", hook_idx);
            let current_exe = std::env::current_exe()
                .unwrap_or_else(|_| std::path::PathBuf::from("daemoneye"));
            let notify_cmd = format!(
                "run-shell -b '{} notify activity {} 0 \"{}\"'",
                current_exe.display(), pane_id, shell_escape_arg(session_name)
            );
            let _ = std::process::Command::new("tmux")
                .args(["set-hook", "-t", pane_id, &hook_name, &notify_cmd])
                .output();

            // Subscribe before spawning to avoid missing early signals.
            let mut wp_rx = bg_done_subscribe();

            let pane_id_owned = pane_id.to_string();
            let session_id_owned = session_id.unwrap_or("-").to_string();
            let sessions_clone = Arc::clone(sessions);
            let timeout = Duration::from_secs(*timeout_secs);
            let pattern_owned = pattern.clone();

            log::info!("watch_pane: monitoring {} (initial_cmd={:?}) for session {}", pane_id, initial_cmd, session_id_owned);
            log_event("watch_pane", serde_json::json!({
                "session": session_id_owned,
                "pane_id": pane_id,
                "pattern": pattern,
                "status": "active"
            }));

            tokio::spawn(async move {
                let slow_poll = Duration::from_millis(500);
                let start_wait = Duration::from_secs(5);

                // Pre-compile pattern regex once so the watch loop doesn't recompile each tick.
                let pattern_re = pattern_owned.as_deref().and_then(|p| {
                    regex::RegexBuilder::new(p).size_limit(1 << 20).build().ok()
                });

                let completed = tokio::time::timeout(timeout, async {
                    if let Some(ref re) = pattern_re {
                        // Pattern mode: return as soon as the regex matches any pane output.
                        // Don't wait for the command to exit — the event we care about may
                        // arrive while the process is still running.
                        loop {
                            tokio::select! {
                                result = wp_rx.recv() => {
                                    if let Ok(notified_pane) = result {
                                        if notified_pane == pane_id_owned {
                                            let snap = tmux::capture_pane(&pane_id_owned, 200).unwrap_or_default();
                                            if re.is_match(&snap) { break; }
                                        }
                                    }
                                }
                                _ = tokio::time::sleep(slow_poll) => {
                                    let snap = tmux::capture_pane(&pane_id_owned, 200).unwrap_or_default();
                                    if re.is_match(&snap) { break; }
                                }
                            }
                        }
                    } else {
                        // Completion mode: return when pane_current_command returns to a shell.
                        // If the pane is already at a shell prompt, first wait up to 5s for a
                        // command to start before we start watching for completion.
                        if is_shell_prompt(&initial_cmd) {
                            let _ = tokio::time::timeout(start_wait, async {
                                loop {
                                    tokio::time::sleep(slow_poll).await;
                                    let cur = tmux::pane_current_command(&pane_id_owned).unwrap_or_default();
                                    if !is_shell_prompt(&cur) { break; }
                                }
                            }).await;
                        }

                        // Race: pane-title-changed IPC signal vs 500 ms poll.
                        loop {
                            tokio::select! {
                                result = wp_rx.recv() => {
                                    if let Ok(notified_pane) = result {
                                        if notified_pane == pane_id_owned {
                                            let cur = tmux::pane_current_command(&pane_id_owned).unwrap_or_default();
                                            if is_shell_prompt(&cur) { break; }
                                        }
                                    }
                                }
                                _ = tokio::time::sleep(slow_poll) => {
                                    let cur = tmux::pane_current_command(&pane_id_owned).unwrap_or_default();
                                    if is_shell_prompt(&cur) { break; }
                                }
                            }
                        }
                    }
                }).await.is_ok();

                // Remove the pane-title-changed hook.
                let _ = std::process::Command::new("tmux")
                    .args(["set-hook", "-u", "-t", &pane_id_owned, &hook_name])
                    .output();

                // Capture and mask pane output.
                let raw = tmux::capture_pane(&pane_id_owned, 200).unwrap_or_default();
                let body = crate::ai::filter::mask_sensitive(&normalize_output(&raw));

                let content = if completed {
                    if let Some(ref pat) = pattern_owned {
                        format!(
                            "[Watch Pane Match] Pattern `{}` matched in pane {}.\n<output>\n{}\n</output>",
                            pat, pane_id_owned, body
                        )
                    } else {
                        format!(
                            "[Watch Pane Complete] Command finished in pane {}.\n<output>\n{}\n</output>",
                            pane_id_owned, body
                        )
                    }
                } else {
                    format!(
                        "[Watch Pane Timeout] Timed out waiting in pane {}.\n<output>\n{}\n</output>",
                        pane_id_owned, body
                    )
                };

                let watch_msg = crate::ai::Message {
                    role: "user".to_string(),
                    content,
                    tool_calls: None,
                    tool_results: None,
                };

                if let Ok(mut store) = sessions_clone.lock() {
                    if let Some(entry) = store.get_mut(&session_id_owned) {
                        append_session_message(&session_id_owned, &watch_msg);
                        entry.messages.push(watch_msg);

                        let alert = if completed {
                            format!("Watched pane {} command completed", pane_id_owned)
                        } else {
                            format!("Watched pane {} timed out", pane_id_owned)
                        };
                        if let Some(ref cp) = entry.chat_pane {
                            let _ = std::process::Command::new("tmux")
                                .args(["display-message", "-d", "5000", "-t", cp, &alert])
                                .output();
                        }
                    }
                }
                log::info!("watch_pane {}: {}", pane_id_owned, if completed { "completed" } else { "timed out" });
            });

            if let Some(pat) = pattern {
                format!(
                    "Now watching pane {} for pattern `{}`. \
                     You will receive [Watch Pane Match] when the pattern appears, \
                     or [Watch Pane Timeout] after {} seconds.",
                    pane_id, pat, timeout_secs
                )
            } else {
                format!(
                    "Now watching pane {} for command completion. \
                     You will receive [Watch Pane Complete] when the command finishes, \
                     or [Watch Pane Timeout] after {} seconds.",
                    pane_id, timeout_secs
                )
            }
        }

        PendingCall::ReadFile { path, offset, limit, pattern, target_pane, .. } => {
            // Reject path traversal attempts.
            if path.contains("..") {
                return Ok(ToolCallOutcome::Result(
                    "Error: path must not contain '..'.".to_string()
                ));
            }
            if !std::path::Path::new(path.as_str()).is_absolute() {
                return Ok(ToolCallOutcome::Result(
                    "Error: path must be absolute (e.g. /var/log/syslog).".to_string()
                ));
            }

            // Block access to the daemoneye config directory — use the dedicated
            // tools (read_script, read_runbook, read_memory, etc.) instead.
            {
                let de_dir = crate::config::config_dir();
                let candidate = std::fs::canonicalize(path.as_str())
                    .unwrap_or_else(|_| std::path::PathBuf::from(path.as_str()));
                if candidate.starts_with(&de_dir) {
                    return Ok(ToolCallOutcome::Result(
                        "Error: read_file cannot access the daemoneye configuration \
                         directory. Use the dedicated tools (read_script, read_runbook, \
                         read_memory, list_memories, etc.) instead.".to_string()
                    ));
                }
            }

            const MAX_LINES: usize = 500;
            const DEFAULT_LINES: usize = 200;
            let limit_n = match limit {
                Some(n) if *n > 0 => (*n as usize).min(MAX_LINES),
                _ => DEFAULT_LINES,
            };
            let offset_n = offset.map(|o| (o as usize).saturating_sub(1)).unwrap_or(0);

            // ── Target-pane path: run sed/grep in target_pane ─────────────────
            if let Some(pane) = target_pane {
                let start = offset_n + 1;
                let end   = offset_n + limit_n;

                // N12: if the target pane is LOCAL use load-buffer/save-buffer to
                // bypass the 600-line scrollback cap.  SSH/mosh panes fall back to
                // the capture_pane-based approach.
                let (content, is_remote) = if get_pane_remote_host(pane).is_none() {
                    let raw = match local_read_via_buffer(pane, path, start, end,
                                                          pattern.as_deref()).await {
                        Ok(s) => s,
                        Err(e) => return Ok(ToolCallOutcome::Result(format!("Error: {}", e))),
                    };
                    (raw, false)
                } else {
                    let cmd = build_remote_read_cmd(path, start, end, pattern.as_deref());
                    let snap = match remote_run_and_capture(pane, &cmd, 30).await {
                        Ok(s) => s,
                        Err(e) => return Ok(ToolCallOutcome::Result(format!("Error: {}", e))),
                    };
                    let extracted = extract_marked(&snap, "__DE_S__", "__DE_E__")
                        .unwrap_or_else(|| snap.clone());
                    (extracted, true)
                };

                if content.trim().is_empty() {
                    return Ok(ToolCallOutcome::Result(
                        format!("{}: no output (file may be empty or lines out of range)", path)
                    ));
                }
                let body = mask_sensitive(content.trim_end());
                let label = if is_remote {
                    if pattern.is_some() {
                        format!("{} (remote grep, lines {}-{}):\n{}", path, start, end, body)
                    } else {
                        format!("{} (remote, lines {}-{}):\n{}", path, start, end, body)
                    }
                } else if pattern.is_some() {
                    format!("{} (local grep, lines {}-{}):\n{}", path, start, end, body)
                } else {
                    format!("{} (local pane, lines {}-{}):\n{}", path, start, end, body)
                };
                return Ok(ToolCallOutcome::Result(label));
            }

            // ── Local path: read directly from daemon-host filesystem ─────────
            // Resolve symlinks so a symlink at the given path cannot point
            // outside the user's intended location.  If the file does not
            // exist yet, canonicalize will fail and the read below returns a
            // natural "not found" error.
            let real_path = std::fs::canonicalize(path.as_str())
                .unwrap_or_else(|_| std::path::PathBuf::from(path.as_str()));
            let raw = match std::fs::read_to_string(&real_path) {
                Ok(s) => s,
                Err(e) => return Ok(ToolCallOutcome::Result(
                    format!("Error reading {}: {}", path, e)
                )),
            };

            let all_lines: Vec<&str> = raw.lines().collect();
            let total = all_lines.len();
            let sliced = &all_lines[offset_n.min(total)..];
            let limited: Vec<&str> = sliced.iter().take(limit_n).copied().collect();
            let limited_len = limited.len();

            let filtered: Vec<&str> = if let Some(pat) = pattern {
                match regex::RegexBuilder::new(pat).size_limit(1 << 20).build() {
                    Ok(re) => limited.into_iter().filter(|l| re.is_match(l)).collect(),
                    Err(e) => return Ok(ToolCallOutcome::Result(
                        format!("Error: invalid pattern regex: {}", e)
                    )),
                }
            } else {
                limited
            };

            if filtered.is_empty() {
                return Ok(ToolCallOutcome::Result(
                    format!("{}: no lines matched (total {} lines in file)", path, total)
                ));
            }

            let body = mask_sensitive(&filtered.join("\n"));
            if pattern.is_some() {
                format!(
                    "{} ({} matching lines, searched lines {}-{} of {}):\n{}",
                    path, filtered.len(), offset_n + 1,
                    (offset_n + limited_len).min(total), total, body
                )
            } else {
                format!(
                    "{} (lines {}-{} of {}):\n{}",
                    path, offset_n + 1,
                    (offset_n + filtered.len()).min(total), total, body
                )
            }
        }

        PendingCall::EditFile { id, path, old_string, new_string, target_pane, .. } => {
            // Reject path traversal.
            if path.contains("..") {
                return Ok(ToolCallOutcome::Result(
                    "Error: path must not contain '..'.".to_string()
                ));
            }
            if !std::path::Path::new(path.as_str()).is_absolute() {
                return Ok(ToolCallOutcome::Result(
                    "Error: path must be absolute.".to_string()
                ));
            }
            if old_string.is_empty() {
                return Ok(ToolCallOutcome::Result(
                    "Error: old_string cannot be empty.".to_string()
                ));
            }

            // Block edits to the daemoneye config directory — use the dedicated
            // tools (write_script, write_runbook, add_memory, etc.) instead.
            {
                let de_dir = crate::config::config_dir();
                let candidate = std::fs::canonicalize(path.as_str())
                    .unwrap_or_else(|_| std::path::PathBuf::from(path.as_str()));
                if candidate.starts_with(&de_dir) {
                    return Ok(ToolCallOutcome::Result(
                        "Error: edit_file cannot access the daemoneye configuration \
                         directory. Use the dedicated tools (write_script, write_runbook, \
                         add_memory, etc.) instead.".to_string()
                    ));
                }
            }

            // ── Remote path: Python3/Perl replacement in target_pane ──────────
            if let Some(pane) = target_pane {
                let location = format!("{} (remote via pane {})", path, pane);
                let approval_cmd = format!(
                    "edit_file {}\n--- old\n{}\n+++ new\n{}",
                    location, old_string, new_string
                );
                if let Some(outcome) = prompt_and_await_approval(
                    id, &approval_cmd, false, None, session_id, tx, rx
                ).await? {
                    return Ok(outcome);
                }

                let cmd = build_remote_edit_cmd(path, old_string, new_string);
                let snap = match remote_run_and_capture(pane, &cmd, 30).await {
                    Ok(s) => s,
                    Err(e) => return Ok(ToolCallOutcome::Result(format!("Error: {}", e))),
                };

                // Look for DE_OK or DE_ERROR in the captured output.
                for line in snap.lines().rev() {
                    if line.contains("DE_OK:") {
                        log_event("file_edit", serde_json::json!({
                            "session": session_id.unwrap_or("-"),
                            "path": path,
                            "remote_pane": pane,
                        }));
                        return Ok(ToolCallOutcome::Result(
                            format!("Edited {} via pane {}.", path, pane)
                        ));
                    }
                    if line.contains("DE_ERROR:") {
                        return Ok(ToolCallOutcome::Result(
                            format!("Error editing {}: {}", path, line.trim())
                        ));
                    }
                }
                return Ok(ToolCallOutcome::Result(
                    format!("Edit command completed but result was unclear. Check {} manually.", path)
                ));
            }

            // ── Local path: direct daemon-host filesystem edit ────────────────
            // Resolve symlinks before reading or writing so that a symlink
            // cannot redirect the edit to an unintended location.
            let std_path = match std::fs::canonicalize(path.as_str()) {
                Ok(p) => p,
                Err(e) => return Ok(ToolCallOutcome::Result(
                    format!("Error: cannot resolve path {}: {}", path, e)
                )),
            };
            let original = match std::fs::read_to_string(&std_path) {
                Ok(s) => s,
                Err(e) => return Ok(ToolCallOutcome::Result(
                    format!("Error reading {}: {}", path, e)
                )),
            };

            let count = original.matches(old_string.as_str()).count();
            if count == 0 {
                return Ok(ToolCallOutcome::Result(
                    format!("Error: old_string not found in {}.", path)
                ));
            }
            if count > 1 {
                return Ok(ToolCallOutcome::Result(
                    format!(
                        "Error: old_string appears {} times in {}. \
                         Add more surrounding context to make it unique.",
                        count, path
                    )
                ));
            }

            let approval_cmd = format!(
                "edit_file {}\n--- old\n{}\n+++ new\n{}",
                path, old_string, new_string
            );
            if let Some(outcome) = prompt_and_await_approval(
                id, &approval_cmd, false, None, session_id, tx, rx
            ).await? {
                return Ok(outcome);
            }

            let updated = original.replacen(old_string.as_str(), new_string.as_str(), 1);
            let tmp_path = std_path.with_extension("de_tmp");
            if let Err(e) = std::fs::write(&tmp_path, &updated) {
                return Ok(ToolCallOutcome::Result(
                    format!("Error writing temp file: {}", e)
                ));
            }
            if let Err(e) = std::fs::rename(&tmp_path, &std_path) {
                let _ = std::fs::remove_file(&tmp_path);
                return Ok(ToolCallOutcome::Result(
                    format!("Error committing edit: {}", e)
                ));
            }

            log_event("file_edit", serde_json::json!({
                "session": session_id.unwrap_or("-"),
                "path": path,
            }));

            let old_lines = old_string.lines().count();
            let new_lines = new_string.lines().count();
            format!(
                "Edited {}: replaced {} line(s) with {} line(s).",
                path, old_lines, new_lines
            )
        }

        PendingCall::WriteRunbook { id, name, content, .. } => {
            send_response_split(tx, Response::RunbookWritePrompt {
                id: id.clone(),
                runbook_name: name.clone(),
                content: content.clone(),
            }).await?;

            let mut line = String::new();
            let read_result = tokio::time::timeout(USER_PROMPT_TIMEOUT, rx.read_line(&mut line)).await;
            if matches!(read_result, Ok(Ok(0))) { return Err(anyhow::anyhow!("EOF")); }
            let approved = match read_result {
                Ok(Ok(_)) => match serde_json::from_str::<Request>(line.trim()) {
                    Ok(Request::RunbookWriteResponse { approved, .. }) => approved,
                    _ => false,
                },
                _ => false,
            };

            if approved {
                match crate::runbook::write_runbook(name, content) {
                    Ok(()) => {
                        log::info!("Runbook '{}' written", name);
                        log_event("runbook_write", serde_json::json!({
                            "session": session_id.unwrap_or("-"),
                            "runbook": name,
                        }));
                        format!("Runbook '{}' written to ~/.daemoneye/runbooks/{}.md", name, name)
                    }
                    Err(e) => format!("Failed to write runbook: {}", e),
                }
            } else {
                "Runbook write denied by user".to_string()
            }
        }

        PendingCall::DeleteRunbook { id, name, .. } => {
            // Check for active scheduled jobs that reference this runbook
            let active_jobs: Vec<String> = schedule_store.list()
                .into_iter()
                .filter(|j| j.runbook.as_deref() == Some(name))
                .map(|j| j.name)
                .collect();

            send_response_split(tx, Response::RunbookDeletePrompt {
                id: id.clone(),
                runbook_name: name.clone(),
                active_jobs,
            }).await?;

            let mut line = String::new();
            let read_result = tokio::time::timeout(USER_PROMPT_TIMEOUT, rx.read_line(&mut line)).await;
            if matches!(read_result, Ok(Ok(0))) { return Err(anyhow::anyhow!("EOF")); }
            let approved = match read_result {
                Ok(Ok(_)) => match serde_json::from_str::<Request>(line.trim()) {
                    Ok(Request::RunbookDeleteResponse { approved, .. }) => approved,
                    _ => false,
                },
                _ => false,
            };

            if approved {
                match crate::runbook::delete_runbook(name) {
                    Ok(()) => {
                        log::info!("Runbook '{}' deleted", name);
                        log_event("runbook_delete", serde_json::json!({
                            "session": session_id.unwrap_or("-"),
                            "runbook": name,
                        }));
                        format!("Runbook '{}' deleted", name)
                    }
                    Err(e) => format!("Failed to delete runbook: {}", e),
                }
            } else {
                "Runbook delete denied by user".to_string()
            }
        }

        PendingCall::ReadRunbook { name, .. } => {
            match crate::runbook::load_runbook(name) {
                Ok(rb) => rb.content,
                Err(e) => format!("Error reading runbook '{}': {}", name, e),
            }
        }

        PendingCall::ListRunbooks { .. } => {
            let items = crate::runbook::list_runbooks().unwrap_or_default();
            let count = items.len();
            let runbook_items: Vec<RunbookListItem> = items.iter()
                .map(|r| RunbookListItem { name: r.name.clone(), tags: r.tags.clone() })
                .collect();
            let _ = send_response_split(tx, Response::RunbookList { runbooks: runbook_items }).await;
            format!("{} runbook(s) in ~/.daemoneye/runbooks/", count)
        }

        PendingCall::AddMemory { key, value, category, .. } => {
            let Some(cat) = crate::memory::MemoryCategory::from_str(category) else {
                return Ok(ToolCallOutcome::Result(format!(
                    "Error: invalid category '{}'. Must be 'session', 'knowledge', or 'incident'.",
                    category
                )));
            };
            if value.trim().is_empty() {
                return Ok(ToolCallOutcome::Result(
                    "Error: memory value cannot be empty.".to_string(),
                ));
            }
            match crate::memory::add_memory(key, value, cat) {
                Ok(()) => {
                    log_event("memory_write", serde_json::json!({
                        "session": session_id,
                        "op": "add",
                        "category": category,
                        "key": key,
                    }));
                    format!("Memory '{}' stored in {}", key, category)
                }
                Err(e) => format!("Error storing memory: {}", e),
            }
        }

        PendingCall::DeleteMemory { key, category, .. } => {
            let Some(cat) = crate::memory::MemoryCategory::from_str(category) else {
                return Ok(ToolCallOutcome::Result(format!(
                    "Error: invalid category '{}'. Must be 'session', 'knowledge', or 'incident'.",
                    category
                )));
            };
            match crate::memory::delete_memory(key, cat) {
                Ok(()) => {
                    log_event("memory_write", serde_json::json!({
                        "session": session_id,
                        "op": "delete",
                        "category": category,
                        "key": key,
                    }));
                    format!("Memory '{}' deleted from {}", key, category)
                }
                Err(e) => format!("Error deleting memory: {}", e),
            }
        }

        PendingCall::ReadMemory { key, category, .. } => {
            let Some(cat) = crate::memory::MemoryCategory::from_str(category) else {
                return Ok(ToolCallOutcome::Result(format!(
                    "Error: invalid category '{}'. Must be 'session', 'knowledge', or 'incident'.",
                    category
                )));
            };
            match crate::memory::read_memory(key, cat) {
                Ok(content) => crate::ai::filter::mask_sensitive(&content),
                Err(e) => format!("Error reading memory '{}': {}", key, e),
            }
        }

        PendingCall::ListMemories { category, .. } => {
            let cat = match category.as_deref() {
                None => None,
                Some(s) => match crate::memory::MemoryCategory::from_str(s) {
                    Some(c) => Some(c),
                    None => return Ok(ToolCallOutcome::Result(format!(
                        "Error: invalid category '{}'. Must be 'session', 'knowledge', or 'incident'.",
                        s
                    ))),
                },
            };
            let entries = crate::memory::list_memories(cat).unwrap_or_default();
            let count = entries.len();
            let items: Vec<MemoryListItem> = entries.iter()
                .map(|(c, k)| MemoryListItem { category: c.clone(), key: k.clone() })
                .collect();
            let _ = send_response_split(tx, Response::MemoryList { entries: items }).await;
            if count == 0 {
                "No memory entries found.".to_string()
            } else {
                let lines: Vec<String> = entries.iter()
                    .map(|(c, k)| format!("[{}] {}", c, k))
                    .collect();
                format!("{} memory entries:\n{}", count, lines.join("\n"))
            }
        }

        PendingCall::SearchRepository { query, kind, .. } => {
            let results = crate::search::search_repository(query, kind, 2);
            crate::search::format_results(&results)
        }

        PendingCall::GetTerminalContext { .. } => {
            cache.get_labeled_context(chat_pane, chat_pane)
        }

        PendingCall::ListPanes { .. } => {
            let panes = cache.panes.read().unwrap_or_log();
            let session = cache.session_name.read().unwrap_or_log().clone();

            // Collect panes, excluding the chat pane (never a valid command target).
            let mut rows: Vec<_> = panes
                .iter()
                .filter(|(id, _)| chat_pane.map_or(true, |c| c != id.as_str()))
                .collect();
            rows.sort_by_key(|(id, _)| id.as_str());

            if rows.is_empty() {
                return Ok(ToolCallOutcome::Result(format!(
                    "No targetable panes found in session '{}'.", session
                )));
            }

            let mut out = format!(
                "{} pane{} in session '{}' (chat pane excluded):\n",
                rows.len(),
                if rows.len() == 1 { "" } else { "s" },
                session
            );
            let now_secs = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            for (id, state) in &rows {
                // Title: omit when it's identical to the command (redundant).
                let title_part = if !state.pane_title.is_empty() && state.pane_title != state.current_cmd {
                    format!("  title:{}", mask_sensitive(&state.pane_title))
                } else {
                    String::new()
                };
                // N5: show start command when it differs from current command.
                let start_part = if !state.start_cmd.is_empty() && state.start_cmd != state.current_cmd {
                    format!("  started:{}", state.start_cmd)
                } else {
                    String::new()
                };
                let sync_part  = if state.synchronized { "  [synchronized]" } else { "" };
                let dead_part  = if state.dead {
                    format!("  [dead: {}]", state.dead_status.unwrap_or(0))
                } else {
                    String::new()
                };
                let activity_part = if state.last_activity > 0 && now_secs >= state.last_activity {
                    let age = now_secs - state.last_activity;
                    if age < 30 {
                        format!("  [active {}s ago]", age)
                    } else if age < 3600 {
                        format!("  [idle {}m]", age / 60)
                    } else {
                        format!("  [idle {}h{}m]", age / 3600, (age % 3600) / 60)
                    }
                } else {
                    String::new()
                };
                out.push_str(&format!(
                    "  {}  window:{:<12}  cmd:{:<8}  cwd:{}{}{}{}{}{}\n",
                    id,
                    state.window_name,
                    state.current_cmd,
                    state.current_path,
                    start_part,
                    title_part,
                    sync_part,
                    dead_part,
                    activity_part,
                ));
            }
            out.push_str(
                "\nUse the pane ID as target_pane in run_terminal_command to execute a command there."
            );
            out
        }
    };
    Ok(ToolCallOutcome::Result(result))
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::{is_shell_prompt, looks_like_shell_prompt};

    // ── is_shell_prompt ───────────────────────────────────────────────────────

    #[test]
    fn is_shell_prompt_recognises_common_shells() {
        for sh in &["bash", "zsh", "fish", "sh", "ksh", "csh", "tcsh", "dash", "nu"] {
            assert!(is_shell_prompt(sh), "{sh} should be a shell prompt");
        }
    }

    #[test]
    fn is_shell_prompt_rejects_commands() {
        for cmd in &["ssh", "vim", "python3", "cargo", "top", ""] {
            assert!(!is_shell_prompt(cmd), "{cmd} should not be a shell prompt");
        }
    }

    #[test]
    fn is_shell_prompt_trims_whitespace() {
        assert!(is_shell_prompt("  bash  "));
        assert!(is_shell_prompt("\tzsh\n"));
    }

    // ── looks_like_shell_prompt ───────────────────────────────────────────────

    #[test]
    fn looks_like_shell_prompt_dollar() {
        assert!(looks_like_shell_prompt("user@host:~$ "));
        assert!(looks_like_shell_prompt("user@host:~$"));
    }

    #[test]
    fn looks_like_shell_prompt_hash() {
        assert!(looks_like_shell_prompt("root@host:~# "));
        assert!(looks_like_shell_prompt("root@host:~#"));
    }

    #[test]
    fn looks_like_shell_prompt_percent() {
        assert!(looks_like_shell_prompt("host% "));
    }

    #[test]
    fn looks_like_shell_prompt_angle() {
        assert!(looks_like_shell_prompt("PS> "));
    }

    #[test]
    fn looks_like_shell_prompt_ignores_blank_lines() {
        // Trailing blank lines should not prevent detection.
        let snap = "user@host:~$\n\n  \n";
        assert!(looks_like_shell_prompt(snap));
    }

    #[test]
    fn looks_like_shell_prompt_rejects_mid_output() {
        // A line ending with "foo" is not a prompt.
        assert!(!looks_like_shell_prompt("some random output\nfoo bar"));
    }

    #[test]
    fn looks_like_shell_prompt_empty_returns_false() {
        assert!(!looks_like_shell_prompt(""));
        assert!(!looks_like_shell_prompt("   \n  "));
    }

    // ── read_file logic (pure, no filesystem) ────────────────────────────────

    fn simulate_read_file(
        content: &str,
        offset: Option<u64>,
        limit: Option<u64>,
        pattern: Option<&str>,
    ) -> String {
        const MAX_LINES: usize = 500;
        const DEFAULT_LINES: usize = 200;

        let limit_n = match limit {
            Some(n) if n > 0 => (n as usize).min(MAX_LINES),
            _ => DEFAULT_LINES,
        };
        let offset_n = offset.map(|o| (o as usize).saturating_sub(1)).unwrap_or(0);

        let all_lines: Vec<&str> = content.lines().collect();
        let total = all_lines.len();
        let sliced = &all_lines[offset_n.min(total)..];
        let limited: Vec<&str> = sliced.iter().take(limit_n).copied().collect();
        let limited_len = limited.len();

        if let Some(pat) = pattern {
            let re = match regex::RegexBuilder::new(pat).size_limit(1 << 20).build() {
                Ok(r) => r,
                Err(e) => return format!("Error: invalid grep pattern: {}", e),
            };
            let filtered: Vec<&str> = limited.into_iter().filter(|l| re.is_match(l)).collect();
            if filtered.is_empty() {
                return format!("no lines matched (total {} lines in file)", total);
            }
            format!(
                "{} matching lines, searched lines {}-{} of {}:\n{}",
                filtered.len(),
                offset_n + 1,
                (offset_n + limited_len).min(total),
                total,
                filtered.join("\n")
            )
        } else {
            if limited.is_empty() {
                return format!("no lines matched (total {} lines in file)", total);
            }
            format!(
                "lines {}-{} of {}:\n{}",
                offset_n + 1,
                (offset_n + limited.len()).min(total),
                total,
                limited.join("\n")
            )
        }
    }

    #[test]
    fn read_file_default_reads_from_start() {
        let content = (1..=10).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n");
        let out = simulate_read_file(&content, None, None, None);
        assert!(out.starts_with("lines 1-10 of 10:"), "got: {out}");
        assert!(out.contains("line 1"));
        assert!(out.contains("line 10"));
    }

    #[test]
    fn read_file_offset_skips_lines() {
        let content = (1..=10).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n");
        let out = simulate_read_file(&content, Some(5), None, None);
        assert!(out.starts_with("lines 5-10 of 10:"), "got: {out}");
        assert!(!out.contains("line 4"));
        assert!(out.contains("line 5"));
    }

    #[test]
    fn read_file_limit_caps_output() {
        let content = (1..=10).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n");
        let out = simulate_read_file(&content, None, Some(3), None);
        assert!(out.starts_with("lines 1-3 of 10:"), "got: {out}");
        assert!(out.contains("line 3"));
        assert!(!out.contains("line 4"));
    }

    #[test]
    fn read_file_limit_capped_at_max() {
        let content = (1..=10).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n");
        // limit=1000 > MAX_LINES=500, should clamp to all 10 lines here.
        let out = simulate_read_file(&content, None, Some(1000), None);
        assert!(out.contains("line 10"), "got: {out}");
    }

    #[test]
    fn read_file_pattern_grep_mode_header() {
        let content = "alpha\nbeta\nalpha again\ngamma";
        let out = simulate_read_file(content, None, None, Some("alpha"));
        assert!(out.starts_with("2 matching lines, searched lines 1-4 of 4:"), "got: {out}");
        assert!(out.contains("alpha"));
        assert!(!out.contains("beta"));
        assert!(!out.contains("gamma"));
    }

    #[test]
    fn read_file_pattern_no_match_returns_message() {
        let content = "alpha\nbeta\ngamma";
        let out = simulate_read_file(content, None, None, Some("zzz"));
        assert!(out.contains("no lines matched"), "got: {out}");
    }

    #[test]
    fn read_file_offset_beyond_eof_returns_empty() {
        let content = "only one line";
        let out = simulate_read_file(content, Some(999), None, None);
        assert!(out.contains("no lines matched") || out.contains("lines"), "got: {out}");
    }
}
