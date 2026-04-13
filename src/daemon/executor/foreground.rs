use super::find_best_target_pane;
use super::prompt_and_await_approval;
use super::send_response_split;
use super::{ApprovalRequest, GhostCtx, SessionCtx, ToolCallOutcome};
use crate::ai::mask_sensitive;
use crate::daemon::background::{respawn_background_in_pane, run_background_in_window};
use crate::daemon::session::{FG_HOOK_COUNTER, bg_done_subscribe};
use crate::daemon::utils::{
    command_has_sudo, extract_command_output, fingerprint_pam_configured, get_pane_remote_host,
    interactive_destination, is_fingerprint_prompt, is_interactive_command, log_command,
    normalize_output, shell_escape_arg, sudo_auth_failed, sudo_credentials_cached,
    wait_for_sudo_prompt_and_inject,
};
use crate::ipc::{Request, Response};
use crate::tmux;
use crate::util::UnpoisonExt;

pub(super) struct FgArgs<'a> {
    pub id: &'a str,
    pub cmd: &'a str,
    pub target: Option<&'a str>,
}
use crate::tmux::cache::SessionCache;
use std::sync::Arc;
use std::time::Duration;

// Timing constants specific to command execution.
const SUDO_POLL_INTERVAL: Duration = Duration::from_millis(100);
const SUDO_DETECT_WINDOW: Duration = Duration::from_secs(3);
/// Maximum sudo password attempts before aborting (matches sudo's own default).
const MAX_SUDO_RETRIES: usize = 3;
const REMOTE_POLL_INTERVAL: Duration = Duration::from_millis(500);
const REMOTE_CMD_TIMEOUT: Duration = Duration::from_secs(30);
const LOCAL_CHILD_POLL: Duration = Duration::from_millis(25);
const LOCAL_CHILD_START_WINDOW: Duration = Duration::from_millis(300);
const LOCAL_CMD_TIMEOUT: Duration = Duration::from_secs(45);
const LOCAL_SLOW_POLL: Duration = Duration::from_millis(500);
const POST_CMD_CAPTURE_DELAY: Duration = Duration::from_millis(50);
const SILENCE_MONITOR_SECS: u32 = 2;
const INTERACTIVE_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const INTERACTIVE_POLL_INTERVAL: Duration = Duration::from_millis(300);
const INTERACTIVE_STABLE_WINDOW: Duration = Duration::from_millis(600);

// ---------------------------------------------------------------------------
// RAII guard for tmux hooks installed during foreground command execution.
// ---------------------------------------------------------------------------

/// Uninstalls tmux hooks on drop so that early returns via `?` or panics
/// never leave stale `pane-title-changed` or `alert-silence` hooks behind.
struct FgHookGuard {
    target: String,
    hooks: Vec<String>,
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

// ---------------------------------------------------------------------------
// Shell prompt detection helpers (also used by knowledge::watch_pane).
// ---------------------------------------------------------------------------

/// Return true when `cmd` is a shell name, meaning the pane is at a prompt.
pub(super) fn is_shell_prompt(cmd: &str) -> bool {
    matches!(
        cmd.trim(),
        "bash"
            | "zsh"
            | "fish"
            | "sh"
            | "ksh"
            | "csh"
            | "tcsh"
            | "dash"
            | "nu"
            | "pwsh"
            | "elvish"
            | "xonsh"
            | "yash"
    )
}

/// Return true when the last non-empty line of a pane snapshot ends with a
/// recognisable shell-prompt character.
pub(super) fn looks_like_shell_prompt(snap: &str) -> bool {
    snap.lines()
        .rfind(|l| !l.trim().is_empty())
        .map(|l| {
            let t = l.trim_end();
            t.ends_with("$ ")
                || t.ends_with("# ")
                || t.ends_with("% ")
                || t.ends_with("> ")
                || t.ends_with('$')
                || t.ends_with('#')
                || t.ends_with('%')
                || t.ends_with('>')
        })
        .unwrap_or(false)
}

// ---------------------------------------------------------------------------
// Foreground command execution
// ---------------------------------------------------------------------------

pub(super) async fn run_foreground<W, R>(
    args: FgArgs<'_>,
    ctx: SessionCtx<'_>,
    cache: &Arc<SessionCache>,
    ghost_ctx: GhostCtx<'_>,
    tx: &mut W,
    rx: &mut R,
) -> anyhow::Result<ToolCallOutcome>
where
    W: tokio::io::AsyncWriteExt + Unpin,
    R: tokio::io::AsyncBufReadExt + Unpin,
{
    let FgArgs { id, cmd, target } = args;
    let SessionCtx {
        session_id,
        session_name,
        chat_pane,
        sessions,
    } = ctx;
    let GhostCtx {
        policy: ghost_policy,
        is_ghost: _,
    } = ghost_ctx;

    // C3a: pane ID format guard — reject anything that doesn't look like %N.
    // Models occasionally pass window-relative indices ("0", "1") instead of
    // the actual tmux pane ID ("%7").  Catch this early and return a corrective
    // error so the model can self-fix without a silent wrong-pane execution.
    if let Some(tp) = target
        && !tp.is_empty()
        && chat_pane != Some(tp)
    {
        let valid_format =
            tp.starts_with('%') && tp.len() > 1 && tp[1..].bytes().all(|b| b.is_ascii_digit());
        if !valid_format {
            let correct = session_id
                .and_then(|sid| sessions.lock().ok()?.get(sid)?.default_target_pane.clone())
                .unwrap_or_default();
            let suggestion = if correct.is_empty() {
                "Check [PANE MAP] or call list_panes to find the correct pane ID.".to_string()
            } else {
                format!(
                    "The foreground target for this session is {correct}. \
                     Pass target_pane=\"{correct}\" or omit it to use the default."
                )
            };
            let msg = format!(
                "Error: '{tp}' is not a valid tmux pane ID. \
                 Pane IDs start with '%' followed by digits (e.g. \"%3\"). \
                 {suggestion}"
            );
            send_response_split(tx, Response::ToolResult(msg.clone())).await?;
            return Ok(ToolCallOutcome::Result(msg));
        }
    }

    // C3b: stale-pane guard — if the AI specified a target_pane that is no longer
    // in the cache (pane was closed or session changed), return an error with the
    // current pane map so the AI can re-discover panes before retrying.
    if let Some(tp) = target
        && chat_pane != Some(tp)
    {
        let pane_exists = {
            let panes = cache.panes.read().unwrap_or_log();
            panes.contains_key(tp)
        };
        if !pane_exists {
            let correct = session_id
                .and_then(|sid| sessions.lock().ok()?.get(sid)?.default_target_pane.clone())
                .unwrap_or_default();
            let suggestion = if correct.is_empty() {
                "Call list_panes to discover current pane IDs, or use the [PANE MAP] below."
                    .to_string()
            } else {
                format!(
                    "The foreground target for this session is {correct}. \
                     Pass target_pane=\"{correct}\" or omit it to use the default."
                )
            };
            let pane_map = cache.pane_map_summary(chat_pane);
            let msg = format!(
                "Error: target_pane '{tp}' no longer exists in the current session. \
                 {suggestion}\n{pane_map}"
            );
            send_response_split(tx, Response::ToolResult(msg.clone())).await?;
            return Ok(ToolCallOutcome::Result(msg));
        }
    }

    // Compute a best-guess target pane hint synchronously so the approval
    // prompt can show which pane will be used.
    let target_hint: Option<String> = (|| {
        if let Some(tp) = target
            && chat_pane != Some(tp)
        {
            let panes = cache.panes.read().unwrap_or_log();
            if panes.contains_key(tp) {
                return Some(tp.to_string());
            }
        }
        if let Some(sid) = session_id
            && let Ok(store) = sessions.lock()
            && let Some(entry) = store.get(sid)
            && let Some(ref dtp) = entry.default_target_pane
            && chat_pane != Some(dtp.as_str())
        {
            let panes = cache.panes.read().unwrap_or_log();
            if panes.contains_key(dtp) {
                return Some(dtp.clone());
            }
        }
        None
    })();

    let cmd_id = match prompt_and_await_approval(
        ApprovalRequest {
            id,
            cmd,
            background: false,
            target_pane_hint: target_hint.as_deref(),
        },
        session_id,
        ghost_policy,
        tx,
        rx,
    )
    .await?
    {
        Ok(id) => id,
        Err(outcome) => return Ok(outcome),
    };

    let target_owned =
        match find_best_target_pane(target, chat_pane, cache, sessions, session_id, tx, rx).await {
            Ok(tp) => tp,
            Err(_) => return Err(anyhow::anyhow!("EOF")),
        };

    let target_str = target_owned.as_str();
    if target_str.is_empty() {
        return Ok(ToolCallOutcome::Result("No active pane found.".to_string()));
    }

    let is_synchronized = {
        let panes = cache.panes.read().unwrap_or_log();
        panes
            .get(target_str)
            .map(|p| p.synchronized)
            .unwrap_or(false)
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
        return Ok(ToolCallOutcome::Result(msg));
    }

    let idle_pid = tmux::pane_pid(target_str).unwrap_or(0);
    let is_remote_pane = get_pane_remote_host(target_str).is_some();

    let current_exe =
        std::env::current_exe().unwrap_or_else(|_| std::path::PathBuf::from("daemoneye"));
    let hook_idx = FG_HOOK_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let hook_name = format!("pane-title-changed[@de_fg_{}]", hook_idx);
    let notify_cmd = format!(
        "run-shell -b '{} notify activity {} 0 \"{}\"'",
        current_exe.display(),
        target_str,
        shell_escape_arg(session_name)
    );
    let _ = std::process::Command::new("tmux")
        .args(["set-hook", "-t", target_str, &hook_name, &notify_cmd])
        .output();
    let mut fg_hook_guard = FgHookGuard::new(target_str, hook_name.clone());
    let mut fg_rx = bg_done_subscribe();

    let result = match tmux::send_keys(target_str, cmd) {
        Ok(()) => {
            tmux::highlight_pane(target_str, chat_pane);
            let mut switched_to_working = false;
            let mut is_interactive = false;

            if command_has_sudo(cmd) {
                // Unified sudo authentication detection.
                //
                // We determine both *whether* auth is needed and *what kind* in a
                // single loop, rather than two separate stages (needs_password →
                // fingerprint_detection).  The two-stage approach had two failure
                // modes when credentials were cached:
                //
                //   1. A transient `pane_current_command == "sudo"` observation
                //      during a fast cached-credential run could set needs_password,
                //      triggering the fingerprint/password flow when none was needed.
                //
                //   2. The fingerprint detection loop checked pane scrollback with
                //      no concurrent `pane_current_command` guard, so stale "Place
                //      your finger" text from a prior sudo invocation still visible
                //      in the buffer was mistaken for a live fingerprint prompt.
                //
                // The fix: only conclude auth is required when we see an actual
                // prompt in the pane output *while* `pane_current_command` is still
                // "sudo" — a stale scrollback prompt cannot match because sudo has
                // already exited.  For local panes, a single transient "sudo"
                // observation with no accompanying prompt is followed by one
                // confirmation poll; only if "sudo" persists do we conclude it is
                // blocked on input.

                enum SudoAuth {
                    None,
                    Password,
                    Fingerprint,
                }

                let auth = {
                    let mut waited = Duration::ZERO;
                    let mut result = SudoAuth::None;

                    'detect: loop {
                        tokio::time::sleep(SUDO_POLL_INTERVAL).await;
                        waited += SUDO_POLL_INTERVAL;

                        let cur = tmux::pane_current_command(target_str).unwrap_or_default();

                        if cur == "sudo" {
                            // sudo is the foreground process; inspect the pane
                            // output *now* to determine what it is waiting for.
                            // Checking while "sudo" is confirmed current prevents
                            // stale scrollback from triggering a false positive.
                            let snap = tmux::capture_pane(target_str, 10).unwrap_or_default();
                            if is_fingerprint_prompt(&snap) {
                                result = SudoAuth::Fingerprint;
                                break 'detect;
                            }
                            if snap.contains("[sudo]")
                                || snap.contains("password")
                                || snap.contains("Password")
                                || snap.contains("[de-sudo-prompt]")
                            {
                                result = SudoAuth::Password;
                                break 'detect;
                            }
                            // sudo is running but hasn't printed a prompt yet.
                            // Remote panes: sudo stays "sudo" only when blocked on
                            // stdin — treat as password needed.
                            // Local panes: a fast cached-credential run can be
                            // observed transiently as "sudo" — do one confirmation
                            // poll before concluding it is blocked.
                            if is_remote_pane {
                                result = SudoAuth::Password;
                                break 'detect;
                            }
                            tokio::time::sleep(SUDO_POLL_INTERVAL).await;
                            waited += SUDO_POLL_INTERVAL;
                            let cur2 = tmux::pane_current_command(target_str).unwrap_or_default();
                            if cur2 == "sudo" {
                                // Persisted for two consecutive polls: blocked on
                                // input.  Re-check the pane in case the prompt just
                                // rendered between polls.
                                let snap2 = tmux::capture_pane(target_str, 10).unwrap_or_default();
                                if is_fingerprint_prompt(&snap2) {
                                    result = SudoAuth::Fingerprint;
                                } else {
                                    result = SudoAuth::Password;
                                }
                                break 'detect;
                            }
                            // "sudo" transitioned away — credentials were cached.
                        } else if idle_pid != 0
                            && tmux::pane_pid(target_str).unwrap_or(0) == idle_pid
                        {
                            break 'detect;
                        }

                        if waited >= SUDO_DETECT_WINDOW {
                            break 'detect;
                        }
                    }
                    result
                };

                match auth {
                    SudoAuth::None => {}
                    SudoAuth::Fingerprint => {
                        send_response_split(
                            tx,
                            Response::SystemMsg(
                                "sudo is waiting for fingerprint authentication — \
                                 touch the fingerprint reader \
                                 (the target pane is highlighted)"
                                    .to_string(),
                            ),
                        )
                        .await?;
                        // Fall through — command completes via the normal
                        // completion-detection path once the fingerprint is accepted.
                    }
                    SudoAuth::Password => {
                        if is_remote_pane {
                            // Remote pane: can't inject password into a remote pty
                            // reliably; fall back to manual focus switch.
                            send_response_split(
                                tx,
                                Response::SystemMsg(
                                    "sudo password prompt detected — \
                                     switching to your terminal pane. \
                                     Type your password there."
                                        .to_string(),
                                ),
                            )
                            .await?;
                            let _ = tmux::select_pane(target_str);
                            switched_to_working = true;
                        } else {
                            // P2: Prompt in the chat pane (no focus switch).
                            // P3: Retry on wrong password, up to MAX_SUDO_RETRIES.
                            // P6: Track failure reason for structured error reporting.
                            enum SudoFail {
                                Cancelled,
                                AuthExhausted,
                            }
                            let mut sudo_fail: Option<SudoFail> = None;
                            let mut attempt = 0usize;
                            'sudo: while attempt < MAX_SUDO_RETRIES {
                                let prompt = if attempt == 0 {
                                    format!("[sudo] password required for: {}", cmd)
                                } else {
                                    format!(
                                        "sudo: Sorry, try again. \
                                     Password for attempt {}/{}: {}",
                                        attempt + 1,
                                        MAX_SUDO_RETRIES,
                                        cmd
                                    )
                                };
                                send_response_split(
                                    tx,
                                    Response::CredentialPrompt {
                                        id: id.to_string(),
                                        prompt,
                                    },
                                )
                                .await?;
                                let mut cred_line = String::new();
                                let cred = match tokio::time::timeout(
                                    super::USER_PROMPT_TIMEOUT,
                                    rx.read_line(&mut cred_line),
                                )
                                .await
                                {
                                    Ok(Ok(_)) => {
                                        match serde_json::from_str::<Request>(cred_line.trim()) {
                                            Ok(Request::CredentialResponse {
                                                credential, ..
                                            }) => Some(zeroize::Zeroizing::new(credential)),
                                            _ => None,
                                        }
                                    }
                                    _ => None,
                                };
                                zeroize::Zeroize::zeroize(&mut cred_line);
                                let Some(cred) = cred else {
                                    sudo_fail = Some(SudoFail::Cancelled);
                                    break 'sudo;
                                };
                                if !wait_for_sudo_prompt_and_inject(target_str, &cred).await {
                                    break 'sudo; // prompt not found; credentials may be cached
                                }
                                if sudo_auth_failed(target_str).await {
                                    attempt += 1;
                                    continue 'sudo;
                                }
                                break 'sudo; // credential accepted
                            }
                            if attempt >= MAX_SUDO_RETRIES {
                                sudo_fail = Some(SudoFail::AuthExhausted);
                            }

                            // P6: Return a structured error to the AI on sudo failure.
                            if let Some(fail) = sudo_fail {
                                let msg = match fail {
                                    SudoFail::Cancelled => format!(
                                        "sudo timed out waiting for a password — \
                                     `{}` was not executed.\n\
                                     For repeated sudo operations, install a NOPASSWD \
                                     sudoers rule with: \
                                     `daemoneye install-sudoers <script-name>`",
                                        cmd
                                    ),
                                    SudoFail::AuthExhausted => format!(
                                        "sudo authentication failed after {} incorrect password \
                                     attempts — `{}` was not executed.\n\
                                     To avoid password prompts for repeated operations, \
                                     install a NOPASSWD sudoers rule with: \
                                     `daemoneye install-sudoers <script-name>`",
                                        MAX_SUDO_RETRIES, cmd
                                    ),
                                };
                                drop(fg_hook_guard);
                                tmux::unhighlight_pane(target_str, chat_pane);
                                crate::daemon::stats::finish_command(cmd_id, 1);
                                send_response_split(tx, Response::ToolResult(msg.clone())).await?;
                                log_command(
                                    session_id,
                                    "foreground",
                                    target_str,
                                    cmd,
                                    "sudo-failed",
                                    &msg,
                                );
                                return Ok(ToolCallOutcome::Result(msg));
                            }
                        }
                    }
                }
            }

            if is_interactive_command(cmd) {
                is_interactive = true;
                let deadline = tokio::time::Instant::now() + INTERACTIVE_CONNECT_TIMEOUT;
                let mut prompt_found = false;

                'connect: loop {
                    if tokio::time::Instant::now() >= deadline {
                        break;
                    }
                    tokio::select! {
                        result = fg_rx.recv() => {
                            if let Ok(notified_pane) = result
                                && notified_pane == target_str
                                    && let Ok(snap) = tmux::capture_pane(target_str, 20)
                                        && looks_like_shell_prompt(&snap) {
                                            prompt_found = true;
                                            break 'connect;
                                        }
                        }
                        _ = tokio::time::sleep(INTERACTIVE_POLL_INTERVAL) => {
                            if let Ok(snap) = tmux::capture_pane(target_str, 20)
                                && looks_like_shell_prompt(&snap) {
                                    prompt_found = true;
                                    break 'connect;
                                }
                        }
                    }
                }

                if !prompt_found {
                    let stable_deadline = tokio::time::Instant::now() + INTERACTIVE_STABLE_WINDOW;
                    let mut prev = String::new();
                    loop {
                        if tokio::time::Instant::now() >= stable_deadline {
                            break;
                        }
                        tokio::time::sleep(INTERACTIVE_POLL_INTERVAL).await;
                        let snap = tmux::capture_pane(target_str, 20).unwrap_or_default();
                        if snap == prev && !snap.is_empty() {
                            break;
                        }
                        prev = snap;
                    }
                }
            } else if is_remote_pane {
                let mut prev_snap = String::new();
                let mut stable_ticks = 0u32;
                let deadline = tokio::time::Instant::now() + REMOTE_CMD_TIMEOUT;

                loop {
                    if tokio::time::Instant::now() >= deadline {
                        break;
                    }
                    tokio::select! {
                        result = fg_rx.recv() => {
                            if let Ok(notified_pane) = result
                                && notified_pane == target_str { stable_ticks = 0; }
                        }
                        _ = tokio::time::sleep(REMOTE_POLL_INTERVAL) => {
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
                // N9: install monitor-silence + alert-silence as secondary completion signal.
                let silence_hook_name = format!("alert-silence[@de_fg_{}]", hook_idx);
                let _ = std::process::Command::new("tmux")
                    .args([
                        "set-hook",
                        "-t",
                        target_str,
                        &silence_hook_name,
                        &notify_cmd,
                    ])
                    .output();
                let _ = std::process::Command::new("tmux")
                    .args([
                        "set-option",
                        "-t",
                        target_str,
                        "monitor-silence",
                        &SILENCE_MONITOR_SECS.to_string(),
                    ])
                    .output();
                fg_hook_guard.add_silence(silence_hook_name.clone());

                let deadline = tokio::time::Instant::now() + LOCAL_CMD_TIMEOUT;

                // Wait until the child process is visible via a PID change.
                // idle_pid == 0 means the query failed; treat as child-started
                // immediately so we fall through to the hook-based completion wait.
                let saw_child = if idle_pid == 0 {
                    true
                } else {
                    tokio::time::timeout(LOCAL_CHILD_START_WINDOW, async {
                        loop {
                            tokio::time::sleep(LOCAL_CHILD_POLL).await;
                            let cur_pid = tmux::pane_pid(target_str).unwrap_or(0);
                            if cur_pid != idle_pid {
                                break;
                            }
                        }
                    })
                    .await
                    .is_ok()
                };

                if saw_child {
                    loop {
                        if tokio::time::Instant::now() >= deadline {
                            break;
                        }
                        tokio::select! {
                            result = fg_rx.recv() => {
                                if let Ok(notified_pane) = result
                                    && notified_pane == target_str {
                                        let cur_pid = tmux::pane_pid(target_str).unwrap_or(0);
                                        // idle_pid == 0: rely solely on hook signals
                                        if idle_pid != 0 && cur_pid == idle_pid { break; }
                                    }
                            }
                            _ = tokio::time::sleep(LOCAL_SLOW_POLL) => {
                                let cur_pid = tmux::pane_pid(target_str).unwrap_or(0);
                                if idle_pid != 0 && cur_pid == idle_pid { break; }
                            }
                        }
                    }
                }
            }

            drop(fg_hook_guard);
            tokio::time::sleep(POST_CMD_CAPTURE_DELAY).await;
            tmux::unhighlight_pane(target_str, chat_pane);

            let output = match tmux::capture_pane(target_str, 200) {
                Ok(snap) if is_interactive => {
                    let destination = interactive_destination(cmd)
                        .unwrap_or_else(|| "the remote host".to_string());
                    let pane_snap =
                        mask_sensitive(&normalize_output(&extract_command_output(&snap, cmd)));
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
                    let mut out = mask_sensitive(&normalize_output(&extracted));
                    let hints = crate::manifest::related_knowledge_hints(&out);
                    if !hints.is_empty() {
                        out.push('\n');
                        out.push_str(&hints);
                    }
                    out
                }
                Err(_) => "Command sent but could not capture output.".to_string(),
            };

            if switched_to_working && let Some(cp) = chat_pane {
                let _ = tmux::select_pane(cp);
            }

            let exit_code = tmux::read_pane_exit_status(target_str).unwrap_or(0);
            crate::daemon::stats::finish_command(cmd_id, exit_code);
            send_response_split(tx, Response::ToolResult(output.clone())).await?;
            log_command(
                session_id,
                "foreground",
                target_str,
                cmd,
                "approved",
                &output,
            );
            output
        }
        Err(e) => {
            crate::daemon::stats::finish_command(cmd_id, 1);
            let msg = format!("Failed to send command: {}", e);
            log_command(
                session_id,
                "foreground",
                target_str,
                cmd,
                "send-failed",
                &msg,
            );
            msg
        }
    };

    Ok(ToolCallOutcome::Result(result))
}

// ---------------------------------------------------------------------------
// Background command execution
// ---------------------------------------------------------------------------

pub(super) async fn run_background<W, R>(
    id: &str,
    cmd: &str,
    retry_pane: Option<&str>,
    ctx: SessionCtx<'_>,
    ghost_ctx: GhostCtx<'_>,
    tx: &mut W,
    rx: &mut R,
) -> anyhow::Result<ToolCallOutcome>
where
    W: tokio::io::AsyncWriteExt + Unpin,
    R: tokio::io::AsyncBufReadExt + Unpin,
{
    let SessionCtx {
        session_id,
        session_name,
        sessions,
        ..
    } = ctx;
    let GhostCtx {
        policy: ghost_policy,
        is_ghost,
    } = ghost_ctx;
    // N11: retry path — reuse an existing background pane via respawn-pane.
    if let Some(pane_id) = retry_pane {
        if !crate::tmux::pane_exists(pane_id) {
            let msg = format!(
                "Error: retry_in_pane '{}' does not exist. Use background=true without \
                 retry_in_pane to start a fresh background window.",
                pane_id
            );
            send_response_split(tx, Response::ToolResult(msg.clone())).await?;
            return Ok(ToolCallOutcome::Result(msg));
        }
        let win_name: String = {
            let mut name = pane_id.to_string();
            if let Some(sid) = session_id
                && let Ok(store) = sessions.lock()
                && let Some(entry) = store.get(sid)
                && let Some(w) = entry.bg_windows.iter().find(|w| w.pane_id == pane_id)
            {
                name = w.window_name.clone();
            }
            name
        };
        let resolved_retry_cmd;
        let cmd = if let Some(policy) = ghost_policy.as_ref().filter(|_| is_ghost) {
            resolved_retry_cmd = policy.resolve_command(cmd);
            resolved_retry_cmd.as_str()
        } else {
            cmd
        };

        let cmd_id = match prompt_and_await_approval(
            ApprovalRequest {
                id,
                cmd,
                background: true,
                target_pane_hint: None,
            },
            session_id,
            ghost_policy,
            tx,
            rx,
        )
        .await?
        {
            Ok(id) => id,
            Err(outcome) => return Ok(outcome),
        };
        let session_id_owned = session_id.map(|s| s.to_string());
        let output = respawn_background_in_pane(
            pane_id,
            &win_name,
            cmd_id,
            cmd,
            session_name,
            session_id_owned,
            sessions.clone(),
        )
        .await;
        send_response_split(tx, Response::ToolResult(output.clone())).await?;
        log_command(session_id, "background_retry", "", cmd, "approved", &output);
        return Ok(ToolCallOutcome::Result(output));
    }

    // Ghost shells: resolve bare/relative script names to absolute path.
    let resolved_cmd;
    let cmd = if let Some(policy) = ghost_policy.as_ref().filter(|_| is_ghost) {
        resolved_cmd = policy.resolve_command(cmd);
        resolved_cmd.as_str()
    } else {
        cmd
    };

    let cmd_id = match prompt_and_await_approval(
        ApprovalRequest {
            id,
            cmd,
            background: true,
            target_pane_hint: None,
        },
        session_id,
        ghost_policy,
        tx,
        rx,
    )
    .await?
    {
        Ok(id) => id,
        Err(outcome) => return Ok(outcome),
    };

    // Ghost shells: wrap the approved command in `ssh <target> <cmd>` when configured.
    let ssh_wrapped_cmd;
    let cmd = if let Some(policy) = ghost_policy.as_ref().filter(|_| is_ghost) {
        ssh_wrapped_cmd = policy.wrap_remote(cmd);
        ssh_wrapped_cmd.as_str()
    } else {
        cmd
    };

    let credential: Option<zeroize::Zeroizing<String>> = if command_has_sudo(cmd) {
        if is_ghost {
            None
        } else if sudo_credentials_cached().await {
            // Credentials are cached; sudo will not prompt — skip the password flow (P1).
            None
        } else if fingerprint_pam_configured() {
            // Fingerprint auth is configured for sudo.  Background panes have no TTY
            // that the user can interact with, so the fingerprint reader can never be
            // satisfied here.  Fail immediately — before the command is sent and before
            // asking the user for a credential — to avoid leaking the password into the
            // background pane when the fingerprint prompt appears and eventually times
            // out to a password fallback.
            let msg = "sudo requires fingerprint authentication which cannot be satisfied in a \
                 background pane — the fingerprint reader requires a foreground terminal. \
                 Use `daemoneye install-sudoers <script-name>` to create a NOPASSWD rule \
                 for this command, or run it in a foreground pane instead."
                .to_string();
            send_response_split(tx, Response::ToolResult(msg.clone())).await?;
            log_command(
                session_id,
                "background",
                "",
                cmd,
                "fingerprint-rejected",
                &msg,
            );
            return Ok(ToolCallOutcome::Result(msg));
        } else {
            send_response_split(
                tx,
                Response::CredentialPrompt {
                    id: id.to_string(),
                    prompt: format!("[sudo] password required for: {}", cmd),
                },
            )
            .await?;
            let mut cred_line = String::new();
            let result = match tokio::time::timeout(
                super::USER_PROMPT_TIMEOUT,
                rx.read_line(&mut cred_line),
            )
            .await
            {
                Ok(Ok(_)) => match serde_json::from_str::<Request>(cred_line.trim()) {
                    Ok(Request::CredentialResponse { credential, .. }) => {
                        Some(zeroize::Zeroizing::new(credential))
                    }
                    _ => None,
                },
                _ => None,
            };
            zeroize::Zeroize::zeroize(&mut cred_line);
            result
        }
    } else {
        None
    };

    let session_id_owned = session_id.map(|s| s.to_string());
    let output = run_background_in_window(
        session_name,
        id,
        cmd_id,
        cmd,
        credential.as_ref().map(|z| z.as_str()),
        session_id_owned,
        sessions.clone(),
    )
    .await;
    send_response_split(tx, Response::ToolResult(output.clone())).await?;
    log_command(session_id, "background", "", cmd, "approved", &output);
    Ok(ToolCallOutcome::Result(output))
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::{is_shell_prompt, looks_like_shell_prompt};

    #[test]
    fn is_shell_prompt_recognises_common_shells() {
        for sh in &[
            "bash", "zsh", "fish", "sh", "ksh", "csh", "tcsh", "dash", "nu",
        ] {
            assert!(is_shell_prompt(sh), "{sh} should be a shell prompt");
        }
    }

    #[test]
    fn is_shell_prompt_rejects_commands() {
        for cmd in &["top", "vim", "python3", "node"] {
            assert!(!is_shell_prompt(cmd), "{cmd} should not be a shell prompt");
        }
    }

    #[test]
    fn is_shell_prompt_trims_whitespace() {
        assert!(is_shell_prompt("  bash  "));
        assert!(is_shell_prompt("\tzsh\n"));
    }

    #[test]
    fn looks_like_shell_prompt_dollar() {
        assert!(looks_like_shell_prompt("user@host:~$ "));
    }

    #[test]
    fn looks_like_shell_prompt_hash() {
        assert!(looks_like_shell_prompt("root@host:~# "));
    }

    #[test]
    fn looks_like_shell_prompt_percent() {
        assert!(looks_like_shell_prompt("% "));
    }

    #[test]
    fn looks_like_shell_prompt_angle() {
        assert!(looks_like_shell_prompt("> "));
    }

    #[test]
    fn looks_like_shell_prompt_ignores_blank_lines() {
        let snap = "user@host:~$ \n\n";
        assert!(looks_like_shell_prompt(snap));
    }

    #[test]
    fn looks_like_shell_prompt_rejects_mid_output() {
        assert!(!looks_like_shell_prompt("some output line"));
    }

    #[test]
    fn looks_like_shell_prompt_empty_returns_false() {
        assert!(!looks_like_shell_prompt(""));
        assert!(!looks_like_shell_prompt("   \n  "));
    }
}
