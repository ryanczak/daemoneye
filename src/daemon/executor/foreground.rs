use super::find_best_target_pane;
use super::prompt_and_await_approval;
use super::send_response_split;
use super::{ApprovalRequest, GhostCtx, SessionCtx, ToolCallOutcome};
use crate::ai::mask_sensitive;
use crate::daemon::background::{respawn_background_in_pane, run_background_in_window};
use crate::daemon::session::{FG_HOOK_COUNTER, bg_done_subscribe, with_sessions};
use crate::daemon::utils::{
    command_has_sudo, extract_command_output, fingerprint_pam_configured, interactive_destination,
    is_fingerprint_prompt, is_interactive_command, log_command, normalize_output, shell_escape_arg,
    sudo_auth_failed, sudo_credentials_cached, sudo_password_prompt, sudo_sentinel,
    wait_for_sudo_prompt_and_inject, with_sudo_sentinel,
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
const LOCAL_CHILD_START_WINDOW: Duration = Duration::from_millis(750);
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
            let _ = crate::tmux::bounded_output(std::process::Command::new("tmux").args([
                "set-hook",
                "-u",
                "-t",
                &self.target,
                hook,
            ]));
        }
        if self.monitor_silence {
            let _ = crate::tmux::bounded_output(std::process::Command::new("tmux").args([
                "set-option",
                "-u",
                "-t",
                &self.target,
                "monitor-silence",
            ]));
        }
    }
}

// ---------------------------------------------------------------------------
// Shell prompt detection helpers (also used by knowledge::watch_pane).
// ---------------------------------------------------------------------------

/// Shell-name predicate — moved to `crate::tmux::status` (M12 D2), re-exported
/// so the `knowledge::` call sites and the tests below keep their paths.
pub(super) use crate::tmux::status::is_shell_prompt;

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
        ..
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
                .and_then(|sid| {
                    with_sessions(sessions, |store| {
                        store.get(sid)?.default_target_pane.clone()
                    })
                })
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
        let tp_owned = tp.to_string();
        let pane_exists = tmux::off_runtime("pane-exists", move || tmux::pane_exists(&tp_owned))
            .await
            .unwrap_or(false);
        if !pane_exists {
            let correct = session_id
                .and_then(|sid| {
                    with_sessions(sessions, |store| {
                        store.get(sid)?.default_target_pane.clone()
                    })
                })
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
    let default_target: Option<String> = session_id.and_then(|sid| {
        with_sessions(sessions, |store| {
            store.get(sid)?.default_target_pane.clone()
        })
    });
    let target_hint: Option<String> = (|| {
        if let Some(tp) = target
            && chat_pane != Some(tp)
            && cache.is_home_pane(tp)
        {
            return Some(tp.to_string());
        }
        if let Some(ref dtp) = default_target
            && chat_pane != Some(dtp.as_str())
            && cache.is_home_pane(dtp)
        {
            return Some(dtp.clone());
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

    let target_str_pid = target_str.to_string();
    let idle_pid = tmux::off_runtime("pane-pid", move || tmux::pane_pid(&target_str_pid))
        .await
        .and_then(|r| r.ok())
        .unwrap_or(0);
    let t = target_str.to_string();
    let is_remote_pane = crate::tmux::off_runtime("pane-remote-host", move || {
        crate::daemon::utils::get_pane_remote_host(&t)
    })
    .await
    .flatten()
    .is_some();

    // § 2.4 remote execution: when the foreground target is a remote (SSH/mosh) pane and
    // the command invokes a daemon-host script, the bare name does not exist on the remote.
    // Stream the script's content into the pane (hex-decode → interpreter stdin, no remote
    // disk) so it runs there with operator parity. Local panes and non-script commands are
    // sent verbatim.
    let streamed_cmd;
    let send_cmd: &str = if is_remote_pane
        && let Some((name, args)) = crate::scripts::parse_script_invocation(cmd)
    {
        match crate::scripts::read_script(&name) {
            Ok(content) => {
                if command_has_sudo(cmd) {
                    // A streamed stdin script cannot run under sudo on the interactive path:
                    // a NOPASSWD sudoers rule authorizes a fixed path, which streaming does
                    // not provide. Fail loud (no silent doomed send) and point at the ghost
                    // ssh_target mechanism (phase-04), which materializes to that path.
                    let msg = format!(
                        "Error: running daemon-host script '{name}' under sudo on a remote \
                         pane is not supported on the interactive path. Run it without sudo, \
                         or use a Ghost Shell with an ssh_target — that path materializes the \
                         script to a sudoers-authorized location before running it."
                    );
                    crate::daemon::stats::finish_command(cmd_id, 1);
                    send_response_split(tx, Response::ToolResult(msg.clone())).await?;
                    log_command(
                        session_id,
                        "foreground",
                        target_str,
                        cmd,
                        "stream-rejected",
                        &msg,
                    );
                    return Ok(ToolCallOutcome::Result(msg));
                }
                // Default: stream content to the interpreter's stdin — no remote disk.
                streamed_cmd = crate::scripts::remote_stream_cmd(&content, &args);
                streamed_cmd.as_str()
            }
            // Basename did not resolve to a daemon-host script — a normal remote command
            // (e.g. `ls -la`). Send it verbatim.
            Err(_) => cmd,
        }
    } else {
        cmd
    };

    let current_exe =
        std::env::current_exe().unwrap_or_else(|_| std::path::PathBuf::from("daemoneye"));
    let hook_idx = FG_HOOK_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let sudo_sentinel = sudo_sentinel(hook_idx);
    let hook_name = format!("pane-title-changed[@de_fg_{}]", hook_idx);
    let notify_cmd = format!(
        "run-shell -b '{} notify activity {} 0 \"{}\"'",
        current_exe.display(),
        target_str,
        shell_escape_arg(session_name)
    );
    let target_str_hook = target_str.to_string();
    let hook_name_hook = hook_name.clone();
    let notify_cmd_hook = notify_cmd.clone();
    let _ = tmux::off_runtime("set-hook", move || {
        std::process::Command::new("tmux")
            .args([
                "set-hook",
                "-t",
                &target_str_hook,
                &hook_name_hook,
                &notify_cmd_hook,
            ])
            .output()
    })
    .await;
    let mut fg_hook_guard = FgHookGuard::new(target_str, hook_name.clone());
    let mut fg_rx = bg_done_subscribe();

    // Clear the DE_EXIT latch so its reappearance signals THIS command's
    // completion (and carries its real exit code) rather than a stale value from
    // the previous command. No-op for remote/interactive panes (they don't
    // consult it).
    let target_str_clear = target_str.to_string();
    let _ = tmux::off_runtime("clear-pane-exit-status", move || {
        tmux::clear_pane_exit_status(&target_str_clear)
    })
    .await;

    let target_str_keys = target_str.to_string();
    let send_cmd_final = if command_has_sudo(send_cmd) {
        with_sudo_sentinel(send_cmd, &sudo_sentinel)
    } else {
        send_cmd.to_string()
    };
    let send_cmd_keys = send_cmd_final;
    let send_keys_res = tmux::off_runtime("send-keys", move || {
        tmux::send_keys(&target_str_keys, &send_cmd_keys)
    })
    .await;
    let result = match send_keys_res {
        Some(Ok(())) => {
            let target_str_highlight = target_str.to_string();
            let chat_pane_highlight = chat_pane.map(|s| s.to_string());
            let _ = tmux::off_runtime("highlight-pane", move || {
                tmux::highlight_pane(&target_str_highlight, chat_pane_highlight.as_deref())
            })
            .await;
            let mut switched_to_working = false;
            let mut is_interactive = false;
            let mut exit_status: Option<i32> = None;

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
                    let mut last_ka = std::time::Instant::now();

                    'detect: loop {
                        tokio::time::sleep(SUDO_POLL_INTERVAL).await;
                        waited += SUDO_POLL_INTERVAL;
                        crate::daemon::utils::maybe_keepalive(tx, &mut last_ka).await?;

                        let target_str_cur = target_str.to_string();
                        let cur = tmux::off_runtime("pane-current-command", move || {
                            tmux::pane_current_command(&target_str_cur)
                        })
                        .await
                        .and_then(|r| r.ok())
                        .unwrap_or_default();

                        // Every iteration: the nonce'd sentinel appears in the
                        // pane only when this invocation's sudo actually
                        // prompts. Checked regardless of `cur` because remote
                        // panes report `ssh`/`mosh`, never `sudo`.
                        let target_str_snap = target_str.to_string();
                        let snap = tmux::off_runtime("capture-pane", move || {
                            tmux::capture_pane(&target_str_snap, 10)
                        })
                        .await
                        .and_then(|r| r.ok())
                        .unwrap_or_default();
                        if snap.contains(&sudo_sentinel) {
                            result = SudoAuth::Password;
                            break 'detect;
                        }

                        if cur == "sudo" {
                            // PAM fingerprint text cannot be nonce'd, so the
                            // liveness gate stays on `pane_current_command`.
                            if is_fingerprint_prompt(&snap) {
                                result = SudoAuth::Fingerprint;
                                break 'detect;
                            }
                        } else if idle_pid != 0 && {
                            let t = target_str.to_string();
                            tmux::off_runtime("pane-pid", move || tmux::pane_pid(&t))
                                .await
                                .and_then(|r| r.ok())
                                .unwrap_or(0)
                        } == idle_pid
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
                            let t = target_str.to_string();
                            let _ = tmux::off_runtime("select-pane", move || tmux::select_pane(&t))
                                .await;
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
                                let prompt = sudo_password_prompt(attempt, MAX_SUDO_RETRIES);
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
                                if !wait_for_sudo_prompt_and_inject(
                                    target_str,
                                    &cred,
                                    &sudo_sentinel,
                                )
                                .await
                                {
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
                                if matches!(fail, SudoFail::Cancelled) {
                                    // sudo is still sitting at the password prompt — clear it so the
                                    // pane returns to a usable shell rather than a dangling prompt.
                                    let t = target_str.to_string();
                                    let _ = tmux::off_runtime("send-cancel", move || {
                                        crate::tmux::send_cancel(&t)
                                    })
                                    .await;
                                }
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
                                let t = target_str.to_string();
                                let cp = chat_pane.map(|s| s.to_string());
                                let _ = tmux::off_runtime("unhighlight-pane", move || {
                                    tmux::unhighlight_pane(&t, cp.as_deref())
                                })
                                .await;
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
                let mut last_ka = std::time::Instant::now();

                'connect: loop {
                    if tokio::time::Instant::now() >= deadline {
                        break;
                    }
                    crate::daemon::utils::maybe_keepalive(tx, &mut last_ka).await?;
                    tokio::select! {
                        result = fg_rx.recv() => {
                            if let Ok(notified_pane) = result
                                && notified_pane == target_str
                            {
                                let t = target_str.to_string();
                                let snap = tmux::off_runtime("capture-pane", move || {
                                    tmux::capture_pane(&t, 20)
                                })
                                .await
                                .and_then(|r| r.ok());
                                if let Some(s) = snap
                                    && looks_like_shell_prompt(&s)
                                {
                                    prompt_found = true;
                                    break 'connect;
                                }
                            }
                        }
                        _ = tokio::time::sleep(INTERACTIVE_POLL_INTERVAL) => {
                            let t = target_str.to_string();
                            let snap = tmux::off_runtime("capture-pane", move || {
                                tmux::capture_pane(&t, 20)
                            })
                            .await
                            .and_then(|r| r.ok());
                            if let Some(s) = snap
                                && looks_like_shell_prompt(&s)
                            {
                                prompt_found = true;
                                break 'connect;
                            }
                        }
                    }
                }

                if !prompt_found {
                    let stable_deadline = tokio::time::Instant::now() + INTERACTIVE_STABLE_WINDOW;
                    let mut prev = String::new();
                    let mut last_ka = std::time::Instant::now();
                    loop {
                        if tokio::time::Instant::now() >= stable_deadline {
                            break;
                        }
                        tokio::time::sleep(INTERACTIVE_POLL_INTERVAL).await;
                        crate::daemon::utils::maybe_keepalive(tx, &mut last_ka).await?;
                        let t = target_str.to_string();
                        let snap =
                            tmux::off_runtime("capture-pane", move || tmux::capture_pane(&t, 20))
                                .await
                                .and_then(|r| r.ok())
                                .unwrap_or_default();
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
                let mut last_ka = std::time::Instant::now();

                loop {
                    if tokio::time::Instant::now() >= deadline {
                        break;
                    }
                    crate::daemon::utils::maybe_keepalive(tx, &mut last_ka).await?;
                    tokio::select! {
                        result = fg_rx.recv() => {
                            if let Ok(notified_pane) = result
                                && notified_pane == target_str { stable_ticks = 0; }
                        }
                        _ = tokio::time::sleep(REMOTE_POLL_INTERVAL) => {
                            let t = target_str.to_string();
                            let snap = tmux::off_runtime("capture-pane", move || {
                                tmux::capture_pane(&t, 10)
                            })
                            .await
                            .and_then(|r| r.ok())
                            .unwrap_or_default();
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
                let shn = silence_hook_name.clone();
                let th = target_str.to_string();
                let nh = notify_cmd.clone();
                let _ = tmux::off_runtime("set-hook", move || {
                    std::process::Command::new("tmux")
                        .args(["set-hook", "-t", &th, &shn, &nh])
                        .output()
                })
                .await;
                let th2 = target_str.to_string();
                let _ = tmux::off_runtime("set-option", move || {
                    std::process::Command::new("tmux")
                        .args([
                            "set-option",
                            "-t",
                            &th2,
                            "monitor-silence",
                            &SILENCE_MONITOR_SECS.to_string(),
                        ])
                        .output()
                })
                .await;
                fg_hook_guard.add_silence(silence_hook_name.clone());

                let deadline = tokio::time::Instant::now() + LOCAL_CMD_TIMEOUT;
                let mut last_ka = std::time::Instant::now();

                // Phase 1 — within the start window, detect either the child
                // appearing (PID diverges from idle) or a fast command having
                // already finished (the DE_EXIT latch reappeared). The latch is
                // exact regardless of how fast the command was; PID-divergence is
                // the fallback when the shell hook is not installed.
                let mut saw_child = idle_pid == 0;
                if idle_pid != 0 {
                    let start_deadline = tokio::time::Instant::now() + LOCAL_CHILD_START_WINDOW;
                    while tokio::time::Instant::now() < start_deadline
                        && exit_status.is_none()
                        && !saw_child
                    {
                        let t = target_str.to_string();
                        let latch = tmux::off_runtime("read-pane-exit-status", move || {
                            tmux::read_pane_exit_status(&t)
                        })
                        .await
                        .flatten();
                        if let Some(code) = latch {
                            exit_status = Some(code);
                            break;
                        }
                        tokio::time::sleep(LOCAL_CHILD_POLL).await;
                        crate::daemon::utils::maybe_keepalive(tx, &mut last_ka).await?;
                        let t2 = target_str.to_string();
                        let pid = tmux::off_runtime("pane-pid", move || tmux::pane_pid(&t2))
                            .await
                            .and_then(|r| r.ok())
                            .unwrap_or(0);
                        if pid != idle_pid {
                            saw_child = true;
                        }
                    }
                }

                // Phase 2 — only when a child was seen running (a non-trivial
                // command). A command that finished inside the start window is
                // already done: either its latch was read above, or — hook absent —
                // it is captured as-is below (matching the prior fast-path
                // behavior, no false hang). Completion = the DE_EXIT latch (exact,
                // primary) or the child PID returning to idle (fallback). Hook
                // signals (fg_rx) drive promptness.
                if saw_child {
                    while exit_status.is_none() {
                        if tokio::time::Instant::now() >= deadline {
                            break;
                        }
                        crate::daemon::utils::maybe_keepalive(tx, &mut last_ka).await?;
                        let t = target_str.to_string();
                        let latch = tmux::off_runtime("read-pane-exit-status", move || {
                            tmux::read_pane_exit_status(&t)
                        })
                        .await
                        .flatten();
                        if let Some(code) = latch {
                            exit_status = Some(code);
                            break;
                        }
                        tokio::select! {
                            result = fg_rx.recv() => {
                                if let Ok(notified_pane) = result
                                    && notified_pane == target_str {
                                        let t3 = target_str.to_string();
                                        let cur_pid = tmux::off_runtime("pane-pid", move || tmux::pane_pid(&t3))
                                            .await
                                            .and_then(|r| r.ok())
                                            .unwrap_or(0);
                                        if idle_pid != 0 && cur_pid == idle_pid { break; }
                                    }
                            }
                            _ = tokio::time::sleep(LOCAL_SLOW_POLL) => {
                                let t4 = target_str.to_string();
                                let cur_pid = tmux::off_runtime("pane-pid", move || tmux::pane_pid(&t4))
                                    .await
                                    .and_then(|r| r.ok())
                                    .unwrap_or(0);
                                if idle_pid != 0 && cur_pid == idle_pid { break; }
                            }
                        }
                    }
                }
            }

            drop(fg_hook_guard);
            tokio::time::sleep(POST_CMD_CAPTURE_DELAY).await;

            let t = target_str.to_string();
            let cp = chat_pane.map(|s| s.to_string());
            let _ = tmux::off_runtime("unhighlight-pane", move || {
                tmux::unhighlight_pane(&t, cp.as_deref())
            })
            .await;

            let t2 = target_str.to_string();
            let captured = tmux::off_runtime("capture-pane", move || tmux::capture_pane(&t2, 200))
                .await
                .and_then(|r| r.ok());
            let mut output = match captured {
                Some(snap) if is_interactive => {
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
                Some(snap) => {
                    let extracted = extract_command_output(&snap, cmd);
                    let mut out = mask_sensitive(&normalize_output(&extracted));
                    let hints = crate::manifest::related_knowledge_hints(&out);
                    if !hints.is_empty() {
                        out.push('\n');
                        out.push_str(&hints);
                    }
                    out
                }
                None => "Command sent but could not capture output.".to_string(),
            };

            if switched_to_working && let Some(cp) = chat_pane {
                let cp2 = cp.to_string();
                let _ = tmux::off_runtime("select-pane", move || tmux::select_pane(&cp2)).await;
            }

            // Surface the exit status to the model — local pane only. Interactive
            // sessions never "exit"; on a remote pane the shell hook records the
            // ssh wrapper's status, not the remote command's — neither is a
            // meaningful per-command code, so both are left unannotated.
            if !is_interactive
                && !is_remote_pane
                && let Some(note) = exit_status_annotation(exit_status)
            {
                output.push_str(&note);
            }
            crate::daemon::stats::finish_command(cmd_id, exit_status.unwrap_or(0));
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
        Some(Err(e)) => {
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
        None => {
            crate::daemon::stats::finish_command(cmd_id, 1);
            let msg = "Failed to send command: tmux send-keys timed out".to_string();
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
        ..
    } = ghost_ctx;
    // N11: retry path — reuse an existing background pane via respawn-pane.
    if let Some(pane_id) = retry_pane {
        let pid = pane_id.to_string();
        let pane_alive = tmux::off_runtime("pane-exists", move || crate::tmux::pane_exists(&pid))
            .await
            .unwrap_or(false);
        if !pane_alive {
            let msg = format!(
                "Error: retry_in_pane '{}' does not exist. Use background=true without \
                 retry_in_pane to start a fresh background window.",
                pane_id
            );
            send_response_split(tx, Response::ToolResult(msg.clone())).await?;
            return Ok(ToolCallOutcome::Result(msg));
        }
        let win_name: String = session_id
            .and_then(|sid| {
                with_sessions(sessions, |store| {
                    store
                        .get(sid)?
                        .bg_windows
                        .iter()
                        .find(|w| w.pane_id == pane_id)
                        .map(|w| w.window_name.clone())
                })
            })
            .unwrap_or_else(|| pane_id.to_string());
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

    // § 2.4 remote execution: a ghost ssh_target whitelisted-script invocation ships to
    // the remote either by streaming (default, no remote disk) or — under sudo — by a
    // persistent materialize to the sudoers-authorized path.
    let remote_script = ghost_policy
        .as_ref()
        .filter(|_| is_ghost)
        .and_then(|p| p.remote_script_call(cmd)); // Option<(String, String)>
    let remote_script_is_sudo = remote_script.is_some()
        && (crate::daemon::utils::command_has_sudo(cmd)
            || ghost_policy
                .as_ref()
                .map(|p| p.run_with_sudo)
                .unwrap_or(false));

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

    // Ghost shells: build the remote command — stream by default, persist only for sudo.
    let remote_built_cmd;
    let cmd = if let Some((name, args)) = remote_script.as_ref() {
        match crate::scripts::read_script(name) {
            Ok(content) => {
                remote_built_cmd = if remote_script_is_sudo {
                    // Sudo: persistent materialize to the sudoers-authorized path, then
                    // run the resolved `sudo ~/.daemoneye/scripts/<name> …` command.
                    format!(
                        "{} && {}",
                        crate::scripts::remote_materialize_cmd(name, &content),
                        cmd
                    )
                } else {
                    // Default: stream content to the interpreter's stdin — no remote disk.
                    crate::scripts::remote_stream_cmd(&content, args)
                };
                remote_built_cmd.as_str()
            }
            Err(e) => {
                let msg = format!(
                    "Error: cannot run script '{}' on the remote host — it is not \
                     available on the daemon host: {}. Use write_script to create it first.",
                    name, e
                );
                send_response_split(tx, Response::ToolResult(msg.clone())).await?;
                log_command(session_id, "background", "", cmd, "transfer-failed", &msg);
                return Ok(ToolCallOutcome::Result(msg));
            }
        }
    } else {
        cmd
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
                    prompt: sudo_password_prompt(0, MAX_SUDO_RETRIES),
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
// Exit status annotation
// ---------------------------------------------------------------------------

/// Build the trailing annotation appended to a local command's captured output so
/// the model can see a failure. Returns `None` for unknown (`None`, hook absent)
/// and clean (`Some(0)`) — neither is annotated, so a clean or
/// exit-code-unknown command reads exactly as its output. A non-zero code yields
/// a one-line note.
fn exit_status_annotation(exit_status: Option<i32>) -> Option<String> {
    match exit_status {
        Some(code) if code != 0 => Some(format!("\n[command exited with status {code}]")),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::{exit_status_annotation, is_shell_prompt, looks_like_shell_prompt};

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

    #[test]
    fn exit_status_annotation_unknown_is_silent() {
        assert!(exit_status_annotation(None).is_none());
    }

    #[test]
    fn exit_status_annotation_zero_is_silent() {
        assert!(exit_status_annotation(Some(0)).is_none());
    }

    #[test]
    fn exit_status_annotation_nonzero_notes_code() {
        let s = exit_status_annotation(Some(2));
        assert!(s.is_some());
        assert!(s.as_ref().unwrap().contains("2"));
    }
}
