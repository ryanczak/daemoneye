use super::find_best_target_pane;
use super::ghost_host_refusal;
use super::prompt_and_await_approval;
use super::send_response_split;
use super::{ApprovalRequest, GhostCtx, SessionCtx, ToolCallOutcome};
use crate::ai::mask_sensitive;
use crate::daemon::background::run_background_in_window;
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
pub(super) const LOCAL_CMD_TIMEOUT: Duration = Duration::from_secs(45);
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
// Shell prompt detection helpers.
// ---------------------------------------------------------------------------

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
    let FgArgs { id, cmd } = args;
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

    // Compute a best-guess target pane hint synchronously so the approval
    // prompt can show which pane will be used.
    let default_target: Option<String> = session_id.and_then(|sid| {
        with_sessions(sessions, |store| {
            store.get(sid)?.default_target_pane.clone()
        })
    });
    let target_hint: Option<String> = {
        if let Some(ref dtp) = default_target
            && chat_pane != Some(dtp.as_str())
            && cache.is_home_pane(dtp)
        {
            Some(dtp.clone())
        } else {
            None
        }
    };

    let cmd_id = match prompt_and_await_approval(
        ApprovalRequest {
            id,
            cmd,
            background: false,
            target_pane_hint: target_hint.as_deref(),
            host: None,
            user_shell: None,
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
        match find_best_target_pane(chat_pane, cache, sessions, session_id, tx, rx).await {
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
                    send_response_split(
                        tx,
                        Response::ToolResult {
                            tool_call_id: id.to_string(),
                            output: msg.clone(),
                        },
                    )
                    .await?;
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
                                send_response_split(
                                    tx,
                                    Response::ToolResult {
                                        tool_call_id: id.to_string(),
                                        output: msg.clone(),
                                    },
                                )
                                .await?;
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
                         The daemon keeps targeting this pane — run plain \
                         `run_terminal_command(background=false)` calls and \
                         each is injected into the open remote shell.\n\
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
            send_response_split(
                tx,
                Response::ToolResult {
                    tool_call_id: id.to_string(),
                    output: output.clone(),
                },
            )
            .await?;
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
    host: Option<&str>,
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
    // A ghost may name the host a command runs on (`host`), overriding the
    // runbook's `ssh_target` binding: rebind the policy to it, so every
    // gate and route below works as if the runbook had named that host.
    // The `hosts:` fence that follows still applies — now to the named
    // host — with its existing wording and event.
    let rebound;
    let ghost_policy = match (is_ghost, host, ghost_policy) {
        (true, Some(name), Some(policy)) => {
            let refusal = match crate::host::HostId::parse(name) {
                Err(e) => Err(format!("Error: {e}")),
                Ok(h) => policy
                    .for_host(&h, &crate::config::Config::load().unwrap_or_default().hosts)
                    .map_err(|e| format!("Error: {e}")),
            };
            match refusal {
                Ok(p) => {
                    rebound = p;
                    Some(&rebound)
                }
                Err(msg) => {
                    crate::daemon::utils::log_event(
                        "ghost_host_refused",
                        serde_json::json!({
                            "session": session_id.unwrap_or("-"),
                            "tool": "run_terminal_command",
                            "requested_host": name,
                            "allowed_hosts": policy.hosts,
                        }),
                    );
                    send_response_split(
                        tx,
                        Response::ToolResult {
                            tool_call_id: id.to_string(),
                            output: msg.clone(),
                        },
                    )
                    .await?;
                    return Ok(ToolCallOutcome::Result(msg));
                }
            }
        }
        _ => ghost_policy,
    };
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

    // The runbook's `hosts:` list fences `ssh_target` before anything runs:
    // a non-empty list must grant the host the value resolved to, and a
    // value that resolved to no configured host cannot be granted at all.
    if let Some(policy) = ghost_policy.as_ref().filter(|_| is_ghost)
        && !policy.may_use_ssh_target()
    {
        let (requested_host, msg) = match policy.remote_host.as_ref() {
            Some(h) => (h.as_str().to_string(), ghost_host_refusal(policy, h)),
            None => {
                let target = policy.ssh_target.clone().unwrap_or_default();
                let list = policy.hosts.join(", ");
                let msg = format!(
                    "Error: this runbook's ssh_target {target} is not a configured host, \
                     so its `hosts:` list cannot grant it. Hosts this runbook allows: \
                     {list} ({}). Configure a [hosts.<name>] entry whose ssh is {target}, \
                     or work on the daemon host.",
                    policy.hosts.len()
                );
                (target, msg)
            }
        };
        crate::daemon::utils::log_event(
            "ghost_host_refused",
            serde_json::json!({
                "session": session_id.unwrap_or("-"),
                "tool": "run_terminal_command",
                "requested_host": requested_host,
                "allowed_hosts": policy.hosts,
            }),
        );
        send_response_split(
            tx,
            Response::ToolResult {
                tool_call_id: id.to_string(),
                output: msg.clone(),
            },
        )
        .await?;
        return Ok(ToolCallOutcome::Result(msg));
    }

    // The resolved host's `sudo_scripts` gates elevated script runs: a host
    // that declares a whitelist refuses anything it does not name.
    if remote_script_is_sudo
        && let Some(policy) = ghost_policy.as_ref().filter(|_| is_ghost)
        && let Some((name, _)) = remote_script.as_ref()
        && !policy.may_elevate_script(name)
    {
        let host = policy
            .remote_host
            .as_ref()
            .map(|h| h.as_str().to_string())
            .unwrap_or_default();
        let list = policy.remote_sudo_scripts.join(", ");
        let msg = format!(
            "Error: script {name} is not in host {host}'s sudo_scripts ({list}), so it may \
             not run under sudo there. Add it to [hosts.{host}] sudo_scripts, or run it \
             without sudo."
        );
        crate::daemon::utils::log_event(
            "ghost_elevation_refused",
            serde_json::json!({
                "session": session_id.unwrap_or("-"),
                "host": host,
                "script": name,
                "allowed_scripts": policy.remote_sudo_scripts,
            }),
        );
        send_response_split(
            tx,
            Response::ToolResult {
                tool_call_id: id.to_string(),
                output: msg.clone(),
            },
        )
        .await?;
        return Ok(ToolCallOutcome::Result(msg));
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
            host: None,
            user_shell: None,
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
                send_response_split(
                    tx,
                    Response::ToolResult {
                        tool_call_id: id.to_string(),
                        output: msg.clone(),
                    },
                )
                .await?;
                log_command(session_id, "background", "", cmd, "transfer-failed", &msg);
                return Ok(ToolCallOutcome::Result(msg));
            }
        }
    } else {
        cmd
    };

    // 08c: a ghost whose `ssh_target` resolved to a configured host runs the
    // approved command in a PTY shell **on that host** — the same substrate
    // chat uses — synchronously, capped at the ghost turn budget, instead of
    // wrapping it in `ssh <target> <cmd>` in a tmux window. Everything above
    // (the two 08a gates, resolve_command, the approval, the remote-script
    // build) already ran; everything below serves an unresolved ssh_target,
    // a local ghost, and `backend = "tmux"`.
    let uses_pty = crate::config::Config::load()
        .unwrap_or_default()
        .execution
        .uses_pty();
    if let Some(host) = super::ghost_remote::ghost_remote_host(is_ghost, uses_pty, ghost_policy)
        && let crate::host::HostId::Named(name) = &host
    {
        // Connect before any shell spawns on the host — with `BatchMode`,
        // so a host needing a credential fails here instead of hanging the
        // ghost (its transport cannot answer a prompt).
        if let Err(msg) = super::shell_exec::ensure_connected(
            name,
            true,
            id,
            &mut tokio::io::sink(),
            &mut tokio::io::BufReader::new(tokio::io::empty()),
        )
        .await
        {
            let output = format!("Error: {msg}");
            crate::daemon::stats::finish_command(cmd_id, 1);
            send_response_split(
                tx,
                Response::ToolResult {
                    tool_call_id: id.to_string(),
                    output: output.clone(),
                },
            )
            .await?;
            log_command(
                session_id,
                "background",
                name,
                cmd,
                "connect-failed",
                &output,
            );
            return Ok(ToolCallOutcome::Result(output));
        }
        let owner = crate::shell::Owner::Ghost(session_id.unwrap_or_default().to_string());
        let (output, exit_code) = super::ghost_remote::run_ghost_remote(
            crate::shell::registry(),
            owner,
            host.clone(),
            cmd,
            id,
            |shell_id| crate::shell::spawn_ghost_ssh_host(shell_id, name, 24, 80),
        )
        .await;
        crate::daemon::stats::finish_command(cmd_id, exit_code);
        send_response_split(
            tx,
            Response::ToolResult {
                tool_call_id: id.to_string(),
                output: output.clone(),
            },
        )
        .await?;
        log_command(session_id, "background", name, cmd, "approved", &output);
        return Ok(ToolCallOutcome::Result(output));
    }

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
            send_response_split(
                tx,
                Response::ToolResult {
                    tool_call_id: id.to_string(),
                    output: msg.clone(),
                },
            )
            .await?;
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
    send_response_split(
        tx,
        Response::ToolResult {
            tool_call_id: id.to_string(),
            output: output.clone(),
        },
    )
    .await?;
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
pub(super) fn exit_status_annotation(exit_status: Option<i32>) -> Option<String> {
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
    use super::{exit_status_annotation, looks_like_shell_prompt};
    use crate::tmux::status::is_shell_prompt;

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

/// Regression pin for bug-09d-1: `run_background`'s **main** path must still
/// resolve a ghost-whitelisted bare script name to its absolute path (and
/// apply `run_with_sudo`) before execution. Phase-09d deleted the retry
/// block's resolver and took the main-path block with it; the deletion was
/// invisible to every gate because no test exercised this call site.
///
/// Driven at `run_background` itself with ghost auto-approval (no `tx`/`rx`
/// traffic), a real private tmux server pointed at the test's `TMUX_TMPDIR`,
/// and a marker-producing script in the test `$HOME/.daemoneye/scripts/`. The
/// script's bytes land in the pane the moment `send-keys` fires, so the call —
/// which takes the 3 s inline-completion window — is awaited, not polled. The
/// assertions read the audit event and the marker file: both must carry the
/// **absolute** resolved path, and the marker must exist, i.e. the rewritten
/// command reached execution. Scoped to a private tmux server under
/// [`crate::test_home_guard`]; when no tmux binary exists it returns without
/// asserting, exactly like the isolation harness.
#[test]
fn run_background_resolves_ghost_script_paths() {
    if std::process::Command::new("tmux")
        .arg("-V")
        .output()
        .map(|o| !o.status.success())
        .unwrap_or(true)
    {
        eprintln!("SKIP: tmux not available");
        return;
    }
    let _lock = crate::test_home_guard();
    let tmp = tempfile::tempdir().expect("tempdir");
    let saved: Vec<(&str, Option<String>)> = ["HOME", "TMUX_TMPDIR", "TMUX", "TMUX_PANE"]
        .into_iter()
        .map(|k| (k, std::env::var(k).ok()))
        .collect();
    struct Restore(Vec<(&'static str, Option<String>)>);
    impl Drop for Restore {
        fn drop(&mut self) {
            for (k, v) in &self.0 {
                unsafe {
                    match v {
                        Some(val) => std::env::set_var(k, val),
                        None => std::env::remove_var(k),
                    }
                }
            }
        }
    }
    let _restore = Restore(saved);
    unsafe {
        std::env::set_var("HOME", tmp.path());
        std::env::set_var("TMUX_TMPDIR", tmp.path());
        std::env::remove_var("TMUX");
        std::env::remove_var("TMUX_PANE");
    }
    crate::config::Config::ensure_dirs().expect("ensure dirs");

    let marker = tmp.path().join("resolved-marker.txt");
    let script_name = "de-ghost-resolve-check.sh";
    let script_body = format!(
        "#!/bin/sh\nprintf 'RESOLVED-EXEC-CHECK\\n'\ntouch '{}'\n",
        marker.display()
    );
    let script_path = crate::scripts::scripts_dir().join(script_name);
    std::fs::write(&script_path, script_body).expect("write ghost script");
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755))
        .expect("make script executable");

    let sessions = crate::daemon::session::SessionStore::new();
    let session_id = "ghost-resolve-check";

    // The job window needs a session to live in, on the private server.
    let session_name = "de-rb-resolve-check";
    let out = std::process::Command::new("tmux")
        .args(["new-session", "-d", "-s", session_name])
        .output()
        .expect("spawn tmux new-session");
    assert!(
        out.status.success(),
        "creating the private tmux session failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let policy = crate::daemon::policy::GhostPolicy {
        auto_approve_scripts: vec![script_name.to_string()],
        run_with_sudo: false,
        ssh_target: None,
        auto_approve_commands: true,
        hosts: Vec::new(),
        remote_host: None,
        remote_sudo_scripts: Vec::new(),
    };

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build runtime");
    let (text, got_prompt) = rt.block_on(async {
        // The ghost is_safe path reads neither stream, but both ends must
        // exist so no write ever blocks: the client halves are held for the
        // whole call (the final ToolResult write needs the read half live),
        // the read side is a BufReader, and nothing is ever written to it.
        let (_client_tx, daemon_rx) = tokio::io::duplex(64 * 1024);
        let (mut daemon_tx, _client_rx) = tokio::io::duplex(64 * 1024);
        let mut daemon_rx = tokio::io::BufReader::new(daemon_rx);
        let ctx = SessionCtx {
            session_id: Some(session_id),
            session_name,
            chat_pane: None,
            sessions: &sessions,
        };
        let ghost_ctx = GhostCtx {
            policy: Some(&policy),
            is_ghost: true,
        };
        let outcome = run_background(
            "rb_resolve_check",
            script_name,
            None,
            ctx,
            ghost_ctx,
            &mut daemon_tx,
            &mut daemon_rx,
        )
        .await
        .expect("run_background must not error on the ghost path");
        match outcome {
            ToolCallOutcome::Result(t) => (t, false),
            ToolCallOutcome::UserMessage(t) => (format!("[redirected] {t}"), true),
            ToolCallOutcome::SpawnGhostSession { .. } => (String::new(), true),
        }
    });
    assert!(
        !got_prompt,
        "the ghost is_safe path must auto-approve without touching the approval wire"
    );
    assert!(
        !text.contains("Error"),
        "the background dispatch must not fail: {text}"
    );
    // The script completes the moment send-keys lands, even though completion
    // signalling needs the real daemon's notify listener — so on the test
    // binary the call takes the 3 s slow path while the invocation is already
    // on disk.
    assert!(
        marker.exists(),
        "the whitelisted script must have executed via the resolved absolute path"
    );

    // The audit trail must carry the resolved command.
    let resolved = script_path.display().to_string();
    let seg = crate::config::current_event_segment_path();
    let content = std::fs::read_to_string(&seg).expect("read the event segment");
    let has_resolved_record = content.lines().any(|line| {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            return false;
        };
        v.get("event").and_then(|e| e.as_str()) == Some("command")
            && v.get("cmd").and_then(|c| c.as_str()) == Some(resolved.as_str())
    });
    assert!(
        has_resolved_record,
        "the audit log must record the resolved absolute path `{resolved}`, not the \
         bare `{script_name}` — event segment:\n{content}"
    );

    // run_with_sudo leg: the same whitelisted script invoked with an explicit
    // `sudo` prefix is resolved identically — the leading sudo is stripped for
    // whitelist matching and re-applied, so the audit record carries
    // `sudo <absolute path> args`.
    let sudo_policy = crate::daemon::policy::GhostPolicy {
        run_with_sudo: true,
        ..policy.clone()
    };
    let (sudo_text, sudo_got_prompt) = rt.block_on(async {
        let (_client_tx, daemon_rx) = tokio::io::duplex(64 * 1024);
        let (mut daemon_tx, _client_rx) = tokio::io::duplex(64 * 1024);
        let mut daemon_rx = tokio::io::BufReader::new(daemon_rx);
        let ctx = SessionCtx {
            session_id: Some(session_id),
            session_name,
            chat_pane: None,
            sessions: &sessions,
        };
        let ghost_ctx = GhostCtx {
            policy: Some(&sudo_policy),
            is_ghost: true,
        };
        let outcome = run_background(
            "rb_resolve_check_sudo",
            &format!("sudo {script_name} extra-arg"),
            None,
            ctx,
            ghost_ctx,
            &mut daemon_tx,
            &mut daemon_rx,
        )
        .await
        .expect("run_background must not error on the ghost sudo path");
        match outcome {
            ToolCallOutcome::Result(t) => (t, false),
            ToolCallOutcome::UserMessage(t) => (format!("[redirected] {t}"), true),
            ToolCallOutcome::SpawnGhostSession { .. } => (String::new(), true),
        }
    });
    assert!(
        !sudo_got_prompt,
        "a whitelisted sudo-prefixed script must still auto-approve"
    );
    let sudo_resolved = format!("sudo {} extra-arg", script_path.display());
    let content2 = std::fs::read_to_string(&seg).expect("read the event segment");
    let has_sudo_record = content2.lines().any(|line| {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            return false;
        };
        v.get("event").and_then(|e| e.as_str()) == Some("command")
            && v.get("cmd").and_then(|c| c.as_str()) == Some(sudo_resolved.as_str())
    });
    assert!(
        has_sudo_record,
        "the audit log must record the sudo-prefixed resolved path `{sudo_resolved}` — \
         event segment:\n{content2}"
    );
    assert!(
        !sudo_text.contains("Error"),
        "the sudo leg's background dispatch must not fail: {sudo_text}"
    );

    let _ = std::process::Command::new("tmux")
        .args(["kill-server"])
        .output();
}

#[cfg(test)]
mod refusal_gate_tests {
    use super::run_background;
    use crate::daemon::executor::{GhostCtx, SessionCtx, ToolCallOutcome};
    use crate::daemon::session::SessionStore;

    /// Drive `run_background` the way a ghost does and return the tool result
    /// it produced. Unlike `run_background_resolves_ghost_script_paths` this
    /// needs **no tmux and no script on disk**: both gates this row adds
    /// return before the approval prompt and before any tmux call, so the
    /// command is refused without ever reaching the pane. It does need an
    /// isolated `HOME`, because refusals write to the event segment.
    async fn drive_run_background_refusal(
        session_id: &str,
        session_name: &str,
        sessions: &SessionStore,
        policy: &crate::daemon::policy::GhostPolicy,
        cmd: &str,
        host: Option<&str>,
    ) -> String {
        let ctx = SessionCtx {
            session_id: Some(session_id),
            session_name,
            chat_pane: None,
            sessions,
        };
        let ghost_ctx = GhostCtx {
            policy: Some(policy),
            is_ghost: true,
        };
        let (_client_tx, daemon_rx) = tokio::io::duplex(64 * 1024);
        let (mut daemon_tx, _client_rx) = tokio::io::duplex(64 * 1024);
        let mut daemon_rx = tokio::io::BufReader::new(daemon_rx);
        match run_background(
            session_id,
            cmd,
            host,
            ctx,
            ghost_ctx,
            &mut daemon_tx,
            &mut daemon_rx,
        )
        .await
        .expect("run_background must not error on the refusal path")
        {
            ToolCallOutcome::Result(t) => t,
            ToolCallOutcome::UserMessage(t) => format!("[redirected] {t}"),
            ToolCallOutcome::SpawnGhostSession { .. } => String::new(),
        }
    }

    /// Point `HOME` at a fresh temp dir for the duration of the test and
    /// serialise against other HOME-mutating tests (bug-05c-1). The temp dir
    /// is dropped with the returned guard pair, so nothing leaks.
    fn test_home() -> (crate::TestHomeGuard, tempfile::TempDir) {
        let guard = crate::test_home_guard();
        let dir = tempfile::tempdir().expect("tempdir");
        unsafe {
            std::env::set_var("HOME", dir.path());
        }
        crate::config::Config::ensure_dirs().ok();
        (guard, dir)
    }

    /// A ghost on a configured host whose elevated-permission whitelist names
    /// another script may not elevate this one: the call is refused **before**
    /// approval and before any tmux call, naming the script and the allowed
    /// set, and the refusal is recorded. Without the gate this command would
    /// auto-approve and only fail later with "not available on the daemon
    /// host", so the assertion on the exact message is what makes this test
    /// go red when the gate is removed.
    #[test]
    fn an_unlisted_elevated_script_is_refused_on_a_configured_host() {
        let (_home, _dir) = test_home();

        let sessions = SessionStore::new();
        let session_id = "ghost-elevation-refused";
        let policy = crate::daemon::policy::GhostPolicy {
            auto_approve_scripts: vec!["other.sh".to_string()],
            ssh_target: Some("db01".to_string()),
            hosts: Vec::new(),
            remote_host: Some(crate::host::HostId::parse("db01").expect("well-formed host")),
            remote_sudo_scripts: vec!["allowed.sh".to_string()],
            ..crate::daemon::policy::GhostPolicy::from_config(&crate::ipc::GhostConfig::default())
        };

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build runtime");
        let text = rt.block_on(async {
            drive_run_background_refusal(
                session_id,
                "de-elevation-refused",
                &sessions,
                &policy,
                "sudo other.sh",
                None,
            )
            .await
        });

        assert!(
            text.contains("other.sh") && text.contains("allowed.sh"),
            "the refusal must name the refused script and the allowed set: {text}"
        );
        assert!(
            text.contains("db01"),
            "the refusal must name the host: {text}"
        );

        let seg = crate::config::current_event_segment_path();
        let content = std::fs::read_to_string(&seg).expect("read the event segment");
        let refused = content.lines().find_map(|line| {
            let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
                return None;
            };
            (v.get("event").and_then(|e| e.as_str()) == Some("ghost_elevation_refused"))
                .then_some(v)
        });
        assert!(
            refused.is_some(),
            "the elevation refusal must be recorded in the event segment:\n{content}"
        );
    }

    /// A ghost whose `ssh_target` names no configured host cannot be granted
    /// by a non-empty `hosts:` list at all — a list that cannot name what it
    /// allows is not a fence. The refusal names the raw value and the allowed
    /// set, and is recorded with the raw value as `requested_host`.
    #[test]
    fn an_unconfigured_ssh_target_is_outside_a_nonempty_hosts_list() {
        let (_home, _dir) = test_home();

        let sessions = SessionStore::new();
        let session_id = "ghost-unconfigured-target";
        let policy = crate::daemon::policy::GhostPolicy {
            ssh_target: Some("user@far".to_string()),
            hosts: vec!["db01".to_string()],
            remote_host: None,
            remote_sudo_scripts: Vec::new(),
            ..crate::daemon::policy::GhostPolicy::from_config(&crate::ipc::GhostConfig::default())
        };

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build runtime");
        let text = rt.block_on(async {
            drive_run_background_refusal(
                session_id,
                "de-unconfigured-target",
                &sessions,
                &policy,
                "uptime",
                None,
            )
            .await
        });

        assert!(
            text.contains("user@far"),
            "the refusal must name the raw ssh_target: {text}"
        );
        assert!(
            text.contains("db01"),
            "the refusal must name the allowed set: {text}"
        );

        let seg = crate::config::current_event_segment_path();
        let content = std::fs::read_to_string(&seg).expect("read the event segment");
        let refused = content.lines().find_map(|line| {
            let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
                return None;
            };
            (v.get("event").and_then(|e| e.as_str()) == Some("ghost_host_refused")).then_some(v)
        });
        let refused = refused.unwrap_or_else(|| {
            panic!("the host refusal must be recorded in the event segment:\n{content}")
        });
        assert_eq!(
            refused.get("requested_host").and_then(|h| h.as_str()),
            Some("user@far"),
            "an unresolved ssh_target is recorded under its own value, not a host key"
        );
    }

    /// A ghost command naming a host outside its runbook's `hosts:` list is
    /// refused with the same sentence the `ssh_target` fence uses: it names
    /// the refused host and the allowed set.
    #[test]
    fn a_ghost_command_host_outside_its_hosts_list_is_refused() {
        let guard = crate::test_home_guard();
        let dir = tempfile::tempdir().expect("tempdir");
        unsafe {
            std::env::set_var("HOME", dir.path());
        }
        crate::config::Config::ensure_dirs().ok();
        std::fs::write(
            crate::config::etc_dir().join("config.toml"),
            "[hosts.db01]\nssh = \"db01\"\n\n[hosts.db02]\nssh = \"db02\"\n",
        )
        .expect("write config");

        let sessions = SessionStore::new();
        let session_id = "ghost-command-host-unlisted";
        let policy = crate::daemon::policy::GhostPolicy::from_config(&crate::ipc::GhostConfig {
            hosts: vec!["db01".to_string()],
            ..Default::default()
        });

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build runtime");
        let text = rt.block_on(async {
            drive_run_background_refusal(
                session_id,
                "de-command-host-unlisted",
                &sessions,
                &policy,
                "echo hi",
                Some("db02"),
            )
            .await
        });

        assert!(
            text.contains("db02"),
            "the refusal must name the refused host: {text}"
        );
        assert!(
            text.contains("db01"),
            "the refusal must name the allowed set: {text}"
        );
        drop(guard);
    }

    /// A ghost command naming a host that is not configured at all is
    /// refused before the fence: there is nothing to rebind to.
    #[test]
    fn a_ghost_command_host_that_is_not_configured_is_refused() {
        let guard = crate::test_home_guard();
        let dir = tempfile::tempdir().expect("tempdir");
        unsafe {
            std::env::set_var("HOME", dir.path());
        }
        crate::config::Config::ensure_dirs().ok();
        std::fs::write(
            crate::config::etc_dir().join("config.toml"),
            "[hosts.db01]\nssh = \"db01\"\n",
        )
        .expect("write config");

        let sessions = SessionStore::new();
        let session_id = "ghost-command-host-unconfigured";
        let policy = crate::daemon::policy::GhostPolicy::from_config(&crate::ipc::GhostConfig {
            hosts: vec!["db01".to_string()],
            ..Default::default()
        });

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build runtime");
        let text = rt.block_on(async {
            drive_run_background_refusal(
                session_id,
                "de-command-host-unconfigured",
                &sessions,
                &policy,
                "echo hi",
                Some("nope"),
            )
            .await
        });

        assert!(
            text.contains("nope"),
            "the refusal must name the unknown host: {text}"
        );
        drop(guard);
    }
}
