//! Running a chat turn's `run_terminal_command` on the PTY shell substrate,
//! and formatting what it produced for a model.
//!
//! Sequence for one command (F3): annotate ANSI first — it is the sanitiser as
//! well as the annotator, dropping control bytes a mid-token escape could use
//! to split a secret past its regex — then mask, then normalize.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::time::{Duration, Instant};

use super::send_response_split;
use crate::ai::mask_sensitive;
use crate::daemon::utils::{
    PromptKind, classify_prompt, classify_ssh_prompt, command_has_sudo, is_ssh_permission_denied,
    normalize_output, sudo_password_prompt, sudo_sentinel, with_pty_prompt_sentinel,
};
use crate::host::connect_argv;
use crate::ipc::{Request, Response};
use crate::shell::PromptReply;
use crate::shell::{
    CommandOutcome, Foreground, MetaIndex, Owner, PROBE_TIMEOUT, ShellId, ShellRegistry,
    await_ready, host_entry_or_bare, marker_shell_name, master_running, meta_path_for, now_unix,
    probe_foreground, run_command, run_command_with_prompts, shell_for_owner, socket_path,
    spawn_for_on_host, which_ssh, with_shells,
};
use crate::shell::{CommandRecord, marked_inner_range};
use std::sync::atomic::AtomicUsize;

/// Per-invocation sudo sentinel source for the PTY path: a fresh index per
/// command is what makes the exact-match property meaningful.
static PTY_SENTINEL_COUNTER: AtomicUsize = AtomicUsize::new(0);

/// How long a fresh host may take to come up and answer `Status`.
pub(super) const SHELL_READY_TIMEOUT: Duration = Duration::from_secs(15);

/// How long the producer waits for the command's bytes to land in the cast
/// file before giving up on indexing. The host broadcasts a chunk to
/// subscribers before writing it to the cast, so `run_command` may return
/// while the END marker is still only in flight.
const CAST_SETTLE_TIMEOUT: Duration = Duration::from_secs(2);
/// How often the settle loop re-reads the cast file length between polls.
const CAST_SETTLE_POLL: Duration = Duration::from_millis(10);

/// How long `launch_interactive` waits after writing the command before it
/// reads the pre-draw baseline. The echo of the command must be on the screen
/// by then, or the baseline would miss the very bytes the change wait is
/// meant to get past.
const BASELINE_GAP: Duration = Duration::from_millis(100);
/// How long the launch's change wait will wait for the program to draw
/// something before giving up (F1): the launch still returns the settled
/// screen, but the result says the program has drawn nothing yet.
const CHANGE_WAIT_TIMEOUT: Duration = Duration::from_secs(3);
/// The poll interval between two screen reads inside the launch's change
/// wait.
const CHANGE_WAIT_POLL: Duration = Duration::from_millis(100);

/// Default wall-clock cap on one command run in a PTY shell, when the caller
/// gives no `timeout_secs`.
pub(super) const LOCAL_CMD_TIMEOUT: Duration = Duration::from_secs(45);

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

/// Annotate, mask, then normalize. The order matters (F3): the annotation step
/// must strip control bytes before masking, or an escape embedded mid-token
/// could split a secret past its regex.
pub(super) fn format_for_model(raw: &str) -> String {
    mask_sensitive(&normalize_output(&crate::process::annotate_output(raw)))
}

/// The context a command's failure carries when it reaches `map_err`: a
/// `ShellExited` in the chain says the shell died, everything else reads as
/// the command overrunning `applied_timeout`. Named so the mapping is
/// testable without a socket.
pub(super) fn map_run_command_error(
    e: anyhow::Error,
    applied_timeout: Duration,
    shell: &str,
) -> anyhow::Error {
    if e.chain().any(|c| c.is::<crate::shell::ShellExited>()) {
        e.context(format!("shell {shell} exited before the command finished"))
    } else {
        e.context(format!(
            "command exceeded the {applied_timeout:#?} limit in shell {shell}"
        ))
    }
}

/// What one command produced, already fit for a model.
#[derive(Debug)]
pub(crate) struct ShellRunResult {
    /// Annotated, masked, normalized. Ready to send as the tool result.
    pub text: String,
    pub exit_code: i32,
}

/// The hard ceiling on `run_terminal_command`'s `timeout_secs`, the same cap
/// `watch_shell` applies. 45 s is the default; the cap exists so a stuck
/// command cannot pin a chat turn for an unbounded time.
pub(super) const RUN_MAX_TIMEOUT_SECS: u64 = 3600;

/// The wait one command is given: the caller's `timeout_secs` when given and
/// non-zero, else `LOCAL_CMD_TIMEOUT`, always clamped to `RUN_MAX_TIMEOUT_SECS`.
fn applied_timeout_secs(timeout_secs: Option<u64>) -> Duration {
    let secs = timeout_secs
        .filter(|s| *s > 0)
        .unwrap_or(LOCAL_CMD_TIMEOUT.as_secs())
        .min(RUN_MAX_TIMEOUT_SECS);
    Duration::from_secs(secs)
}

/// The entry point `executor/mod.rs` calls with a pub/sub transport. `cmd` is
/// the plain command: whether it needs a sudo prompt and which sentinel it
/// gets is decided here, and the non-sudo result is byte-identical to the
/// historical no-transport run (`prompts: None`).
///
/// The prompt round-trips `Response::CredentialPrompt` →
/// `Request::CredentialResponse` over the caller's transport, exactly as the
/// tmux path does.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_in_shell_with_prompts<'p, W, R, F, Fut>(
    reg: &ShellRegistry,
    owner: Owner,
    cmd: &str,
    max_per_owner: u32,
    shell: Option<&str>,
    timeout_secs: Option<u64>,
    prompt_id: &str,
    tx: &'p mut W,
    rx: &'p mut R,
    host: crate::host::HostId,
    spawn: F,
) -> anyhow::Result<(ShellId, ShellRunResult)>
where
    W: tokio::io::AsyncWriteExt + Unpin + 'p,
    R: tokio::io::AsyncBufReadExt + Unpin + 'p,
    F: FnOnce(ShellId) -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<()>>,
{
    let has_sudo = command_has_sudo(cmd);
    let prompts = if has_sudo {
        Some(SudoPromptArgs {
            wrapped: cmd.to_string(),
            id: prompt_id.to_string(),
            tx,
            rx,
        })
    } else {
        None
    };
    run_in_shell_core::<W, R, F, Fut>(
        reg,
        owner,
        cmd,
        max_per_owner,
        shell,
        timeout_secs,
        false,
        prompts,
        host,
        spawn,
    )
    .await
}

/// The launch path (03e) `run_terminal_command(interactive: true)` takes:
/// no prompts and no timeout — the bare command is typed into the shell, the
/// call waits only for the screen to settle (D6), and `run_in_shell_core`
/// marks the run interactive so a non-launching command also takes this door.
pub(crate) async fn run_interactive_in_shell<S, Fut>(
    reg: &ShellRegistry,
    owner: Owner,
    cmd: &str,
    max_per_owner: u32,
    shell: Option<&str>,
    host: crate::host::HostId,
    spawn: S,
) -> anyhow::Result<(ShellId, ShellRunResult)>
where
    S: FnOnce(ShellId) -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<()>>,
{
    run_in_shell_core(
        reg,
        owner,
        cmd,
        max_per_owner,
        shell,
        None,
        true,
        None::<SudoPromptArgs<'_, tokio::io::Sink, tokio::io::Empty>>,
        host,
        spawn,
    )
    .await
}

/// A sudo command's prompt plumbing. `wrapped` is the command **as given**:
/// the sentinel is derived and inserted once, inside `run_sudo_command`, so
/// the typed line carries exactly one `SUDO_PROMPT` assignment.
pub(super) struct SudoPromptArgs<'p, W, R>
where
    W: tokio::io::AsyncWriteExt + Unpin + 'p,
    R: tokio::io::AsyncBufReadExt + Unpin + 'p,
{
    wrapped: String,
    id: String,
    tx: &'p mut W,
    rx: &'p mut R,
}

/// What `shell?` on `run_terminal_command` resolves to, before the registry
/// is touched. Pure — no await, no lock, no filesystem — so a bad value is
/// rejected before anything runs.
#[derive(Debug)]
pub(super) enum ShellTarget {
    Reuse,
    New,
    Existing(ShellId),
}

pub(super) fn resolve_shell_target(arg: Option<&str>) -> Result<ShellTarget, String> {
    match arg {
        None => Ok(ShellTarget::Reuse),
        Some("new") => Ok(ShellTarget::New),
        Some(value) => match ShellId::parse(value) {
            Ok(id) => Ok(ShellTarget::Existing(id)),
            Err(_) => Err(format!(
                "Error: unknown shell target {value:?}. Accepted forms: \
                 omit `shell` to reuse this session's lowest-numbered open \
                 shell (the default), \"new\" to open a fresh shell for this \
                 command, or a shell id from list_shells such as \"s3\"."
            )),
        },
    }
}

/// The ids the unknown-shell sentences name, kept here so the refusal wording
/// cannot drift between the read tools and this module's target refusals.
fn live_shell_ids(reg: &ShellRegistry) -> Vec<String> {
    with_shells(reg, |store| {
        let mut ids: Vec<String> = store.values().map(|r| r.id.as_str().to_string()).collect();
        ids.sort_by_key(|i| ShellId::parse(i).map(|s| s.index()).unwrap_or(u32::MAX));
        ids
    })
}

/// Wrap a shell-resolution failure in `RefusedBeforeWrite`: the per-owner
/// cap refusal (F7) is raised by `registry.rs` before anything is written,
/// exactly as the busy refusal is, so it must not reach the model wrapped in
/// the generic "may still be running / double-apply" run-error caveat.
fn pre_write(e: anyhow::Error) -> anyhow::Error {
    anyhow::Error::new(RefusedBeforeWrite).context(e)
}

/// The single body of the run path. `prompts: None` is the historical
/// behaviour; a transport turns the sudo branch on.
#[allow(clippy::too_many_arguments)]
async fn run_in_shell_core<'p, W, R, F, Fut>(
    reg: &ShellRegistry,
    owner: Owner,
    cmd: &str,
    max_per_owner: u32,
    shell: Option<&str>,
    timeout_secs: Option<u64>,
    interactive: bool,
    prompts: Option<SudoPromptArgs<'p, W, R>>,
    host: crate::host::HostId,
    spawn: F,
) -> anyhow::Result<(ShellId, ShellRunResult)>
where
    W: tokio::io::AsyncWriteExt + Unpin + 'p,
    R: tokio::io::AsyncBufReadExt + Unpin + 'p,
    F: FnOnce(ShellId) -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<()>>,
{
    // A named host is resolved against `[hosts]` **before any spawn**, with
    // the permissive lookup (`host_entry_or_bare`): an unconfigured host is
    // allowed as its own ssh destination, behind the approval the user has
    // already given. `HostId::parse`'s malformed-name branch is checked by the
    // caller (`mod.rs`), so only the configuration question is answered here.
    if let crate::host::HostId::Named(name) = &host {
        crate::shell::host_entry_or_bare(name)?;
    }
    let target = resolve_shell_target(shell).map_err(anyhow::Error::msg)?;
    let applied_timeout = applied_timeout_secs(timeout_secs);
    // Resolve the shell socket from the requested target before anything is
    // written to a PTY: `Existing` must refuse an id the caller does not own
    // or that has exited, rather than falling back to the reuse shell —
    // falling back would run the command somewhere the model did not ask for.
    let (id, socket) = match target {
        ShellTarget::Reuse => {
            let (id, socket) =
                shell_for_owner(reg, owner, host, max_per_owner, SHELL_READY_TIMEOUT, spawn)
                    .await
                    .map_err(pre_write)?;
            (id, socket)
        }
        ShellTarget::New => {
            let id = spawn_for_on_host(reg, owner, max_per_owner, host, false, spawn)
                .await
                .map_err(pre_write)?;
            let socket = socket_path(id.as_str());
            await_ready(&socket, Instant::now() + SHELL_READY_TIMEOUT).await?;
            (id, socket)
        }
        ShellTarget::Existing(ref id) => {
            // Compared on `Owner::key()`, never the inner string: a chat
            // session and a ghost job can share an id string, and the kinds
            // are what the cap groups, so they are what this check must use.
            let record_owner = with_shells(reg, |store| {
                store.get(id.as_str()).map(|record| record.owner.key())
            });
            let Some(record_owner) = record_owner else {
                let live = live_shell_ids(reg);
                anyhow::bail!(
                    "Error: shell {} is not in the registry — it has exited or was \
                     closed. Live shell ids: {}. Call list_shells to refresh.",
                    id.as_str(),
                    live.join(", ")
                );
            };
            if !owner.may_drive(&record_owner) {
                anyhow::bail!(
                    "Error: shell {} is not yours (it belongs to {record_owner}), so \
                     this session cannot run a command in it. Run the command in one \
                     of your own shells — omit `shell`, or pass \"new\" — and leave \
                     {} to the session that owns it.",
                    id.as_str(),
                    id.as_str()
                );
            }
            // A dedicated shell (a background run's own PTY) is deliberately
            // unreachable: it exists for exactly one command, and reaching it
            // from the foreground would interleave in its output and break
            // `watch_shell`'s contract.
            let dedicated = with_shells(reg, |store| {
                store.get(id.as_str()).map(|record| record.dedicated)
            });
            if dedicated == Some(true) {
                anyhow::bail!(
                    "Error: shell {} is a dedicated background shell — it exists for \
                     exactly one background command and is not a target for \
                     run_terminal_command. Watch it with watch_shell, read it with \
                     read_shell, or target a shell of your own.",
                    id.as_str()
                );
            }
            if with_shells(reg, |store| {
                store
                    .get(id.as_str())
                    .and_then(|record| record.exited_unix)
                    .is_some()
            }) {
                let live = live_shell_ids(reg);
                anyhow::bail!(
                    "Error: shell {} has exited. Live shell ids: {}. Call list_shells \
                     to refresh.",
                    id.as_str(),
                    live.join(", ")
                );
            }
            // The caller named a host and the shell runs on another one: the
            // command would run somewhere the model did not ask for, the same
            // failure the ownership check above exists to prevent. The record's
            // host is in hand; refuse here, naming both, rather than relocate.
            // With no `host` named the comparison is skipped — `shell: "s3"`
            // alone still runs wherever s3 is, including a remote shell.
            if let crate::host::HostId::Named(requested) = &host {
                let record_host = with_shells(reg, |store| {
                    store.get(id.as_str()).map(|record| record.host.clone())
                });
                if record_host.as_ref() != Some(&host) {
                    let actual = record_host
                        .as_ref()
                        .map(|h| h.as_str().to_string())
                        .unwrap_or_else(|| "local".to_string());
                    anyhow::bail!(
                        "Error: shell {} runs on host {actual}, not {requested}. A \
                         command is never relocated to a different shell's host — \
                         pass host: \"{actual}\" to run it there, omit `shell` to \
                         reuse this session's lowest-numbered open shell on \
                         {requested}, or pass shell: \"new\" for a fresh shell on \
                         {requested}.",
                        id.as_str(),
                    );
                }
            }
            (id.clone(), socket_path(id.as_str()))
        }
    };
    // A shell the caller named is probed live before anything is written to
    // it (D5): a command written into a busy shell would be typed into
    // whatever program holds it. A shell that was just interrupted can read
    // `Running` for a moment, so a non-prompt answer is re-probed twice,
    // 150 ms apart, before the refusal stands. Nothing has been written yet,
    // so the refusal leaves the shell exactly as it was.
    if let ShellTarget::Existing(ref id) = target {
        const RECHECKS: usize = 2;
        const RECHECK_GAP: Duration = Duration::from_millis(150);
        let mut fg = probe_foreground(reg, id, PROBE_TIMEOUT).await;
        for _ in 0..RECHECKS {
            if matches!(fg, Foreground::Prompt) {
                break;
            }
            tokio::time::sleep(RECHECK_GAP).await;
            fg = probe_foreground(reg, id, PROBE_TIMEOUT).await;
        }
        if !matches!(fg, Foreground::Prompt) {
            // D5's wording. For `Unknown` the middle clause names the
            // unreadable state instead of a program, because nothing asserts
            // what holds the terminal.
            let reason = match fg {
                Foreground::Unknown => "the shell's state could not be read".to_string(),
                _ => "a command written there would be typed into it".to_string(),
            };
            // The refusal happened before anything was written, so it must
            // not arrive wrapped in the generic run-error text's "may still
            // be running / double-apply" caveat — that contradicts the
            // refusal. `RefusedBeforeWrite` in the chain is what
            // `shell_run_error_text` keys on (F3).
            let msg = format!(
                "Error: shell {} is {fg}; {reason}. Use send_input to type into \
                 that program, read_shell_screen to see it, or shell: \"new\" for \
                 a fresh shell.",
                id.as_str()
            );
            return Err(anyhow::Error::new(RefusedBeforeWrite).context(msg));
        }
    }
    if interactive || crate::daemon::utils::is_interactive_command(cmd) {
        // The command is launched, not run to completion: the bare command
        // plus `\r` starts the session and the settled screen is what comes
        // back (D6). No markers, no exit wait, no signal to the foreground
        // group — the shell stays busy holding the session, so the reuse
        // rule hands later commands to another shell.
        let result = launch_interactive(reg, &id, &socket, cmd).await?;
        return Ok((id, result));
    }
    let (started_unix, record_host, marker) = with_shells(reg, |store| {
        store
            .get(id.as_str())
            .map(|record| {
                (
                    record.started_unix,
                    record.host.clone(),
                    crate::shell::shell_marker_name(record),
                )
            })
            .unwrap_or_else(|| {
                (
                    0,
                    crate::host::HostId::Local,
                    marker_shell_name(&crate::host::HostId::Local),
                )
            })
    });
    // The cast length BEFORE the command runs — the producer window's left
    // edge. Read here, not inside the indexing step: by the time a completed
    // command is in hand the pump thread has usually flushed the whole
    // command into the cast, so a length read any later is past the command
    // and the window is empty.
    let before = std::fs::metadata(crate::shell::resolve_cast(id.as_str(), started_unix))
        .map(|m| m.len())
        .ok();

    // `in_flight` spans the whole run — set now, cleared by the guard's Drop
    // on every way the run ends (outcome, timeout, prompt given up, shell
    // exited, error), so an early `?` cannot leave the mark behind.
    let _in_flight = crate::shell::InFlightGuard::new(reg, id.clone(), cmd, now_unix());

    // A command without sudo takes the plain path.
    let Some(prompts) = prompts else {
        let outcome = run_command(&socket, cmd, &marker, applied_timeout)
            .await
            .map_err(|e| map_run_command_error(e, applied_timeout, id.as_str()))?;
        return finish_run(reg, cmd, &id, &outcome, before).await;
    };

    let wrapped = prompts.wrapped.clone();
    let outcome = run_sudo_command(
        &socket,
        &wrapped,
        &marker,
        applied_timeout,
        prompts,
        record_host.as_str(),
    )
    .await;
    match outcome {
        Ok(outcome) => finish_run(reg, cmd, &id, &outcome, before).await,
        Err(e) => {
            // The give-up reason must reach the AI: surface it through the same
            // structured error the command's own timeout takes.
            let ctx = if e.chain().any(|c| c.is::<crate::shell::PromptGivenUp>()) {
                format!("the password prompt in shell {} was cancelled", id.as_str())
            } else if e.chain().any(|c| c.is::<crate::shell::ShellExited>()) {
                format!("shell {} exited before the command finished", id.as_str())
            } else {
                format!(
                    "sudo in shell {} did not complete within the {applied_timeout:#?} limit",
                    id.as_str()
                )
            };
            Err(e.context(ctx))
        }
    }
}

/// Start an interactive command in `id`'s shell and return the settled screen
/// without waiting for the session to end (D6, and F1 from rehearsal 1): the
/// bare command plus `\r`, no marker wrapping, then a change wait — the
/// launch must not call a screen settled while it still shows only what was
/// there before the program started — followed by
/// `read_shell_screen_in`'s settle-and-read, which records the shell's
/// `last_foreground`. The shell stays busy while the session runs — the
/// reuse rule and the named-shell probe see it through `last_foreground`, so
/// later commands land elsewhere.
async fn launch_interactive(
    reg: &ShellRegistry,
    id: &ShellId,
    sock: &Path,
    cmd: &str,
) -> anyhow::Result<ShellRunResult> {
    let bytes: &[u8] = &[cmd.as_bytes(), b"\r"].concat();
    crate::shell::send_input(sock, bytes, Duration::from_secs(5)).await?;
    // The echo of the command is on the screen within ~100 ms, so a read now
    // is the pre-draw baseline: anything the program draws later differs from
    // it. If the read fails (a host that went away) skip the change wait and
    // let the settled read surface the failure.
    tokio::time::sleep(BASELINE_GAP).await;
    let baseline = super::knowledge::screen_text(id.as_str(), super::knowledge::SCREEN_TIMEOUT)
        .await
        .ok();
    let drew = match baseline.as_deref() {
        Some(baseline) => {
            let shell = id.as_str().to_string();
            let read = || {
                let shell = shell.clone();
                async move {
                    super::knowledge::screen_text(&shell, super::knowledge::SCREEN_TIMEOUT).await
                }
            };
            super::knowledge::wait_screen_change(
                read,
                baseline,
                CHANGE_WAIT_TIMEOUT,
                CHANGE_WAIT_POLL,
            )
            .await
        }
        None => false,
    };
    let screen = super::knowledge::read_shell_screen_in(
        reg,
        id.as_str(),
        None,
        None,
        None,
        false,
        None,
        Some(500),
        Some(5),
        super::knowledge::SCREEN_TIMEOUT,
    )
    .await;
    // One line, after the header, when three seconds passed and the program
    // has drawn nothing: the settled screen would otherwise read as if the
    // launch had seen the program's own display.
    let screen = if drew {
        screen
    } else {
        format!(
            "{screen}\n[the program has not drawn anything yet; read it again with \
             read_shell_screen]"
        )
    };
    Ok(ShellRunResult {
        text: format!(
            "[Interactive session started] `{cmd}` holds shell {}; nothing waits \
             for it. Type into it with send_input(shell: \"{}\") and read it \
             with read_shell_screen(shell: \"{}\"). Later commands run in \
             another shell while it is busy.\n{screen}",
            id.as_str(),
            id.as_str(),
            id.as_str(),
        ),
        exit_code: 0,
    })
}

/// The non-sudo epilogue: index, format, annotate. Shared by the plain and the
/// sudo-aware path so the two can never diverge on what the model sees.
///
/// Reached only with a marker-terminated `CommandOutcome` — the plain path
/// and the sudo path both end here — so the shell is back at its prompt, and
/// the record says so: `last_foreground` is stamped `Prompt` exactly as
/// `probe_foreground` would have. Nothing is stored when the id is not in
/// the registry (it exited under the command and was swept).
async fn finish_run(
    reg: &ShellRegistry,
    cmd: &str,
    id: &ShellId,
    outcome: &CommandOutcome,
    before: Option<u64>,
) -> anyhow::Result<(ShellId, ShellRunResult)> {
    with_shells(reg, |store| {
        if let Some(record) = store.get_mut(id.as_str()) {
            record.last_foreground = Some((Foreground::Prompt, now_unix()));
        }
    });
    index_command_output(reg, cmd, id, outcome, before).await;
    let raw = String::from_utf8_lossy(&outcome.output);
    let mut text = format_for_model(&raw);
    if let Some(note) = exit_status_annotation(Some(outcome.exit_code)) {
        text.push_str(&note);
    }
    Ok((
        id.clone(),
        ShellRunResult {
            text,
            exit_code: outcome.exit_code,
        },
    ))
}

/// This command was refused **before anything was written** to the shell: the
/// busy-shell probe saw a program holding the terminal, so writing would have
/// typed into it. Distinct from a timeout on purpose — the command did not
/// fail, it never started, and the shell is exactly as it was — so
/// `shell_run_error_text` must not wrap it in the "may still be running /
/// double-apply" caveat that contradicts the refusal (F3).
#[derive(Debug)]
pub struct RefusedBeforeWrite;

impl std::fmt::Display for RefusedBeforeWrite {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "the command was refused before anything was written")
    }
}

impl std::error::Error for RefusedBeforeWrite {}

#[path = "shell_exec_auth.rs"]
mod auth;
// The glob brings in `run_sudo_command` and, for the tests, `drive_connect`.
pub(super) use auth::ensure_connected;
use auth::*;
// `auth` reaches this as `super::USER_PROMPT_TIMEOUT`, through this import.
use super::USER_PROMPT_TIMEOUT;

/// Record one completed command in the shell's `.meta.json` index, so
/// `read_shell` / `find_in_shells` return what the user would have seen.
/// Everything here is best-effort: the index is derived data, exactly like the
/// memory FTS index, and a failure must never change what the command returns
/// to the model.
async fn index_command_output(
    reg: &ShellRegistry,
    cmd: &str,
    id: &ShellId,
    outcome: &CommandOutcome,
    before: Option<u64>,
) {
    let (started_unix, owner, host) = with_shells(reg, |store| {
        store
            .get(id.as_str())
            .map(|record| {
                (
                    record.started_unix,
                    Some(record.owner.key()),
                    Some(record.host.as_str().to_string()),
                )
            })
            .unwrap_or((0, None, None))
    });
    let cast = crate::shell::resolve_cast(id.as_str(), started_unix);
    // `before` was snapshot in `run_in_shell_with`, before the command ran.
    // A missing cast then is a shell with no recording at all — nothing to
    // index into.
    let Some(before) = before else {
        log::warn!(
            "meta index: no cast file before the command in shell {}; not indexed: {cmd}",
            id.as_str()
        );
        return;
    };
    // The host broadcasts a chunk before it writes that chunk to the cast
    // (backend.rs pump thread), so `run_command` can return before the
    // command's bytes are on disk. Wait for the END marker rather than
    // assuming the file has caught up.
    let deadline = tokio::time::Instant::now() + CAST_SETTLE_TIMEOUT;
    let range = loop {
        let after = match std::fs::metadata(&cast) {
            Ok(m) => m.len(),
            Err(e) => {
                log::warn!("meta index: cast {} vanished mid-read: {e}", cast.display());
                return;
            }
        };
        if after > before
            && let Ok(slice) = read_cast_slice(&cast, before, after)
            && let Some((first, end)) = marked_inner_range(&slice, &outcome.nonce)
        {
            break Some((before + first, before + end));
        }
        if tokio::time::Instant::now() >= deadline {
            break None;
        }
        tokio::time::sleep(CAST_SETTLE_POLL).await;
    };
    let Some((first_byte, end_byte)) = range else {
        log::warn!(
            "meta index: markers for shell {} command `{cmd}` never landed in {} \
             within {CAST_SETTLE_TIMEOUT:?}; not indexed",
            id.as_str(),
            cast.display()
        );
        return;
    };
    let record = CommandRecord {
        index: 0,
        command: cmd.to_owned(),
        started: before as f64,
        ended: after_cast_len(&cast, before) as f64,
        exit_code: outcome.exit_code,
        first_byte,
        end_byte,
    };
    let meta_path = meta_path_for(&cast);
    if let Err(e) = MetaIndex::append(
        &meta_path,
        &cast,
        id.as_str(),
        started_unix,
        owner.as_deref(),
        host.as_deref(),
        record,
    ) {
        log::warn!(
            "meta index: appending to {} failed, command output is unaffected: {e}",
            meta_path.display()
        );
    }
}

/// The cast length at the time the record closes, so `ended` lands after the
/// command's final write. Best-effort: a vanished cast yields `before`.
fn after_cast_len(cast: &Path, before: u64) -> u64 {
    std::fs::metadata(cast)
        .map(|m| m.len())
        .unwrap_or(before)
        .max(before)
}

/// The `[before, after)` bytes of `cast`, read back after the command ran.
/// Reads fail loudly for the caller to log; the command's own result is not at
/// stake.
fn read_cast_slice(cast: &Path, before: u64, after: u64) -> std::io::Result<Vec<u8>> {
    let mut file = File::open(cast)?;
    file.seek(SeekFrom::Start(before))?;
    let mut buf = vec![0u8; (after - before) as usize];
    file.read_exact(&mut buf)?;
    Ok(buf)
}

#[cfg(test)]
#[path = "shell_exec_connect_tests.rs"]
mod connect_tests;
#[cfg(test)]
#[path = "shell_exec_tests.rs"]
mod tests;
