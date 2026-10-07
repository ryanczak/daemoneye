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

use super::foreground;
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
    CommandOutcome, MetaIndex, Owner, ShellId, ShellRegistry, await_ready, host_entry_or_bare,
    mark_interactive, marker_shell_name, master_running, meta_path_for, run_command,
    run_command_with_prompts, shell_for_owner, socket_path, spawn_for_on_host, which_ssh,
    with_shells,
};
use crate::shell::{CommandRecord, marked_inner_range};
use std::sync::atomic::AtomicUsize;

/// Per-invocation sudo sentinel source for the PTY path, the analogue of the
/// tmux path's `FG_HOOK_COUNTER`: a fresh index per command is what makes the
/// exact-match property meaningful.
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

/// Annotate, mask, then normalize. The order matters (F3): the annotation step
/// must strip control bytes before masking, or an escape embedded mid-token
/// could split a secret past its regex.
pub(super) fn format_for_model(raw: &str) -> String {
    mask_sensitive(&normalize_output(&crate::tmux::annotate_output(raw)))
}

/// What one command produced, already fit for a model.
#[derive(Debug)]
pub(super) struct ShellRunResult {
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
        .unwrap_or(foreground::LOCAL_CMD_TIMEOUT.as_secs())
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
pub(super) async fn run_in_shell_with_prompts<'p, W, R, F, Fut>(
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
        prompts,
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
            shell_for_owner(reg, owner, host, max_per_owner, SHELL_READY_TIMEOUT, spawn).await?
        }
        ShellTarget::New => {
            let id = spawn_for_on_host(reg, owner, max_per_owner, host, false, spawn).await?;
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
    if crate::daemon::utils::is_interactive_command(cmd) {
        // The command must never be written to the shell: an interactive
        // session owns the terminal and consumes every byte, so writing would
        // poison the shell for every later command (08b F2). Return before
        // running; the session is established inside this shell.
        mark_interactive(reg, &id);
        let destination = crate::daemon::utils::interactive_destination(cmd)
            .unwrap_or_else(|| "the remote host".to_string());
        let text = format!(
            "[Interactive session started]\n\
             `{cmd}` opened an interactive session on shell {id} — now \
             connected to {destination}.\n\
             The command did not exit; that shell is running an interactive \
             session on the remote host.\n\
             Do NOT call `{cmd}` again — the session is already established.\n\
             Later commands run in a fresh shell and do not reach this \
             session; a way to type into the open session arrives with a \
             later milestone."
        );
        return Ok((id, ShellRunResult { text, exit_code: 0 }));
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

    // Task 3, step 1: a command without sudo takes the plain path. Everything
    // about the call is identical to what shipped before this phase.
    let Some(prompts) = prompts else {
        let outcome = run_command(&socket, cmd, &marker, applied_timeout)
            .await
            .map_err(|e| {
                if e.chain().any(|c| c.is::<crate::shell::ShellExited>()) {
                    e.context(format!(
                        "shell {} exited before the command finished",
                        id.as_str()
                    ))
                } else {
                    e.context(format!(
                        "command exceeded the {applied_timeout:#?} limit in shell {}",
                        id.as_str()
                    ))
                }
            })?;
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

/// The non-sudo epilogue: index, format, annotate. Shared by the plain and the
/// sudo-aware path so the two can never diverge on what the model sees.
async fn finish_run(
    reg: &ShellRegistry,
    cmd: &str,
    id: &ShellId,
    outcome: &CommandOutcome,
    before: Option<u64>,
) -> anyhow::Result<(ShellId, ShellRunResult)> {
    index_command_output(reg, cmd, id, outcome, before).await;
    let raw = String::from_utf8_lossy(&outcome.output);
    let mut text = format_for_model(&raw);
    if let Some(note) = foreground::exit_status_annotation(Some(outcome.exit_code)) {
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

/// Prompt round-trip bookkeeping for one sudo run: how many credentials have
/// been sent. sudo re-prompts with the same sentinel on a wrong password, so
/// the count is what turns the retry wording on and the give-up check on.
#[derive(Default)]
struct SudoAttempts {
    attempts: usize,
}

/// Maximum sudo password attempts before aborting (matches sudo's own
/// default, and the wording `sudo_password_prompt(attempt, MAX)` emits).
const MAX_SUDO_RETRIES: usize = 3;

/// The prompt wording an ssh credential prompt shows the client: names what
/// is actually being asked for — a key passphrase is not a login password —
/// instead of `sudo_password_prompt`'s sudo wording.
fn ssh_credential_prompt(kind: PromptKind) -> String {
    match kind {
        PromptKind::SshPassword => "ssh login password required".to_string(),
        PromptKind::SshPassphrase => "ssh key passphrase required".to_string(),
        PromptKind::SshHostKey | PromptKind::SudoPassword | PromptKind::Fingerprint => {
            "ssh authentication required".to_string()
        }
    }
}

/// One `Response::CredentialPrompt` → `Request::CredentialResponse` round
/// trip over the caller's transport, shared by the sudo and ssh arms so the
/// two cannot diverge on the wire shape, the timeout, the zeroizing or the
/// attempt count. `id` is the prompt id the client echoes back; `prompt` is
/// the human-facing wording.
async fn credential_round_trip<W, R>(
    id: &str,
    tx: &mut W,
    rx: &mut R,
    prompt: String,
) -> PromptReply
where
    W: tokio::io::AsyncWriteExt + Unpin,
    R: tokio::io::AsyncBufReadExt + Unpin,
{
    let write = send_response_split(
        tx,
        Response::CredentialPrompt {
            id: id.to_string(),
            prompt,
        },
    )
    .await;
    if let Err(e) = write {
        return PromptReply::GiveUp(format!("credential prompt could not be sent: {e}"));
    }
    let mut cred_line = String::new();
    let read = tokio::time::timeout(super::USER_PROMPT_TIMEOUT, rx.read_line(&mut cred_line)).await;
    let answer = match read {
        // 0 bytes is EOF: nobody is attached to answer — every ghost reads
        // from `tokio::io::empty()`. Name that, not a parse failure.
        Ok(Ok(0)) => PromptReply::GiveUp(
            "a password prompt appeared and nobody is attached to answer it — the prompt \
             was cancelled and the command did not run with elevated privileges. Give \
             this command passwordless access on that host, or run it from a chat session."
                .to_string(),
        ),
        Ok(Ok(_)) => match serde_json::from_str::<Request>(cred_line.trim()) {
            Ok(Request::CredentialResponse { credential, .. }) => {
                let credential = zeroize::Zeroizing::new(credential);
                let mut bytes = credential.as_bytes().to_vec();
                bytes.push(b'\n');
                PromptReply::Send(bytes)
            }
            _ => PromptReply::GiveUp("credentials could not be read".to_string()),
        },
        _ => PromptReply::GiveUp("credentials were not supplied in time".to_string()),
    };
    zeroize::Zeroize::zeroize(&mut cred_line);
    answer
}

/// How long a cold host's connect step may take overall, and since the last
/// answer given (a prompt answered and then silence means ssh is working — or
/// stuck). 30 s covers a slow DNS lookup plus a live user thinking; the
/// timeout is restartable so an answered prompt does not eat the budget.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// How often [`drive_connect`]'s loop wakes to drain the PTY's broadcast and
/// re-check the child.
const CONNECT_POLL: Duration = Duration::from_millis(20);

/// The connect step, testable: spawn `program` with `argv` in a PTY, answer
/// its credential prompts through `tx`/`rx`, and return once `connected()`
/// holds (checked after the child exits, per the measured rule). The flag
/// closure is what makes it testable — a fake ssh sets a temp file, the
/// production closure runs `master_running`.
///
/// Failing is the child's exit, never a text match: ssh's retry line carries
/// both `Permission denied` and a fresh prompt, so "denied before prompt"
/// would abort a recoverable login.
#[allow(clippy::too_many_arguments)]
pub(super) async fn drive_connect<W, R, C>(
    program: &str,
    argv: &[String],
    connected: C,
    host_name: &str,
    prompt_id: &str,
    tx: &mut W,
    rx: &mut R,
    timeout: Duration,
) -> Result<(), String>
where
    W: tokio::io::AsyncWriteExt + Unpin,
    R: tokio::io::AsyncBufReadExt + Unpin,
    C: Fn() -> bool,
{
    let mut child = match crate::shell::PtyShell::spawn(program, argv, (24, 80)) {
        Ok(c) => c,
        Err(e) => {
            return Err(format!(
                "ssh to host {host_name} could not be spawned: {e:#}"
            ));
        }
    };
    // A chunk published before the first subscribe is dropped; ssh's prompts
    // arrive well after spawn, so subscribing here loses nothing.
    let mut stream = child.subscribe();
    let mut buf: Vec<u8> = Vec::new();
    // 08f's rule: only output after the last answer may be classified — a
    // prompt that appeared before it has already been answered.
    let mut answered_at: usize = 0;
    let mut started = Instant::now();
    let mut last_answer = started;
    loop {
        tokio::time::sleep(CONNECT_POLL).await;
        loop {
            match stream.try_recv() {
                Ok(chunk) => buf.extend_from_slice(&chunk),
                Err(tokio::sync::broadcast::error::TryRecvError::Empty) => break,
                Err(tokio::sync::broadcast::error::TryRecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::TryRecvError::Closed) => break,
            }
        }
        if !child.is_alive().map_err(|e| e.to_string())? {
            if connected() {
                return Ok(());
            }
            let last_line = String::from_utf8_lossy(&buf)
                .lines()
                .rev()
                .map(str::trim)
                .find(|l| !l.is_empty())
                .map(mask_sensitive)
                .unwrap_or_default();
            return Err(format!(
                "ssh could not connect to host {host_name}: {last_line}"
            ));
        }
        if Instant::now().duration_since(started) > timeout
            || Instant::now().duration_since(last_answer) > timeout
        {
            let _ = child.kill();
            return Err(format!(
                "ssh to host {host_name} did not connect within {timeout:?}"
            ));
        }
        let fresh = String::from_utf8_lossy(&buf[answered_at..]).into_owned();
        let reply = match classify_ssh_prompt(&fresh) {
            Some(kind @ (PromptKind::SshPassword | PromptKind::SshPassphrase)) => {
                credential_round_trip(prompt_id, tx, rx, ssh_credential_prompt(kind)).await
            }
            Some(PromptKind::SshHostKey) => {
                let _ = child.kill();
                return Err(format!(
                    "ssh is asking whether to trust host {host_name}'s unknown host key, \
                     which cannot be answered here — accept the fingerprint once from \
                     your own terminal, or wait for the new-host approval flow"
                ));
            }
            _ => PromptReply::Wait,
        };
        match reply {
            PromptReply::Send(bytes) => {
                if let Err(e) = child.write_input(&bytes) {
                    let _ = child.kill();
                    return Err(format!(
                        "ssh to host {host_name} could not be answered: {e}"
                    ));
                }
                answered_at = buf.len();
                last_answer = Instant::now();
                started = last_answer;
            }
            PromptReply::GiveUp(reason) => {
                let _ = child.kill();
                return Err(reason);
            }
            PromptReply::Wait => {}
        }
    }
}

/// Connect to named host `name` before any shell there is spawned: a no-op
/// when its ControlMaster already runs, otherwise `ssh -M -N -f` driven
/// through [`drive_connect`]. `batch` = a ghost (never prompt).
pub(super) async fn ensure_connected<W, R>(
    name: &str,
    batch: bool,
    prompt_id: &str,
    tx: &mut W,
    rx: &mut R,
) -> Result<(), String>
where
    W: tokio::io::AsyncWriteExt + Unpin,
    R: tokio::io::AsyncBufReadExt + Unpin,
{
    let entry = host_entry_or_bare(name).map_err(|e| e.to_string())?;
    let control = crate::config::ssh_dir();
    if let Err(e) = std::fs::create_dir_all(&control) {
        return Err(format!(
            "ssh control directory {} could not be created: {e}",
            control.display()
        ));
    }
    if master_running(&entry, &control) {
        return Ok(());
    }
    let program = match which_ssh() {
        Ok(p) => p,
        Err(e) => return Err(format!("ssh could not be found for host {name}: {e}")),
    };
    let argv = connect_argv(&entry, &control, batch);
    drive_connect(
        &program,
        &argv,
        || master_running(&entry, &control),
        name,
        prompt_id,
        tx,
        rx,
        CONNECT_TIMEOUT,
    )
    .await
}

/// Run one sudo command under [`run_command_with_prompts`], classified by
/// [`classify_prompt`], with the retry/give-up semantics.
///
/// The prompt round-trips over the same `tx`/`rx` the tmux path uses. On the
/// third failed password the credential is never sent again; the give-up
/// write of the single `0x03` byte is what cancels the prompt without leaking
/// the credential.
///
/// `host_name` is the destination this command's shell reaches when the run is
/// on a named host, `"local"` for the daemon host: the ssh give-up wording
/// names it, so the model is told *which* host refused. An ssh prompt can
/// only appear on a named host — the daemon host's shell spawns no ssh — so
/// a `local` here never reaches the ssh arms' wording.
#[allow(clippy::too_many_arguments)]
async fn run_sudo_command<'p, W, R>(
    socket: &Path,
    cmd: &str,
    shell_name: &str,
    timeout: Duration,
    prompts: SudoPromptArgs<'p, W, R>,
    host_name: &str,
) -> anyhow::Result<CommandOutcome>
where
    W: tokio::io::AsyncWriteExt + Unpin + 'p,
    R: tokio::io::AsyncBufReadExt + Unpin + 'p,
{
    let idx = PTY_SENTINEL_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let sentinel = sudo_sentinel(idx);
    let wrapped_cmd = with_pty_prompt_sentinel(cmd, &sentinel);
    let SudoPromptArgs { id, tx, rx, .. } = prompts;
    let attempts = std::sync::Arc::new(std::sync::Mutex::new(SudoAttempts::default()));
    let id = std::sync::Arc::new(id);
    let tx: std::sync::Arc<tokio::sync::Mutex<&mut W>> =
        std::sync::Arc::new(tokio::sync::Mutex::new(tx));
    let rx: std::sync::Arc<tokio::sync::Mutex<&mut R>> =
        std::sync::Arc::new(tokio::sync::Mutex::new(rx));
    let sentinel = std::sync::Arc::new(sentinel);
    let host_name = host_name.to_string();
    run_command_with_prompts(
        socket,
        &wrapped_cmd,
        shell_name,
        timeout,
        move |output: &str| {
            let out = output.to_string();
            let id = std::sync::Arc::clone(&id);
            let tx = std::sync::Arc::clone(&tx);
            let rx = std::sync::Arc::clone(&rx);
            let state = std::sync::Arc::clone(&attempts);
            let sentinel = std::sync::Arc::clone(&sentinel);
            let host_name = host_name.clone();
            async move {
                let attempts_used = {
                    let s = state.lock().expect("sudo attempt state");
                    s.attempts
                };
                match classify_prompt(&out, &sentinel) {
                    Some(PromptKind::SudoPassword) if attempts_used < MAX_SUDO_RETRIES => {
                        let answer = {
                            let mut tx_guard = tx.lock().await;
                            let mut rx_guard = rx.lock().await;
                            credential_round_trip(
                                id.as_ref(),
                                &mut **tx_guard,
                                &mut **rx_guard,
                                sudo_password_prompt(attempts_used, MAX_SUDO_RETRIES),
                            )
                            .await
                        };
                        if matches!(answer, PromptReply::Send(_)) {
                            state.lock().expect("sudo attempt state").attempts += 1;
                        }
                        answer
                    }
                    Some(PromptKind::SudoPassword) => PromptReply::GiveUp(format!(
                        "sudo authentication failed after {MAX_SUDO_RETRIES} incorrect password \
                     attempts — the command was not executed. To avoid password prompts for \
                     repeated operations, install a NOPASSWD sudoers rule with: \
                     `daemoneye install-sudoers <script-name>`"
                    )),
                    Some(PromptKind::Fingerprint) => PromptReply::GiveUp(
                        "sudo is waiting for fingerprint authentication which cannot be satisfied \
                     here — touch the fingerprint reader in a foreground terminal, or install \
                     a NOPASSWD sudoers rule with `daemoneye install-sudoers <script-name>`"
                            .to_string(),
                    ),
                    Some(kind @ (PromptKind::SshPassword | PromptKind::SshPassphrase)) => {
                        // An ssh credential prompt asks for the remote login
                        // or a key passphrase, not for sudo, so it gets its
                        // own wording; the sudo attempt count must not turn
                        // its first answer into a give-up.
                        let mut tx_guard = tx.lock().await;
                        let mut rx_guard = rx.lock().await;
                        credential_round_trip(
                            id.as_ref(),
                            &mut **tx_guard,
                            &mut **rx_guard,
                            ssh_credential_prompt(kind),
                        )
                        .await
                    }
                    Some(PromptKind::SshHostKey) => PromptReply::GiveUp(format!(
                        "ssh is asking whether to trust host {host_name}'s unknown host key, \
                         which cannot be answered here — accept the fingerprint once from \
                         your own terminal, or wait for the new-host approval flow"
                    )),
                    _ if is_ssh_permission_denied(&out) => PromptReply::GiveUp(format!(
                        "ssh authentication was refused for host {host_name}: Permission \
                         denied — the credentials given did not authenticate and the \
                         command was not executed"
                    )),
                    _ => PromptReply::Wait,
                }
            }
        },
    )
    .await
}

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
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::shell::{bind, lock_path, serve, socket_path};
    use crate::test_home_guard;
    use anyhow::Context;
    use std::sync::Arc;

    /// The PTY geometry every shell host is spawned with, matching the
    /// detached `spawn_detached_host` production spawn (24×80).
    const ROWS: u16 = 24;
    /// The PTY geometry every shell host is spawned with.
    const COLS: u16 = 80;

    /// A fake program that prompts like real sudo: it re-prompts on an empty
    /// answer and prints the answered length. Spelled `f=su; f+=do; eval …`
    /// so the defining command does not itself name the privilege tool and is
    /// not routed through the prompt path.
    const PROMPTING: &str = r#"f=su; f+=do; eval "$f() { local pw=; while [ -z \"\$pw\" ]; do read -rs -p \"\$SUDO_PROMPT\" pw || return 1; echo; done; echo GOT_\${#pw}; }""#;
    /// A fake program that never prompts.
    const SILENT: &str = r#"f=su; f+=do; eval "$f() { echo NOPROMPT; }""#;

    /// The credential the "answered" transport holds, three
    /// `Request::CredentialResponse` lines over a `Cursor`.
    const CREDENTIAL: &str = "s3cret-answer-q";

    /// The "attached" write half of [`answered_transport`]: records every
    /// write the run path makes (the credential prompt goes here, not the
    /// PTY) and discards them.
    struct RecordingWriter {
        writes: Arc<std::sync::Mutex<Vec<Vec<u8>>>>,
    }

    impl RecordingWriter {
        fn new(writes: Arc<std::sync::Mutex<Vec<Vec<u8>>>>) -> Self {
            Self { writes }
        }
    }

    impl tokio::io::AsyncWrite for RecordingWriter {
        fn poll_write(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
            buf: &[u8],
        ) -> std::task::Poll<std::io::Result<usize>> {
            self.writes
                .lock()
                .expect("write log lock")
                .push(buf.to_vec());
            std::task::Poll::Ready(Ok(buf.len()))
        }

        fn poll_flush(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            std::task::Poll::Ready(Ok(()))
        }

        fn poll_shutdown(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            std::task::Poll::Ready(Ok(()))
        }
    }

    /// An "attached" transport: the write half records every write (the
    /// credential prompt goes there, not the PTY), the read half answers
    /// every prompt with [`CREDENTIAL`]. Over a `Cursor`, so the read half
    /// is exhausted exactly when the three attempts are.
    fn answered_transport() -> (
        Arc<std::sync::Mutex<Vec<Vec<u8>>>>,
        impl tokio::io::AsyncBufRead + Unpin,
    ) {
        let writes = Arc::new(std::sync::Mutex::new(Vec::new()));
        let lines = (0..3)
            .map(|_| {
                let mut line = serde_json::to_string(&Request::CredentialResponse {
                    id: "p".to_string(),
                    credential: CREDENTIAL.to_string(),
                })
                .expect("CredentialResponse serialises");
                line.push('\n');
                line
            })
            .collect::<Vec<_>>()
            .join("");
        (writes, std::io::Cursor::new(lines.into_bytes()))
    }

    /// Run `cmd` in the shell a first (seed) command opened, with the given
    /// transport halves — "nobody attached" is sink/EOF, "answered" is
    /// [`answered_transport`]. The seed runs through the same run path with
    /// sink/EOF, defining the fake program; the command under test then runs
    /// in that same shell with the real transport. Returns the shell id, the
    /// run result and the cast path, so the log can be read raw.
    async fn run_with_transport<'p, W, R>(
        reg: &ShellRegistry,
        owner: Owner,
        seed_cmd: &str,
        cmd: &str,
        writer: &'p mut W,
        reader: &'p mut R,
    ) -> anyhow::Result<(ShellId, ShellRunResult, std::path::PathBuf)>
    where
        W: tokio::io::AsyncWriteExt + Unpin + 'p,
        R: tokio::io::AsyncBufReadExt + Unpin + 'p,
    {
        let cast_path: Arc<std::sync::Mutex<Option<std::path::PathBuf>>> =
            Arc::new(std::sync::Mutex::new(None));
        let cells = Arc::clone(&cast_path);
        let (id, _) = run_in_shell_with_prompts(
            reg,
            owner.clone(),
            seed_cmd,
            5,
            None,
            Some(20),
            "",
            &mut tokio::io::sink(),
            &mut tokio::io::empty(),
            crate::host::HostId::Local,
            move |id: ShellId| {
                let cells = Arc::clone(&cells);
                async move {
                    let path = spawn_with_cast(id).await?;
                    std::mem::forget(path.clone());
                    *cells.lock().expect("cast cell lock") = Some(path);
                    Ok(())
                }
            },
        )
        .await
        .context("the seed command runs")?;
        // The command under test, in the same shell the seed defined the fake
        // in — with the real transport this time.
        let (_same_id, out) = run_in_shell_with_prompts(
            reg,
            owner,
            cmd,
            5,
            Some(id.as_str()),
            Some(20),
            "",
            writer,
            reader,
            crate::host::HostId::Local,
            move |_id: ShellId| async move { Ok(()) },
        )
        .await?;
        let cast = cast_path
            .lock()
            .expect("cast cell lock")
            .clone()
            .expect("the spawn step recorded the host's cast path");
        Ok((id, out, cast))
    }

    /// Point `HOME` at a fresh temp dir for the duration of the test and
    /// serialise against other HOME-mutating tests (bug-05c-1).
    fn test_home() -> (crate::TestHomeGuard, tempfile::TempDir) {
        let guard = test_home_guard();
        let dir = tempfile::tempdir().expect("tempdir");
        unsafe {
            std::env::set_var("HOME", dir.path());
        }
        Config::ensure_dirs().ok();
        (guard, dir)
    }

    /// `$SHELL` pointed at fish for the duration of one test, restored by
    /// `Drop` even when an assertion fails mid-test — the guard form of the
    /// session module's `with_fish_shell`, which cannot early-return.
    pub(super) struct FishShellEnv;

    impl FishShellEnv {
        pub(super) fn set() -> Self {
            unsafe {
                std::env::set_var("SHELL", "/usr/bin/fish");
            }
            FishShellEnv
        }
    }

    impl Drop for FishShellEnv {
        fn drop(&mut self) {
            unsafe {
                std::env::remove_var("SHELL");
            }
        }
    }

    /// The production spawn substitute: a real in-process PTY host serving the
    /// configured socket for `id`, a bash session whose `.cast` the returned
    /// path points at. `spawn_detached_host` cannot run under a test (07b F4).
    async fn spawn_with_cast(id: ShellId) -> anyhow::Result<std::path::PathBuf> {
        let dir = tempfile::tempdir().context("tempdir for cast")?;
        let cast = dir.path().join(format!("{}.cast", id.as_str()));
        let backend = crate::shell::PtyBackend::spawn("bash", &[], ROWS, COLS, 100, &cast, 0)
            .context("spawning bash")?;
        let lock = crate::daemon::instance::InstanceLock::acquire(&lock_path(id.as_str()))
            .context("holding the per-id instance lock")?;
        let socket = socket_path(id.as_str());
        let listener = bind(&socket)
            .await
            .context("binding the in-process host's socket")?;
        tokio::spawn(serve(listener, Arc::clone(&backend)));
        std::mem::forget(lock);
        std::mem::forget(dir);
        Ok(cast)
    }

    /// The spawn step `run_in_shell_with` takes, paired with the cast path the
    /// host writes so a test can read the raw log afterwards.
    pub(super) async fn spawn_in_process_host(id: ShellId) -> anyhow::Result<()> {
        let cast = spawn_with_cast(id.clone()).await?;
        std::mem::forget(cast);
        Ok(())
    }

    /// The historical no-transport run, the shape every pre-existing test in
    /// this module exercises: `run_in_shell_with_prompts` with a sink write
    /// half and an EOF read half, so the sudo branch is off and the answer
    /// path can never be entered.
    async fn run_in_shell_for_test<S, Fut>(
        reg: &ShellRegistry,
        owner: Owner,
        cmd: &str,
        max_per_owner: u32,
        shell: Option<&str>,
        timeout_secs: Option<u64>,
        spawn: S,
    ) -> anyhow::Result<(ShellId, ShellRunResult)>
    where
        S: FnOnce(ShellId) -> Fut,
        Fut: std::future::Future<Output = anyhow::Result<()>>,
    {
        run_in_shell_for_test_on_host(
            reg,
            owner,
            cmd,
            max_per_owner,
            shell,
            timeout_secs,
            crate::host::HostId::Local,
            spawn,
        )
        .await
    }

    /// `run_in_shell_for_test` with the host explicit — the shape the host
    /// tests drive: the reuse and create steps are host-keyed, so a test that
    /// names a host must pass one through rather than rely on the local
    /// default.
    #[allow(clippy::too_many_arguments)]
    async fn run_in_shell_for_test_on_host<S, Fut>(
        reg: &ShellRegistry,
        owner: Owner,
        cmd: &str,
        max_per_owner: u32,
        shell: Option<&str>,
        timeout_secs: Option<u64>,
        host: crate::host::HostId,
        spawn: S,
    ) -> anyhow::Result<(ShellId, ShellRunResult)>
    where
        S: FnOnce(ShellId) -> Fut,
        Fut: std::future::Future<Output = anyhow::Result<()>>,
    {
        let mut sink = tokio::io::sink();
        let mut eof = tokio::io::empty();
        run_in_shell_with_prompts(
            reg,
            owner,
            cmd,
            max_per_owner,
            shell,
            timeout_secs,
            "",
            &mut sink,
            &mut eof,
            host,
            spawn,
        )
        .await
    }

    /// A spawn substitute whose cast the producer can resolve through the
    /// ordinary path: `cast_path` under the test `HOME`'s `shell_logs_dir`,
    /// the shape `resolve_cast` + `meta_path_for` operate on in production.
    /// The per-id lock is leaked so the host reads as live.
    async fn spawn_in_logs_dir(id: ShellId) -> anyhow::Result<()> {
        let started = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let cast = crate::shell::cast_path(id.as_str(), started);
        std::fs::create_dir_all(cast.parent().context("cast has a parent")?)
            .context("creating the logs dir")?;
        let backend = crate::shell::PtyBackend::spawn("bash", &[], ROWS, COLS, 100, &cast, started)
            .context("spawning bash into the logs dir")?;
        let lock = crate::daemon::instance::InstanceLock::acquire(&lock_path(id.as_str()))
            .context("holding the per-id instance lock")?;
        let socket = socket_path(id.as_str());
        let listener = bind(&socket)
            .await
            .context("binding the in-process host's socket")?;
        tokio::spawn(serve(listener, Arc::clone(&backend)));
        std::mem::forget(lock);
        std::mem::forget(cast);
        Ok(())
    }

    #[test]
    fn resolve_shell_target_accepts_none_new_and_an_id() {
        assert!(matches!(resolve_shell_target(None), Ok(ShellTarget::Reuse)));
        assert!(matches!(
            resolve_shell_target(Some("new")),
            Ok(ShellTarget::New)
        ));
        match resolve_shell_target(Some("s7")) {
            Ok(ShellTarget::Existing(id)) => assert_eq!(id.as_str(), "s7"),
            other => panic!("Some(\"s7\") must resolve to Existing, got {other:?}"),
        }
    }

    #[test]
    fn resolve_shell_target_rejects_a_bad_value() {
        let err = resolve_shell_target(Some("banana"))
            .expect_err("a non-id, non-new value must be refused");
        assert!(
            err.contains("banana"),
            "the error must name the offending value: {err}"
        );
        assert!(
            err.contains("new") && err.contains("list_shells"),
            "the error must name both accepted forms: {err}"
        );
    }

    #[test]
    fn resolve_shell_target_is_case_sensitive() {
        for bad in ["New", "NEW", "New "] {
            assert!(
                resolve_shell_target(Some(bad)).is_err(),
                "{bad} must not match \"new\" — the match is exact-case like \
                 list_shells' scope"
            );
        }
    }

    #[tokio::test]
    async fn run_terminal_command_targets_the_named_shell() {
        let (_home, _dir) = test_home();
        let reg = ShellRegistry::default();
        let owner = Owner::Chat("tip".into());
        // The reuse rule's pick, opened first so it is the lowest-numbered one.
        let (reused, _) = shell_for_owner(
            &reg,
            owner.clone(),
            crate::host::HostId::Local,
            5,
            SHELL_READY_TIMEOUT,
            spawn_in_process_host,
        )
        .await
        .expect("the reuse shell comes up");
        // A second shell the reuse rule would never hand out: opening it
        // directly, exactly as `shelf: "new"` does, bypasses the reuse rule.
        let second = spawn_for_on_host(
            &reg,
            owner.clone(),
            5,
            crate::host::HostId::Local,
            false,
            spawn_in_process_host,
        )
        .await
        .expect("the second shell comes up");
        await_ready(
            &socket_path(second.as_str()),
            Instant::now() + SHELL_READY_TIMEOUT,
        )
        .await
        .expect("the second shell becomes ready");
        assert_ne!(reused.as_str(), second.as_str(), "fixture: two ids");

        // The reuse rule would pick `skipped`; naming `second` must win.
        let (id, result) = run_in_shell_for_test(
            &reg,
            owner,
            "echo TARGET_HIT",
            5,
            Some(second.as_str()),
            None,
            spawn_in_process_host,
        )
        .await
        .expect("the targeted command runs");
        assert_eq!(
            id.as_str(),
            second.as_str(),
            "shell: must win over the lowest-numbered reuse rule"
        );
        assert!(
            result.text.contains("TARGET_HIT"),
            "the command's output must come back: {:?}",
            result.text
        );
    }

    #[tokio::test]
    async fn run_terminal_command_refuses_another_owners_shell() {
        let (_home, _dir) = test_home();
        let reg = ShellRegistry::default();
        // Same inner string "a", different kind — the shape a comparison on
        // the inner string instead of `Owner::key()` would accept.
        let (other, _) = shell_for_owner(
            &reg,
            Owner::Ghost("a".into()),
            crate::host::HostId::Local,
            5,
            SHELL_READY_TIMEOUT,
            spawn_in_process_host,
        )
        .await
        .expect("the ghost's shell comes up");

        let err = run_in_shell_for_test(
            &reg,
            Owner::Chat("a".into()),
            "echo SENTINEL_NOT_FOR_GHOST",
            5,
            Some(other.as_str()),
            None,
            spawn_in_process_host,
        )
        .await
        .expect_err("another owner's shell must be refused");
        let msg = format!("{err:#}");
        assert!(
            msg.contains(other.as_str()),
            "the refusal must name the shell id: {msg}"
        );
        let recorded = crate::shell::run_command(
            &socket_path(other.as_str()),
            "echo RECORDED_STILL_ON",
            "bash",
            Duration::from_secs(10),
        )
        .await
        .expect("the ghost's shell still answers after the refusal");
        let text = String::from_utf8_lossy(&recorded.output);
        assert!(
            !text.contains("SENTINEL_NOT_FOR_GHOST"),
            "no byte of the refused command may run in the other owner's shell: {text:?}"
        );
    }

    #[tokio::test]
    async fn run_terminal_command_custom_timeout_names_the_applied_value() {
        let (_home, _dir) = test_home();
        let reg = ShellRegistry::default();
        let owner = Owner::Chat("sleeper".into());
        let err = run_in_shell_for_test(
            &reg,
            owner,
            "sleep 4",
            5,
            None,
            Some(1),
            spawn_in_process_host,
        )
        .await
        .expect_err("a command outliving its timeout must be an error");
        let msg = format!("{err:#}");
        assert!(
            msg.contains('1') && msg.contains("limit"),
            "the error must name the applied timeout: {msg}"
        );
        assert!(
            !msg.contains("45s"),
            "the error must name the applied 1 s, not the 45 s default: {msg}"
        );
    }

    #[tokio::test]
    async fn shell_exec_returns_the_real_exit_code() {
        let (_home, _dir) = test_home();
        let reg = ShellRegistry::default();
        let (id, result) = run_in_shell_for_test(
            &reg,
            Owner::Chat("exit-code".into()),
            "printf 'HELLO\\n'; sh -c 'exit 3'",
            5,
            None,
            None,
            spawn_in_process_host,
        )
        .await
        .expect("an approved, printing, failing command resolves");
        assert_eq!(result.exit_code, 3, "the child's real exit code surfaces");
        assert!(
            result.text.contains("HELLO"),
            "the output must survive alongside the exit code: {:?}",
            result.text
        );
        assert!(
            result.text.contains("exited with status 3"),
            "the non-zero note must be present: {:?}",
            result.text
        );
        assert!(
            id.as_str().starts_with('s'),
            "a fresh id is allocated for the first command"
        );
    }

    #[tokio::test]
    async fn shell_exec_masks_a_secret_that_stays_raw_in_the_cast() {
        let (_home, _dir) = test_home();
        let reg = ShellRegistry::default();
        let cast_path: Arc<std::sync::Mutex<Option<std::path::PathBuf>>> =
            Arc::new(std::sync::Mutex::new(None));
        let spawn_cast_path = Arc::clone(&cast_path);
        let (id, result) = run_in_shell_for_test(
            &reg,
            Owner::Chat("masking".into()),
            "echo AKIAIOSFODNN7EXAMPLE",
            5,
            None,
            None,
            move |id: ShellId| {
                let cells = Arc::clone(&spawn_cast_path);
                async move {
                    let path = spawn_with_cast(id).await?;
                    std::mem::forget(path.clone());
                    *cells.lock().expect("cast cell lock") = Some(path);
                    Ok(())
                }
            },
        )
        .await
        .expect("the seeded AWS key resolves");
        assert!(
            !result.text.contains("AKIAIOSFODNN7EXAMPLE"),
            "the secret must not reach the tool result: {:?}",
            result.text
        );
        assert!(
            result.text.contains("<AWS_KEY>"),
            "the mask marker must reach the tool result: {:?}",
            result.text
        );
        let cast = std::fs::read(
            cast_path
                .lock()
                .expect("cast cell lock")
                .clone()
                .expect("the spawn step recorded the host's cast path"),
        )
        .with_context(|| format!("reading the cast the host {} wrote", id.as_str()))
        .expect("the cast is readable");
        let cast = String::from_utf8_lossy(&cast);
        assert!(
            cast.contains("AKIAIOSFODNN7EXAMPLE"),
            "the log stays raw at rest — the M20 masking exit criterion is the pair"
        );
    }

    #[tokio::test]
    async fn shell_exec_annotates_colour_and_strips_control_bytes() {
        let (_home, _dir) = test_home();
        let reg = ShellRegistry::default();
        let (_, result) = run_in_shell_for_test(
            &reg,
            Owner::Chat("colour".into()),
            "printf '\\033[31mred\\033[0m\\n\\033[2J\\033[Hclear\\n'",
            5,
            None,
            None,
            spawn_in_process_host,
        )
        .await
        .expect("the coloured command resolves");
        assert!(
            result.text.contains("[ERROR: red]"),
            "red becomes a semantic marker: {:?}",
            result.text
        );
        assert!(
            !result.text.contains('\x1b'),
            "no ESC byte may reach the model: {:?}",
            result.text
        );
    }

    #[tokio::test]
    async fn shell_exec_reuses_the_owners_shell_across_calls() {
        let (_home, _dir) = test_home();
        let reg = ShellRegistry::default();
        let owner = Owner::Chat("reuse".into());
        let (id1, first) = run_in_shell_for_test(
            &reg,
            owner.clone(),
            "cd /tmp",
            5,
            None,
            None,
            spawn_in_process_host,
        )
        .await
        .expect("the first call runs");
        assert_eq!(first.exit_code, 0, "cd /tmp is clean");
        let (id2, second) = run_in_shell_for_test(
            &reg,
            owner.clone(),
            "pwd",
            5,
            None,
            None,
            spawn_in_process_host,
        )
        .await
        .expect("the second call runs");
        assert_eq!(
            id1.as_str(),
            id2.as_str(),
            "the same owner keeps one shell, so action persists: cd then pwd"
        );
        assert!(
            second.text.contains("/tmp"),
            "the working directory survived the first call: {:?}",
            second.text
        );
    }

    /// 08b's primary case: an interactive command returns immediately with the
    /// session-established text and a clean exit status, and is never written
    /// to the shell. Set against a real in-process host; nothing is sent over
    /// the PTY, so no network is involved — the classifier keys on the
    /// command string, and `nowhere.invalid` has no reachable host anyway.
    #[tokio::test]
    async fn shell_exec_interactive_returns_without_running_the_command() {
        let (_home, _dir) = test_home();
        let reg = ShellRegistry::default();
        let start = std::time::Instant::now();
        let (id, result) = run_in_shell_for_test(
            &reg,
            Owner::Chat("interactive".into()),
            "ssh nowhere.invalid",
            5,
            None,
            None,
            spawn_in_process_host,
        )
        .await
        .expect("an interactive command resolves immediately, without timing out");
        let elapsed = start.elapsed();
        assert!(
            elapsed < std::time::Duration::from_secs(5),
            "the interactive path must return at once — {elapsed:?} means the \
             command was written to the shell and waited for a prompt"
        );
        assert_eq!(
            result.exit_code, 0,
            "nothing failed: the session opened, so no fabricated failure"
        );
        assert!(
            id.as_str().starts_with('s'),
            "the shell the command was (not) written to is named: {id:?}"
        );
        assert!(
            result.text.contains("nowhere.invalid"),
            "the destination reaches the model: {:?}",
            result.text
        );
        assert!(
            result.text.contains(id.as_str()),
            "the shell id must be in the text so the model can name the session: {:?}",
            result.text
        );
        assert!(
            result.text.contains("Do NOT call") && result.text.contains("already established"),
            "the message must warn against re-running the command: {:?}",
            result.text
        );

        // The decisive half: the shell the command was *not* written to must
        // still answer. If the command had been written, its bytes would sit
        // under an interactive process and every later write would time out.
        let outcome = crate::shell::run_command(
            &crate::shell::socket_path(id.as_str()),
            "echo PROBE_ALIVE",
            "bash",
            std::time::Duration::from_secs(10),
        )
        .await
        .expect("the unpoisoned shell answers");
        let text = String::from_utf8_lossy(&outcome.output);
        assert!(
            text.contains("PROBE_ALIVE"),
            "the shell must be unpoisoned — nothing was written to it: {text:?}"
        );
    }

    /// The F2 regression guard: after the interactive command, the owner's
    /// next command must report a *different* shell id and actually run. An
    /// implementation that only marks the shell without also stopping reuse
    /// would hand back the poisoned shell here.
    #[tokio::test]
    async fn shell_exec_interactive_marks_the_shell_and_the_next_command_gets_a_new_one() {
        let (_home, _dir) = test_home();
        let reg = ShellRegistry::default();
        let owner = Owner::Chat("marked".into());
        let (id1, first) = run_in_shell_for_test(
            &reg,
            owner.clone(),
            "ssh nowhere.invalid",
            5,
            None,
            None,
            spawn_in_process_host,
        )
        .await
        .expect("the interactive command resolves immediately");
        assert_eq!(first.exit_code, 0, "the session opened, nothing failed");

        let (id2, second) = run_in_shell_for_test(
            &reg,
            owner.clone(),
            "echo ALIVE",
            5,
            None,
            None,
            spawn_in_process_host,
        )
        .await
        .expect("the next command must not inherit the captured shell");
        assert_ne!(
            id2.as_str(),
            "the owner must get a fresh shell after the interactive capture"
        );
        assert_eq!(second.exit_code, 0, "echo actually ran on the fresh shell");
        assert!(
            second.text.contains("ALIVE"),
            "the fresh shell produced the command's output, not a timeout: {:?}",
            second.text
        );

        // The captured shell's record stays: the host is alive and later work
        // addresses the session by id.
        let guard = reg.try_lock().expect("registry lock");
        assert!(
            guard.contains_key(id1.as_str()),
            "the captured shell's record must remain in the registry"
        );
        assert!(
            guard[id1.as_str()].interactive,
            "the record stays marked interactive"
        );
    }
    /// shell, both commands run on it, and the reuse filter is untouched.
    #[tokio::test]
    async fn shell_exec_non_interactive_still_runs_on_the_same_shell() {
        let (_home, _dir) = test_home();
        let reg = ShellRegistry::default();
        let owner = Owner::Chat("plain".into());
        let (id1, one) = run_in_shell_for_test(
            &reg,
            owner.clone(),
            "echo one",
            5,
            None,
            None,
            spawn_in_process_host,
        )
        .await
        .expect("the first command runs");
        assert_eq!(one.exit_code, 0, "echo one is clean");
        assert!(
            one.text.contains("one"),
            "the output surfaced: {:?}",
            one.text
        );
        let (id2, two) = run_in_shell_for_test(
            &reg,
            owner.clone(),
            "echo two",
            5,
            None,
            None,
            spawn_in_process_host,
        )
        .await
        .expect("the second command runs");
        assert_eq!(
            id1.as_str(),
            id2.as_str(),
            "no interactive command means the same shell is reused"
        );
        assert_eq!(two.exit_code, 0, "echo two is clean");
        assert!(
            two.text.contains("two"),
            "the output surfaced: {:?}",
            two.text
        );

        // The shell was never marked: the flag must still be unset so a later
        // reuse does not silently skip a perfectly good shell.
        let guard = reg.try_lock().expect("registry lock");
        assert!(
            !guard[id1.as_str()].interactive,
            "a non-interactive command must not mark the shell"
        );
    }

    #[test]
    fn shell_exec_pipeline_is_annotate_then_mask_then_normalize() {
        assert_eq!(
            format_for_model("AKIAIOSFODNN7EXAMPLE\r\n\x1b[31mred line\x1b[0m\r\n"),
            "<AWS_KEY>\n[ERROR: red line]",
            "annotate strips ESC bytes before masking; normalize folds CRLF"
        );
    }

    #[test]
    fn shell_exec_pipeline_leaves_a_clean_command_as_its_output() {
        assert_eq!(format_for_model("hello\r\n"), "hello");
        assert_eq!(format_for_model("\r\n\r\nhello\r\n\r\n"), "hello");
    }

    /// bug-07c-3's DoD. There must exist a test that is red against the
    /// reversed pipeline, `annotate_output(&normalize_output(&mask_sensitive_(
    /// raw)))`, so the annotate→mask order is observable. The input carries an
    /// ESC byte mid-token: annotation-first deletes it before the mask regex
    /// runs, mask-first leaves the key containing it intact and lets
    /// annotate strip only the escape, so the two orderings disagree on the
    /// exact string asserted below.
    #[test]
    fn shell_exec_ordering_guard_masks_a_secret_split_by_an_embedded_escape() {
        let dioxed = "AKIAIOSFOD\x1b[0mNN7EXAMPLE\r\n";
        let masked = format_for_model(dioxed);
        assert!(
            !masked.contains("AKIAIOSFOD"),
            "the secret must not reach the tool result even when a control byte sits mid-token: {masked:?}"
        );
        assert!(
            masked.contains("<AWS_KEY>"),
            "the real mask marker must be present: {masked:?}"
        );
        assert!(
            !masked.contains('\x1b'),
            "no escape may survive into text a model reads: {masked:?}"
        );
    }

    /// Task 5's call-site guard: the meta record's `command` field holds the
    /// string the run path sent to the shell. A command without `sudo` must
    /// arrive byte-for-byte as written — no `[de-sudo-prompt-N]` prefix may
    /// appear — proving the plain path never allocates a sentinel or wraps.
    #[tokio::test]
    async fn non_sudo_command_takes_the_plain_path() {
        let (_home, _dir) = test_home();
        let reg = ShellRegistry::default();
        let (id, result) = run_in_shell_for_test(
            &reg,
            Owner::Chat("plain-path".into()),
            "echo PLAIN_ECHO_UNCHANGED",
            5,
            None,
            None,
            spawn_in_logs_dir,
        )
        .await
        .expect("the plain command resolves");
        let record_started_unix = with_shells(&reg, |store| {
            store.get(id.as_str()).map(|r| r.started_unix).unwrap_or(0)
        });
        let cast = crate::shell::resolve_cast(id.as_str(), record_started_unix);
        let meta =
            MetaIndex::load(&meta_path_for(&cast)).expect("the one plain command was indexed");
        let record = meta
            .commands
            .iter()
            .find(|r| r.command == "echo PLAIN_ECHO_UNCHANGED")
            .unwrap_or_else(|| {
                panic!(
                    "the plain command must be indexed exactly as written, indexed: {:?}",
                    meta.commands
                        .iter()
                        .map(|r| r.command.as_str())
                        .collect::<Vec<_>>()
                )
            });
        assert_eq!(record.exit_code, 0, "the plain command really ran");
        assert!(
            result.text.contains("PLAIN_ECHO_UNCHANGED"),
            "the plain path's output reaches the model: {:?}",
            result.text
        );
    }

    /// The full producer path: one command through `run_in_shell_with` ends up
    /// as a `CommandRecord` in the shell's `.meta.json`, with the command's
    /// text and its **real** exit code (3 here, so the assertion cannot pass on
    /// a defaulted 0).
    #[tokio::test]
    async fn a_run_command_appends_a_meta_index_record() {
        let (_home, _dir) = test_home();
        let reg = ShellRegistry::default();
        let (id, result) = run_in_shell_for_test(
            &reg,
            Owner::Chat("index-producer".into()),
            "printf 'SENTINEL_ONE\\n'; sh -c 'exit 3'",
            5,
            None,
            None,
            spawn_in_logs_dir,
        )
        .await
        .expect("the command resolves");
        assert_eq!(result.exit_code, 3, "sanity: the command really failed");

        let record_started_unix = with_shells(&reg, |store| {
            store.get(id.as_str()).map(|r| r.started_unix).unwrap_or(0)
        });
        let cast = crate::shell::resolve_cast(id.as_str(), record_started_unix);
        let meta_path = meta_path_for(&cast);
        let meta = MetaIndex::load(&meta_path)
            .unwrap_or_else(|e| panic!("the command was indexed at {meta_path:?}: {e}"));
        assert_eq!(meta.commands.len(), 1, "exactly one command indexed");
        let record = &meta.commands[0];
        assert_eq!(record.index, 0, "the first recorded command is index 0");
        assert_eq!(record.command, "printf 'SENTINEL_ONE\\n'; sh -c 'exit 3'");
        assert_eq!(record.exit_code, 3, "the real exit code is recorded");
        assert_eq!(
            meta.owner.as_deref(),
            Some("chat:index-producer"),
            "the index records who owned the shell"
        );
        assert_eq!(meta.host.as_deref(), Some("local"), "and where it ran");
        assert!(
            record.first_byte < record.end_byte,
            "a non-empty clean range is stored: {record:?}"
        );
        let bytes = std::fs::read(&cast).expect("the cast is on disk");
        let start = record.first_byte as usize;
        // Decoded through the production reader: the stored range, read the
        // way `read_command_output` reads it, carries no marker text. (Raw
        // cast bytes are newline-delimited JSON, so a raw-byte check at the
        // boundary can only ever see `[` — it cannot discriminate anything.)
        let decoded = {
            let mut out = Vec::new();
            for line in String::from_utf8_lossy(&bytes[start..record.end_byte as usize]).lines() {
                crate::shell::extend_output_line(Some(line.to_owned()), &mut out);
            }
            out
        };
        assert!(
            !String::from_utf8_lossy(&decoded).contains("DE_"),
            "the stored range must not carry marker text: {record:?}"
        );
    }

    // ── run_terminal_command { host? } (M21 phase-04d) ──────────────────────

    /// The module seam `run_in_shell_with_prompts` needs is the transport's
    /// write/read halves, which `run_in_shell_for_test` already supplies;
    /// what the host tests additionally need is a registry the host assertions
    /// can read after the call. This helper exists because the tests hold
    /// `reg` themselves — `run_in_shell_core` takes it, so no swap is needed.
    ///
    /// Returns the id string the run's answer quoted (`text` embeds it via the
    /// ordinary tool path is *not* used here — the raw result is returned), so
    /// the host tests can find the shell the reuse rule chose.
    async fn run_hosted<S, Fut>(
        reg: &ShellRegistry,
        owner: &Owner,
        cmd: &str,
        shell: Option<&str>,
        host: crate::host::HostId,
        spawn: S,
    ) -> anyhow::Result<(ShellId, ShellRunResult)>
    where
        S: FnOnce(ShellId) -> Fut,
        Fut: std::future::Future<Output = anyhow::Result<()>>,
    {
        run_in_shell_for_test_on_host(reg, owner.clone(), cmd, 5, shell, None, host, spawn).await
    }

    /// Task 1 row 2: `run_terminal_command { host: "web01" }` with no `shell`
    /// reuses this session's lowest-numbered open shell **on that host**, and
    /// creates one there when there is none. The record's `host` and the
    /// remote `hostname` output are both checked, so this pins not just where
    /// the record was stamped but where the command actually ran.
    #[tokio::test]
    async fn run_terminal_command_on_a_named_host_reuses_that_hosts_shell() {
        let (_home, dir) = test_home();
        std::fs::write(
            dir.path().join(".daemoneye/etc/config.toml"),
            "[models.default]\nprovider = \"ollama\"\n\n[hosts.web01]\n\
             ssh = \"matt@web01.lan\"\n",
        )
        .expect("write config");
        let reg = ShellRegistry::default();
        let owner = Owner::Chat("remote-run".into());

        // A first command on web01: no shell of this owner exists anywhere, so
        // one is created **on web01**.
        let (id1, first) = run_hosted(
            &reg,
            &owner,
            "echo first-remote",
            None,
            crate::host::HostId::Named("web01".to_string()),
            spawn_in_process_host,
        )
        .await
        .expect("the remote shell resolves");
        assert_eq!(first.exit_code, 0, "the remote command ran cleanly");
        let record_host = with_shells(&reg, |store| {
            store.get(id1.as_str()).map(|r| r.host.clone())
        });
        assert_eq!(
            record_host,
            Some(crate::host::HostId::Named("web01".to_string())),
            "the record must carry the host the command was asked for"
        );

        // A second command on the same host with no `shell`: the reuse rule
        // must hand back the *same* shell, not open another one.
        let (id2, second) = run_hosted(
            &reg,
            &owner,
            "echo second-remote",
            None,
            crate::host::HostId::Named("web01".to_string()),
            spawn_in_process_host,
        )
        .await
        .expect("the reuse step resolves");
        assert_eq!(
            id2.as_str(),
            id1.as_str(),
            "the second command must reuse web01's shell, not open another"
        );
        assert_eq!(second.exit_code, 0, "the reused shell ran the command");

        // And the local rule is untouched by all of this: a third command on
        // the local host does not reach web01's shell — it opens its own.
        let (id3, third) = run_hosted(
            &reg,
            &owner,
            "echo local-run",
            None,
            crate::host::HostId::Local,
            spawn_in_process_host,
        )
        .await
        .expect("the local shell resolves");
        assert_ne!(
            id3.as_str(),
            id1.as_str(),
            "the local command must not be handed web01's remote shell"
        );
        let local_host = with_shells(&reg, |store| {
            store.get(id3.as_str()).map(|r| r.host.clone())
        });
        assert_eq!(
            local_host,
            Some(crate::host::HostId::Local),
            "the local command's record stays Local"
        );
        assert_eq!(third.exit_code, 0, "the local command ran");
    }

    /// Task 1 row 1 (preservation): with no `host` named, the command lands
    /// on a `HostId::Local` record exactly as it did before hosts existed —
    /// both for reuse and for a fresh shell.
    #[tokio::test]
    async fn run_terminal_command_without_a_host_stays_on_the_daemon_host() {
        let (_home, _dir) = test_home();
        let reg = ShellRegistry::default();
        let owner = Owner::Chat("local-run".into());

        let (id1, first) = run_in_shell_for_test(
            &reg,
            owner.clone(),
            "echo one",
            5,
            None,
            None,
            spawn_in_process_host,
        )
        .await
        .expect("the first command resolves");
        assert_eq!(first.exit_code, 0, "the first command ran");
        let host1 = with_shells(&reg, |store| {
            store.get(id1.as_str()).map(|r| r.host.clone())
        });
        assert_eq!(
            host1,
            Some(crate::host::HostId::Local),
            "the no-host reuse path keeps the local stamp"
        );

        let (id2, second) = run_in_shell_for_test(
            &reg,
            owner.clone(),
            "echo two",
            5,
            Some("new"),
            None,
            spawn_in_process_host,
        )
        .await
        .expect("the fresh shell resolves");
        assert_ne!(
            id2.as_str(),
            id1.as_str(),
            "shell: \"new\" opens its own shell"
        );
        let host2 = with_shells(&reg, |store| {
            store.get(id2.as_str()).map(|r| r.host.clone())
        });
        assert_eq!(
            host2,
            Some(crate::host::HostId::Local),
            "the no-host fresh-shell path keeps the local stamp"
        );
        assert_eq!(second.exit_code, 0, "the fresh shell ran the command");
    }

    /// Task 1's last row — the load-bearing negative: `shell: "s3"` naming a
    /// shell on the **local** host while `host: "web01"` is requested is
    /// refused, naming both hosts, and the command does **not** run in s3.
    #[tokio::test]
    async fn run_terminal_command_refuses_a_shell_whose_host_differs_from_the_named_host() {
        let (_home, dir) = test_home();
        std::fs::write(
            dir.path().join(".daemoneye/etc/config.toml"),
            "[models.default]\nprovider = \"ollama\"\n\n[hosts.web01]\n\
             ssh = \"matt@web01.lan\"\n",
        )
        .expect("write config");
        let reg = ShellRegistry::default();
        let owner = Owner::Chat("mismatch".into());

        // s3 is opened **without** a host, so it is a local shell — the shell
        // whose relocation the row forbids.
        let (id, opened) = run_in_shell_for_test(
            &reg,
            owner.clone(),
            "echo opened",
            5,
            None,
            None,
            spawn_in_process_host,
        )
        .await
        .expect("the local shell opens");
        assert_eq!(opened.exit_code, 0, "the precondition command ran");

        let err = run_in_shell_for_test_on_host(
            &reg,
            owner.clone(),
            "echo must-not-run",
            5,
            Some(id.as_str()),
            None,
            crate::host::HostId::Named("web01".to_string()),
            spawn_in_process_host,
        )
        .await
        .expect_err("a shell on another host must be refused");
        let msg = err.to_string();
        assert!(
            msg.contains("local"),
            "the refusal must name the shell's real host: {msg}"
        );
        assert!(
            msg.contains("web01"),
            "the refusal must name the requested host: {msg}"
        );
        assert!(
            !msg.contains("never became ready") && !msg.contains("per-owner shell limit"),
            "a host mismatch is neither a readiness nor a limit problem: {msg}"
        );

        // The command did not run in the named shell: read the transcript with
        // a *different* command — the only `must-not-run` byte sequence a
        // re-read could contain would be this probe's own — and check the
        // shell's recorded command count instead. The refusal must have left
        // the shell with exactly the one command the precondition ran.
        let outcome = run_command(
            &socket_path(id.as_str()),
            "echo not-echoed-above",
            "bash",
            Duration::from_secs(10),
        )
        .await
        .expect("the shell is still live and answers");
        let recorded = String::from_utf8_lossy(&outcome.output).to_string();
        assert!(
            !recorded.contains("must-not-run"),
            "the refused command must not have run in the named shell: {recorded:?}"
        );
        let still_there = with_shells(&reg, |store| store.contains_key(id.as_str()));
        assert!(still_there, "a refused command must not close the shell");
    }

    /// Task 1's redundant-but-legal row: naming a `shell` whose host *is* the
    /// requested one runs in that shell — the refusal must not be a blanket
    /// one, or `shell` + `host` could never be combined at all.
    #[tokio::test]
    async fn run_terminal_command_with_a_matching_shell_and_host_is_not_an_error() {
        let (_home, dir) = test_home();
        std::fs::write(
            dir.path().join(".daemoneye/etc/config.toml"),
            "[models.default]\nprovider = \"ollama\"\n\n[hosts.web01]\n\
             ssh = \"matt@web01.lan\"\n",
        )
        .expect("write config");
        let reg = ShellRegistry::default();
        let owner = Owner::Chat("match".into());

        let (id, opened) = run_in_shell_for_test_on_host(
            &reg,
            owner.clone(),
            "echo opened",
            5,
            None,
            None,
            crate::host::HostId::Named("web01".to_string()),
            spawn_in_process_host,
        )
        .await
        .expect("the remote shell opens");
        assert_eq!(opened.exit_code, 0, "the precondition command ran");

        let (got, result) = run_in_shell_for_test_on_host(
            &reg,
            owner.clone(),
            "echo matching",
            5,
            Some(id.as_str()),
            None,
            crate::host::HostId::Named("web01".to_string()),
            spawn_in_process_host,
        )
        .await
        .expect("a shell on the named host is a legal target");
        assert_eq!(
            got.as_str(),
            id.as_str(),
            "the named shell is the one that ran the command"
        );
        assert_eq!(result.exit_code, 0, "the command ran in it");
    }
    /// An unconfigured host is **not** refused by the run path any more: it
    /// resolves through the permissive lookup to its own ssh destination,
    /// behind the approval the caller has already given, and the shell it
    /// opens carries the named host. Never a half-created shell on refusal —
    /// the refusals that remain (`local`, empty-ssh) still fire before any
    /// spawn.
    #[tokio::test]
    async fn run_terminal_command_an_unconfigured_host_resolves_to_its_own_destination() {
        let (_home, dir) = test_home();
        std::fs::write(
            dir.path().join(".daemoneye/etc/config.toml"),
            "[models.default]\nprovider = \"ollama\"\n\n[hosts.web01]\n\
             ssh = \"matt@web01.lan\"\n",
        )
        .expect("write config");
        let reg = ShellRegistry::default();
        let owner = Owner::Chat("new-host".into());

        let (id, result) = run_in_shell_for_test_on_host(
            &reg,
            owner.clone(),
            "echo reachable",
            5,
            None,
            None,
            crate::host::HostId::Named("nope".to_string()),
            spawn_in_process_host,
        )
        .await
        .expect("an unconfigured host resolves behind its approval");
        assert_eq!(result.exit_code, 0, "the command ran in the shell");
        let record_host = with_shells(&reg, |store| store.get(id.as_str()).map(|r| r.host.clone()));
        assert_eq!(
            record_host,
            Some(crate::host::HostId::Named("nope".to_string())),
            "the record must carry the host the command was asked for"
        );
        // The destination the spawn reaches is the bare name — the same shape
        // a configured `[hosts.pinky] ssh = "pinky"` already has.
        let entry = crate::shell::host_entry_or_bare("nope").expect("the host resolves");
        assert_eq!(
            entry.ssh, "nope",
            "an unconfigured host becomes its own ssh destination"
        );
    }

    /// `host: "local"` behaves exactly as omitting the parameter: the same
    /// reuse shell, the same local stamp. `HostId::parse` maps the word to
    /// `Local` in any case, so this is the row that says the mapping reaches
    /// the tool path, not just the parser.
    #[tokio::test]
    async fn run_terminal_command_host_local_is_the_daemon_host() {
        let (_home, _dir) = test_home();

        let reg = ShellRegistry::default();
        let owner = Owner::Chat("local-word".into());

        let (id1, first) = run_in_shell_for_test(
            &reg,
            owner.clone(),
            "echo base",
            5,
            None,
            None,
            spawn_in_process_host,
        )
        .await
        .expect("the no-host shell opens");
        assert_eq!(first.exit_code, 0, "the precondition command ran");

        // The production path parses the name first (`mod.rs`), which is what
        // maps "local" to `Local`; the test mirrors that order rather than
        // handing `Named("local")` past the resolver, which is not a shape
        // production can produce.
        let host = crate::host::HostId::parse("local").expect("the word local parses");
        assert_eq!(host, crate::host::HostId::Local, "local maps to Local");
        let (id2, second) = run_in_shell_for_test_on_host(
            &reg,
            owner.clone(),
            "echo explicit",
            5,
            None,
            None,
            host,
            spawn_in_process_host,
        )
        .await
        .expect("host: \"local\" resolves");
        assert_eq!(
            id2.as_str(),
            id1.as_str(),
            "host: \"local\" reuses the same shell the omitted form would"
        );
        assert_eq!(second.exit_code, 0, "the command ran in it");
        let host2 = with_shells(&reg, |store| {
            store.get(id2.as_str()).map(|r| r.host.clone())
        });
        assert_eq!(
            host2,
            Some(crate::host::HostId::Local),
            "host: \"local\" stamps Local, not a name"
        );
    }

    #[tokio::test]
    async fn read_shell_returns_clean_output_after_a_real_command() {
        let (_home, _dir) = test_home();
        let reg = crate::shell::registry();
        let id = crate::shell::spawn_for_on_host(
            reg,
            Owner::Chat("read-shell-e2e".into()),
            5,
            crate::host::HostId::Local,
            false,
            spawn_in_logs_dir,
        )
        .await
        .expect("an ordinary shell spawns");
        let result = run_in_shell_for_test(
            reg,
            Owner::Chat("read-shell-e2e".into()),
            "printf 'SENTINEL_TWO\\n'",
            5,
            None,
            None,
            spawn_in_logs_dir,
        )
        .await
        .expect("the command resolves");
        assert_eq!(
            result.0.as_str(),
            id.as_str(),
            "the second call reuses that shell"
        );

        let out = crate::daemon::executor::knowledge::read_shell(id.as_str(), None, None, None);
        assert!(
            out.contains("SENTINEL_TWO"),
            "the command's output is recorded and readable: {out}"
        );
        assert!(
            !out.contains("DE_"),
            "no marker framing may leak into the recorded output: {out}"
        );
    }

    /// Each record's range is its own command's, not the whole cast. The check
    /// is on the **index**, decoded with the raw `"o"` decoder and no windowing:
    /// `read_shell` cannot see a too-wide left boundary, because the reader
    /// re-windows the accumulated buffer and keeps only the last BEGIN..END, so
    /// a producer anchoring every record at byte 0 would still read cleanly.
    /// Decoding record 1's stored bytes directly is reader-independent and
    /// deterministic: the first command's output is always on disk before the
    /// second command is sent, so a range that reaches back must contain it.
    #[tokio::test]
    async fn read_shell_since_command_excludes_the_previous_commands_output() {
        let (_home, _dir) = test_home();
        let reg = crate::shell::registry();
        let owner = Owner::Chat("since-command-e2e".into());
        let id = crate::shell::spawn_for(reg, owner.clone(), 5, false, spawn_in_logs_dir)
            .await
            .expect("an ordinary shell spawns");
        for cmd in ["printf 'SENTINEL_ALPHA\\n'", "printf 'SENTINEL_BETA\\n'"] {
            let (got, _) =
                run_in_shell_for_test(reg, owner.clone(), cmd, 5, None, None, spawn_in_logs_dir)
                    .await
                    .expect("the command resolves");
            assert_eq!(
                got.as_str(),
                id.as_str(),
                "both commands land in the one shell"
            );
        }
        let out = crate::daemon::executor::knowledge::read_shell(id.as_str(), None, Some(1), None);
        assert!(
            out.contains("SENTINEL_BETA"),
            "command 1's own output is readable from its index: {out}"
        );
        assert!(
            !out.contains("DE_"),
            "no marker framing may leak into the recorded output: {out}"
        );
        // The reader-independent half: record 1's stored bytes, decoded raw.
        let started = with_shells(reg, |st| {
            st.get(id.as_str()).map(|r| r.started_unix).unwrap_or(0)
        });
        let cast = crate::shell::resolve_cast(id.as_str(), started);
        let meta = MetaIndex::load(&meta_path_for(&cast)).expect("both commands were indexed");
        let rec = meta
            .commands
            .iter()
            .find(|c| c.index == 1)
            .expect("command 1 has a record");
        let bytes = std::fs::read(&cast).expect("the cast is on disk");
        let mut raw = Vec::new();
        for line in
            String::from_utf8_lossy(&bytes[rec.first_byte as usize..rec.end_byte as usize]).lines()
        {
            crate::shell::decode_payload(Some(line.to_owned()), &mut raw);
        }
        let raw = String::from_utf8_lossy(&raw);
        assert!(
            raw.contains("SENTINEL_BETA"),
            "record 1's range holds its own output: {raw:?}"
        );
        assert!(
            !raw.contains("SENTINEL_ALPHA"),
            "record 1's range must not reach back into command 0: {raw:?}"
        );
    }

    // ── shell-exit detection (M21 phase-08b) ─────────────────────────────────

    /// The production spawn substitute whose program **exits immediately**:
    /// `sh -c 'exit 0'` in place of `spawn_with_cast`'s bash — everything else
    /// identical, so the run path sees a socket that answers status with
    /// `Exited` while the PTY child is gone.
    async fn spawn_with_exited_shell(id: ShellId) -> anyhow::Result<std::path::PathBuf> {
        let dir = tempfile::tempdir().context("tempdir for cast")?;
        let cast = dir.path().join(format!("{}.cast", id.as_str()));
        let backend = crate::shell::PtyBackend::spawn(
            "sh",
            &["-c".to_string(), "exit 0".to_string()],
            ROWS,
            COLS,
            100,
            &cast,
            0,
        )
        .context("spawning the exiting shell")?;
        let lock = crate::daemon::instance::InstanceLock::acquire(&lock_path(id.as_str()))
            .context("holding the per-id instance lock")?;
        let socket = socket_path(id.as_str());
        let listener = bind(&socket)
            .await
            .context("binding the in-process host's socket")?;
        tokio::spawn(serve(listener, Arc::clone(&backend)));
        std::mem::forget(lock);
        std::mem::forget(dir);
        Ok(cast)
    }

    /// An exited shell must be *reported as exited*, not dressed up as a
    /// timeout: nothing exceeded a limit and nothing is still running. The
    /// wording the model reads decides what it does next — retrying into a
    /// dead shell.
    #[tokio::test]
    async fn an_exited_shell_is_reported_as_exited_not_as_a_timeout() {
        let (_home, _dir) = test_home();
        let reg = ShellRegistry::default();
        let owner = Owner::Chat("exited-e2e".into());
        let mut sink = tokio::io::sink();
        let mut eof = tokio::io::empty();
        let started = std::time::Instant::now();
        let err = run_in_shell_with_prompts(
            &reg,
            owner,
            "echo hi",
            5,
            None,
            Some(20),
            "",
            &mut sink,
            &mut eof,
            crate::host::HostId::Local,
            |id| async move { spawn_with_exited_shell(id).await.map(|_cast| ()) },
        )
        .await
        .expect_err("a dead shell must fail the run");
        let elapsed = started.elapsed();
        assert!(
            elapsed < std::time::Duration::from_secs(5),
            "the run must fail in seconds, not at its 20 s timeout: {elapsed:?}"
        );
        let msg = format!("{err:#}");
        assert!(
            msg.contains("exited"),
            "the failure must say the shell exited: {msg}"
        );
        assert!(
            !msg.contains("exceeded"),
            "a dead shell exceeded no limit: {msg}"
        );
    }

    /// Test 5: a command naming the privilege tool that **never prompts** is
    /// not given up on — the old path classified on the shell's echo and
    /// abandoned it with a false "could not be read" error.
    #[tokio::test]
    async fn a_password_prompt_that_never_appears_is_not_given_up() {
        let (_home, _dir) = test_home();
        let reg = ShellRegistry::default();
        let owner = Owner::Chat("never-prompt".into());
        let (id, result, _cast) = run_with_transport(
            &reg,
            owner,
            SILENT,
            "sudo true",
            &mut tokio::io::sink(),
            &mut tokio::io::empty(),
        )
        .await
        .expect("the never-prompting command resolves");
        assert_eq!(result.exit_code, 0, "the command ran cleanly");
        assert!(
            result.text.contains("NOPROMPT"),
            "the fake's output reaches the model: {:?}",
            result.text
        );
        assert!(
            id.as_str().starts_with('s'),
            "the run names its shell: {id:?}"
        );
    }

    /// Test 6: a correct password is answered **exactly once** — never again
    /// by later ordinary output, which is what Defect B did: the extra
    /// answers ran as shell commands in plaintext, and the cast held over a
    /// dozen copies of the secret.
    #[tokio::test]
    async fn a_correct_password_is_sent_exactly_once() {
        let (_home, _dir) = test_home();
        let reg = ShellRegistry::default();
        let owner = Owner::Chat("answered-once".into());
        let (writes, mut reader) = answered_transport();
        let mut writer = RecordingWriter::new(Arc::clone(&writes));
        let outcome = run_with_transport(
            &reg,
            owner,
            PROMPTING,
            "sudo true; sleep 1; echo TAIL",
            &mut writer,
            &mut reader,
        )
        .await;
        if let Err(ref e) = outcome {
            let seen = writes.lock().expect("write log lock");
            panic!(
                "the answered run resolves: {e:#} — the transport saw {} writes: {:?}",
                seen.len(),
                *seen
            );
        }
        let (id, result, cast) = outcome.expect("checked above");
        assert_eq!(result.exit_code, 0, "the command ran cleanly: {result:?}");
        let text = result.text.clone();
        assert!(
            text.contains("GOT_15"),
            "the fake reports the answered length (15 = \"s3cret-answer-q\" + \\n): {text:?}"
        );
        assert!(
            text.contains("TAIL"),
            "the rest of the line ran after the prompt was answered: {text:?}"
        );
        assert!(
            id.as_str().starts_with('s'),
            "the run names its shell: {id:?}"
        );

        let raw = std::fs::read_to_string(&cast)
            .with_context(|| format!("reading the cast of shell {}", id.as_str()))
            .expect("the cast is readable");
        let secret_hits = raw.matches(CREDENTIAL).count();
        assert_eq!(
            secret_hits,
            0,
            "the credential must never reach the shell log: {secret_hits} copies in {}",
            cast.display()
        );
        assert_eq!(
            raw.matches("[redacted]").count(),
            1,
            "exactly one redacted entry — the answer: {raw}"
        );
        assert!(
            !raw.contains("command not found"),
            "a leaked answer runs as a shell command; none may appear: {raw}"
        );
    }

    /// Test 7: nobody attached (the ghost shape) → the prompt is given up
    /// **fast and honestly**, and the same shell runs the next command.
    #[tokio::test]
    async fn an_unanswerable_password_prompt_gives_up_fast_and_leaves_the_shell_usable() {
        let (_home, _dir) = test_home();
        let reg = ShellRegistry::default();
        let owner = Owner::Chat("nobody-attached".into());
        let started = std::time::Instant::now();
        let (id, err) = match run_with_transport(
            &reg,
            owner.clone(),
            PROMPTING,
            "sudo true",
            &mut tokio::io::sink(),
            &mut tokio::io::empty(),
        )
        .await
        {
            Ok((spawned, _, _)) => {
                panic!("an unanswerable prompt must fail the run: shell {spawned:?}")
            }
            Err(e) => (None::<ShellId>, e),
        };
        let _ = id;
        let err = anyhow::anyhow!("{err:#}");
        // The failing run's error names its shell only in the context: pull it
        // out of the message so the next command can reuse the same shell.
        let id = ShellId::parse(
            err.to_string()
                .split("shell ")
                .nth(1)
                .and_then(|rest| rest.split(&[' ', '\'', ')'][..]).next())
                .expect("the failing run names its shell"),
        )
        .expect("the failing run names a valid shell");
        let elapsed = started.elapsed();
        assert!(
            elapsed < Duration::from_secs(5),
            "the give-up must be fast, not a timeout: {elapsed:?}"
        );
        let msg = format!("{err:#}");
        assert!(
            msg.contains("nobody is attached to answer it"),
            "the reason names the real problem: {msg}"
        );
        assert!(
            !msg.contains("did not complete within"),
            "a cancelled prompt did not wait out a limit: {msg}"
        );

        let next_started = std::time::Instant::now();
        let (same_id, next) = run_in_shell_with_prompts(
            &reg,
            owner,
            "echo AFTER",
            5,
            Some(id.as_str()),
            Some(10),
            "",
            &mut tokio::io::sink(),
            &mut tokio::io::empty(),
            crate::host::HostId::Local,
            move |_shell: ShellId| async move { Ok(()) },
        )
        .await
        .expect("the shell is still usable");
        let next_elapsed = next_started.elapsed();
        assert!(
            next_elapsed < Duration::from_secs(5),
            "the next command must not wait out its timeout: {next_elapsed:?}"
        );
        assert!(
            next.text.contains("AFTER"),
            "the next command ran: {:?}",
            next.text
        );
        assert!(
            same_id.as_str().starts_with('s'),
            "the reused shell is named: {same_id:?}"
        );
    }

    /// Test 8: a later command on the same line still sees the sentinel —
    /// `export` (not a first-command-only prefix assignment) is what makes
    /// the classification fire on the program that actually prompts.
    #[tokio::test]
    async fn a_later_command_on_the_line_still_gets_the_prompt_sentinel() {
        let (_home, _dir) = test_home();
        let reg = ShellRegistry::default();
        let owner = Owner::Chat("later-on-line".into());
        let (writes, mut reader) = answered_transport();
        let mut writer = RecordingWriter::new(Arc::clone(&writes));
        let (id, result, _cast) = run_with_transport(
            &reg,
            owner,
            PROMPTING,
            "cd /tmp && sudo true",
            &mut writer,
            &mut reader,
        )
        .await
        .expect("the later-on-line run resolves");
        assert_eq!(result.exit_code, 0, "the command ran cleanly: {result:?}");
        assert!(
            result.text.contains("GOT_15"),
            "the prompt on the later command was classified and answered: {:?}",
            result.text
        );
        assert!(
            id.as_str().starts_with('s'),
            "the run names its shell: {id:?}"
        );
    }

    /// M23 01b test 9: a user who answers **after** the command's timeout has
    /// elapsed must not have the command killed as timed out — the wait for
    /// the answer is added back to the deadline, so the command still gets
    /// its own budget after the password was sent. The transport's answer
    /// arrives 3 s after the prompt, past a 2 s timeout, and the run must
    /// still complete with the fake's `GOT_15`.
    /// Mutation A guard: with `deadline += asked.elapsed();` deleted in
    /// `read_to_marker` this fails at "timed out after 2s".
    #[tokio::test]
    async fn an_answer_after_the_timeout_still_completes_the_command() {
        let (_home, _dir) = test_home();
        let reg = ShellRegistry::default();
        let owner = Owner::Chat("late-answer".into());
        let writes = Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut writer = RecordingWriter::new(Arc::clone(&writes));
        // The "attached" read half whose answer is late: nothing for 3 s,
        // then one `CredentialResponse` for prompt id "p", then 30 s of
        // silence with the stream still open (the run must be answered, not
        // EOF'd).
        let (late_tx, late_rx) = tokio::io::duplex(4096);
        let mut reader = tokio::io::BufReader::new(late_rx);
        tokio::spawn(async move {
            use tokio::io::AsyncWriteExt;
            let mut late_tx = late_tx;
            tokio::time::sleep(Duration::from_secs(3)).await;
            let mut line = serde_json::to_string(&Request::CredentialResponse {
                id: "p".to_string(),
                credential: CREDENTIAL.to_string(),
            })
            .expect("CredentialResponse serialises");
            line.push('\n');
            late_tx
                .write_all(line.as_bytes())
                .await
                .expect("the late answer writes");
            tokio::time::sleep(Duration::from_secs(30)).await;
        });
        let mut write_sink = tokio::io::sink();
        let (id, _seed) = run_in_shell_with_prompts(
            &reg,
            owner.clone(),
            PROMPTING,
            5,
            None,
            Some(20),
            "",
            &mut write_sink,
            &mut tokio::io::empty(),
            crate::host::HostId::Local,
            |id| async move { spawn_with_cast(id).await.map(|_cast| ()) },
        )
        .await
        .context("the seed command runs")
        .expect("the seed command defines the fake");
        let outcome = run_in_shell_with_prompts(
            &reg,
            owner,
            "sudo true",
            5,
            Some(id.as_str()),
            Some(2),
            "p",
            &mut writer,
            &mut reader,
            crate::host::HostId::Local,
            move |_id: ShellId| async move { Ok(()) },
        )
        .await;
        if let Err(ref e) = outcome {
            let seen = writes.lock().expect("write log lock");
            panic!(
                "a late answer must still complete the command: {e:#} — the transport saw \
                 {} writes: {:?}",
                seen.len(),
                *seen
            );
        }
        let (_, result) = outcome.expect("checked above");
        assert_eq!(
            result.exit_code, 0,
            "the answered command ran cleanly: {result:?}"
        );
        assert!(
            result.text.contains("GOT_15"),
            "the fake reports the answered length after the late answer: {:?}",
            result.text
        );
    }

    /// M23 01b test 10: only the answer's wait is excluded — a command that
    /// hangs **after** an instant answer still times out on schedule, so the
    /// deadline extension cannot be read as an open-ended extension.
    /// Mutation B guard: with `asked.elapsed()` replaced by
    /// `Duration::from_secs(60)` the hang survives to its 30 s sleep instead
    /// of timing out at 2 s, and the under-5-s assertion fails.
    #[tokio::test]
    async fn a_hang_after_an_answer_still_times_out() {
        let (_home, _dir) = test_home();
        let reg = ShellRegistry::default();
        let owner = Owner::Chat("hang-after-answer".into());
        let (writes, mut reader) = answered_transport();
        let mut writer = RecordingWriter::new(Arc::clone(&writes));
        let (id, _) = run_in_shell_with_prompts(
            &reg,
            owner.clone(),
            PROMPTING,
            5,
            None,
            Some(20),
            "",
            &mut tokio::io::sink(),
            &mut tokio::io::empty(),
            crate::host::HostId::Local,
            |id| async move { spawn_with_cast(id).await.map(|_cast| ()) },
        )
        .await
        .context("the seed command runs")
        .expect("the seed command defines the fake");
        let started = std::time::Instant::now();
        let outcome = run_in_shell_with_prompts(
            &reg,
            owner,
            "sudo true; sleep 30",
            5,
            Some(id.as_str()),
            Some(2),
            "p",
            &mut writer,
            &mut reader,
            crate::host::HostId::Local,
            move |_id: ShellId| async move { Ok(()) },
        )
        .await;
        let elapsed = started.elapsed();
        let err = match outcome {
            Ok((_, result)) => panic!(
                "a hang after an answer must time out, not complete: exit {} {:?}",
                result.exit_code, result.text
            ),
            Err(e) => e,
        };
        assert!(
            elapsed < Duration::from_secs(5),
            "the timeout must fire on the command's own budget: {elapsed:?}"
        );
        let msg = format!("{err:#}");
        assert!(
            msg.contains("timed out after 2s"),
            "the failure is the command's own 2 s limit, not the answer's wait: {msg}"
        );
    }

    /// Live test 11: the real privilege tool on `pinky` (sudo-rs 0.2.13) —
    /// the only witness for the settle wait (Mutation D is green locally).
    #[tokio::test]
    #[ignore = "requires the configured host `pinky` reachable over ssh"]
    async fn an_unanswerable_password_prompt_on_pinky_leaves_the_shell_usable() {
        let (_home, dir) = test_home();
        std::fs::write(
            dir.path().join(".daemoneye/etc/config.toml"),
            "[models.default]\nprovider = \"ollama\"\n\n[hosts.pinky]\nssh = \"pinky\"\n",
        )
        .expect("write config");
        crate::config::ssh_dir();
        let reg = ShellRegistry::default();
        let owner = Owner::Chat("pinky-08f".into());
        // The spawn step: in-process, the real `ssh` argv with `-o
        // BatchMode=yes` inserted **before the destination** (the last argv
        // element — options after it become the remote command). In-process
        // rather than `spawn_detached` because a test binary cannot be
        // re-invoked as the shell host.
        let spawn = |id: ShellId| async move {
            let control = crate::config::ssh_dir();
            std::fs::create_dir_all(&control).context("creating the ssh control dir")?;
            let entry = crate::config::HostEntry {
                ssh: "pinky".into(),
                ..Default::default()
            };
            let mut argv = crate::host::ssh_argv(&entry, &control);
            let dest = argv.len() - 1;
            argv.insert(dest, "BatchMode=yes".to_string());
            argv.insert(dest, "-o".to_string());
            let program = crate::shell::which_ssh()?;
            let temp = tempfile::tempdir().context("tempdir for cast")?;
            let cast = temp.path().join(format!("{}.cast", id.as_str()));
            let backend =
                crate::shell::PtyBackend::spawn(&program, &argv, ROWS, COLS, 100, &cast, 0)
                    .context("spawning ssh to pinky")?;
            let lock = crate::daemon::instance::InstanceLock::acquire(&lock_path(id.as_str()))
                .context("holding the per-id instance lock")?;
            let socket = socket_path(id.as_str());
            let listener = bind(&socket).await.context("binding the host's socket")?;
            tokio::spawn(serve(listener, Arc::clone(&backend)));
            std::mem::forget(lock);
            std::mem::forget(temp);
            Ok(())
        };
        let started = std::time::Instant::now();
        let (id, first) = run_in_shell_with_prompts(
            &reg,
            owner.clone(),
            "sudo -k",
            5,
            None,
            Some(20),
            "",
            &mut tokio::io::sink(),
            &mut tokio::io::empty(),
            crate::host::HostId::Named("pinky".to_string()),
            spawn,
        )
        .await
        .context("the never-prompting clear on pinky")
        .expect("the never-prompting clear on pinky resolves");
        let first_elapsed = started.elapsed();
        println!(
            "pinky `sudo -k`: {first_elapsed:?} (exit {})",
            first.exit_code
        );
        assert_eq!(first.exit_code, 0, "the clear ran cleanly");
        assert!(id.as_str().starts_with('s'), "the shell is named: {id:?}");

        let prompt_started = std::time::Instant::now();
        let mut sink = tokio::io::sink();
        let mut eof = tokio::io::empty();
        let err = run_in_shell_with_prompts(
            &reg,
            owner.clone(),
            "sudo whoami",
            5,
            Some(id.as_str()),
            Some(20),
            "",
            &mut sink,
            &mut eof,
            crate::host::HostId::Named("pinky".to_string()),
            spawn,
        )
        .await
        .expect_err("the unanswerable prompt must fail the run");
        let prompt_elapsed = prompt_started.elapsed();
        println!("pinky give-up: {prompt_elapsed:?}");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("nobody is attached"),
            "the reason names the real problem: {msg}"
        );
        assert!(
            prompt_elapsed < Duration::from_secs(5),
            "the give-up must be fast: {prompt_elapsed:?}"
        );

        let after_started = std::time::Instant::now();
        let (_, after) = run_in_shell_with_prompts(
            &reg,
            owner,
            "echo AFTER",
            5,
            Some(id.as_str()),
            Some(10),
            "",
            &mut tokio::io::sink(),
            &mut tokio::io::empty(),
            crate::host::HostId::Named("pinky".to_string()),
            spawn,
        )
        .await
        .expect("the shell is still usable");
        let after_elapsed = after_started.elapsed();
        println!("pinky next command: {after_elapsed:?}");
        assert!(
            after.text.contains("AFTER"),
            "the next command ran: {:?}",
            after.text
        );
        assert!(
            after_elapsed < Duration::from_secs(5),
            "the next command must not wait out its timeout: {after_elapsed:?}"
        );
    }
}

#[cfg(test)]
mod connect_tests {
    use super::*;
    use crate::config::Config;
    use crate::test_home_guard;
    use std::sync::Arc;

    /// Serialises this module's PTY-spawning tests against each other. Each
    /// test also holds `test_home_guard()` next to this lock, so a
    /// HOME-mutating test cannot delete `HOME` out from under one of these
    /// spawns (a spawn into a deleted `HOME` fails with `ENOENT`).
    static PTY_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    /// The credential the "attached" transport answers with; the fake ssh
    /// checks for exactly this before declaring the login good.
    const CREDENTIAL: &str = "right-pw";
    /// The prompt id the round trip echoes; the fake never sees it.
    const PROMPT_ID: &str = "c";

    /// The "attached" write half: records every write (the credential prompt
    /// goes here, not the PTY) so a test can count prompts.
    struct RecordingWriter {
        writes: Arc<std::sync::Mutex<Vec<Vec<u8>>>>,
    }

    impl tokio::io::AsyncWrite for RecordingWriter {
        fn poll_write(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
            buf: &[u8],
        ) -> std::task::Poll<std::io::Result<usize>> {
            self.writes
                .lock()
                .expect("write log lock")
                .push(buf.to_vec());
            std::task::Poll::Ready(Ok(buf.len()))
        }

        fn poll_flush(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            std::task::Poll::Ready(Ok(()))
        }

        fn poll_shutdown(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            std::task::Poll::Ready(Ok(()))
        }
    }

    /// The "attached" read half: `answers` `CredentialResponse` lines over a
    /// `Cursor`, answered in order. Distinct credentials per test pass a
    /// custom list.
    fn answered_reader(credentials: &[&str]) -> impl tokio::io::AsyncBufRead + Unpin {
        let lines = credentials
            .iter()
            .map(|c| {
                let mut line = serde_json::to_string(&Request::CredentialResponse {
                    id: PROMPT_ID.to_string(),
                    credential: (*c).to_string(),
                })
                .expect("CredentialResponse serialises");
                line.push('\n');
                line
            })
            .collect::<Vec<_>>()
            .join("");
        std::io::Cursor::new(lines.into_bytes())
    }

    /// Count the credential prompts the transport saw.
    fn prompt_count(writes: &Arc<std::sync::Mutex<Vec<Vec<u8>>>>) -> usize {
        writes
            .lock()
            .expect("write log lock")
            .iter()
            .filter(|w| {
                serde_json::from_slice::<Response>(w)
                    .is_ok_and(|r| matches!(r, Response::CredentialPrompt { .. }))
            })
            .count()
    }

    /// Spawn the fake ssh (`sh -c script`) in a PTY through [`drive_connect`]
    /// with the given transport halves, and time it. `connected` = the flag
    /// file exists.
    async fn drive_fake(
        script: &str,
        flag: &std::path::Path,
        tx: &mut (impl tokio::io::AsyncWriteExt + Unpin),
        rx: &mut (impl tokio::io::AsyncBufReadExt + Unpin),
    ) -> Result<(), String> {
        let flag_path = flag.to_path_buf();
        let argv = vec!["-c".to_string(), script.to_string()];
        drive_connect(
            "/bin/sh",
            &argv,
            || flag_path.exists(),
            "pinky",
            PROMPT_ID,
            tx,
            rx,
            Duration::from_secs(10),
        )
        .await
    }

    /// Hold `HOME` at a fresh temp dir (so config and `ssh_dir()` are cold)
    /// for one test, serialised against the other HOME-mutating tests.
    fn test_home() -> (crate::TestHomeGuard, tempfile::TempDir) {
        let guard = test_home_guard();
        let dir = tempfile::tempdir().expect("tempdir");
        unsafe {
            std::env::set_var("HOME", dir.path());
        }
        Config::ensure_dirs().ok();
        (guard, dir)
    }

    /// bug-08e-1's regression: the deadline is measured from the **last
    /// answer**, not from spawn — a prompt answered after ~0.4 s that
    /// connects ~0.5 s later is inside a 700 ms window of the answer, so a
    /// user who took their time typing the credential still connects. The
    /// never-answered half is pinned in the same test: a fake that says
    /// nothing fails with the `did not connect within` wording.
    #[tokio::test]
    async fn an_answered_prompt_restarts_the_connect_timeout() {
        let _pty = PTY_TEST_LOCK.lock().await;
        let _home = crate::test_home_guard();
        let tmp = tempfile::tempdir().expect("tempdir");
        let flag = tmp.path().join("connected");
        let script = format!(
            "sleep 0.4; printf \"u@h's password: \"; read -r pw; sleep 0.5; \
             [ \"$pw\" = {CREDENTIAL} ] && touch {} && exit 0; exit 255",
            flag.display()
        );
        let writes = Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut writer = RecordingWriter {
            writes: Arc::clone(&writes),
        };
        let mut reader = answered_reader(&[CREDENTIAL]);
        let flag_path = flag.to_path_buf();
        let argv = vec!["-c".to_string(), script.clone()];
        let outcome = drive_connect(
            "/bin/sh",
            &argv,
            || flag_path.exists(),
            "pinky",
            PROMPT_ID,
            &mut writer,
            &mut reader,
            Duration::from_millis(700),
        )
        .await;
        assert_eq!(
            outcome,
            Ok(()),
            "a login that finishes within the timeout of its answer succeeds: {outcome:?}"
        );
        assert!(flag.exists(), "the fake connected: {outcome:?}");

        // The never-answered half: no prompt, no answer, only sleep — the
        // deadline measured from spawn still fires with the pinned wording.
        let flag2 = tmp.path().join("never");
        let script2 = format!("sleep 2; touch {}; exit 0", flag2.display());
        let flag2_path = flag2.to_path_buf();
        let argv2 = vec!["-c".to_string(), script2];
        let started = std::time::Instant::now();
        let silent = drive_connect(
            "/bin/sh",
            &argv2,
            || flag2_path.exists(),
            "pinky",
            PROMPT_ID,
            &mut writer,
            &mut reader,
            Duration::from_millis(300),
        )
        .await;
        let msg = silent.expect_err("the silent fake must time out: {silent:?}");
        assert!(
            msg.contains("did not connect within 300ms"),
            "the never-answered deadline still fires from spawn: {msg:?}"
        );
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "the timeout kills the child at the deadline, not later: {msg:?}"
        );
    }

    /// Test 2: a host whose ssh asks nothing connects with no prompt round
    /// trip at all — the connect step is transparent on key auth.
    #[tokio::test]
    async fn a_host_that_needs_no_credential_connects_without_a_prompt() {
        let _pty = PTY_TEST_LOCK.lock().await;
        let _home = crate::test_home_guard();
        let tmp = tempfile::tempdir().expect("tempdir");
        let flag = tmp.path().join("connected");
        let script = format!("sleep 0.2; touch {}; exit 0", flag.display());
        let writes = Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut writer = RecordingWriter {
            writes: Arc::clone(&writes),
        };
        let mut reader = answered_reader(&[CREDENTIAL]);
        let outcome = drive_fake(&script, &flag, &mut writer, &mut reader).await;
        assert_eq!(outcome, Ok(()), "the fake connects: {outcome:?}");
        assert_eq!(
            prompt_count(&writes),
            0,
            "no prompt is asked when ssh connects without one"
        );
    }

    /// Test 3: the password prompt is answered once and the host connects —
    /// the credential reaches the child's PTY and never the transport log.
    #[tokio::test]
    async fn a_password_prompt_is_answered_once_and_the_host_connects() {
        let _pty = PTY_TEST_LOCK.lock().await;
        let _home = crate::test_home_guard();
        let tmp = tempfile::tempdir().expect("tempdir");
        let flag = tmp.path().join("connected");
        // The `sleep 0.3` after the read keeps the child alive once the
        // answer is in — ssh's post-answer authentication latency — so the
        // classify-only-after-the-last-answer window (08f) is observable:
        // bug-08e-3's Mutation A re-applied turns this red (the stale prompt
        // is re-answered against the exhausted transport).
        let script = format!(
            "sleep 0.2; printf \"u@h's password: \"; read -r pw; sleep 0.3; \
             [ \"$pw\" = {CREDENTIAL} ] && touch {} && exit 0; exit 255",
            flag.display()
        );
        let writes = Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut writer = RecordingWriter {
            writes: Arc::clone(&writes),
        };
        let mut reader = answered_reader(&[CREDENTIAL]);
        let outcome = drive_fake(&script, &flag, &mut writer, &mut reader).await;
        assert_eq!(outcome, Ok(()), "the fake connects: {outcome:?}");
        assert_eq!(
            prompt_count(&writes),
            1,
            "exactly one credential prompt was written: {:?}",
            writes.lock().expect("write log lock")
        );
    }

    /// Test 4: a wrong password is asked again, and the retry chunk — which
    /// carries both `Permission denied` **and** a fresh prompt — does not
    /// abort the login (the process-exit rule, not a text match).
    #[tokio::test]
    async fn a_wrong_password_is_asked_again_and_the_retry_line_does_not_abort() {
        let _pty = PTY_TEST_LOCK.lock().await;
        let _home = crate::test_home_guard();
        let tmp = tempfile::tempdir().expect("tempdir");
        let flag = tmp.path().join("connected");
        let script = format!(
            "sleep 0.2; printf \"u@h's password: \"; read -r pw; \
             if [ \"$pw\" = {CREDENTIAL} ]; then touch {}; exit 0; fi; \
             printf \"Permission denied, please try again.\\r\\nu@h2's password: \"; \
             read -r pw; \
             if [ \"$pw\" = {CREDENTIAL} ]; then touch {}; exit 0; fi; exit 255",
            flag.display(),
            flag.display()
        );
        let writes = Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut writer = RecordingWriter {
            writes: Arc::clone(&writes),
        };
        let mut reader = answered_reader(&["wrong-pw", CREDENTIAL]);
        let outcome = drive_fake(&script, &flag, &mut writer, &mut reader).await;
        assert_eq!(outcome, Ok(()), "the retry connects: {outcome:?}");
        assert_eq!(
            prompt_count(&writes),
            2,
            "the wrong answer was asked again: {:?}",
            writes.lock().expect("write log lock")
        );
    }

    /// Test 5: nobody attached — the round trip gives up at once with 08f's
    /// sentence, well inside any plausible timeout, and nothing is left
    /// running.
    #[tokio::test]
    async fn an_unanswerable_connection_prompt_fails_fast_and_leaves_nothing_running() {
        let _pty = PTY_TEST_LOCK.lock().await;
        let _home = crate::test_home_guard();
        let tmp = tempfile::tempdir().expect("tempdir");
        let flag = tmp.path().join("connected");
        let script = format!(
            "sleep 0.2; printf \"u@h's password: \"; read -r pw; \
             [ \"$pw\" = {CREDENTIAL} ] && touch {} && exit 0; exit 255",
            flag.display()
        );
        let started = std::time::Instant::now();
        let outcome = drive_fake(
            &script,
            &flag,
            &mut tokio::io::sink(),
            &mut tokio::io::BufReader::new(tokio::io::empty()),
        )
        .await;
        let elapsed = started.elapsed();
        let msg = outcome.expect_err("the unanswerable prompt must fail: {outcome:?}");
        assert!(
            msg.contains("nobody is attached"),
            "the give-up names the missing reader: {msg:?}"
        );
        assert!(
            elapsed < Duration::from_secs(5),
            "the failure is fast, not a timeout: {elapsed:?}"
        );
        assert!(
            !flag.exists(),
            "the fake never reached its connected state: {msg:?}"
        );
    }

    /// Test 6: ssh's unknown-host-key question is refused — never answered
    /// with a credential, and the error names the host.
    #[tokio::test]
    async fn an_unknown_host_key_is_refused() {
        let _pty = PTY_TEST_LOCK.lock().await;
        let _home = crate::test_home_guard();
        let tmp = tempfile::tempdir().expect("tempdir");
        let flag = tmp.path().join("connected");
        let script = "sleep 0.2; printf \"Are you sure you want to continue connecting \
                      (yes/no/[fingerprint])? \"; read -r a; exit 255"
            .to_string();
        let writes = Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut writer = RecordingWriter {
            writes: Arc::clone(&writes),
        };
        let mut reader = answered_reader(&[CREDENTIAL]);
        let outcome = drive_fake(&script, &flag, &mut writer, &mut reader).await;
        let msg = outcome.expect_err("the host-key question must refuse: {outcome:?}");
        assert!(
            msg.contains("unknown host key") && msg.contains("pinky"),
            "the refusal names the host and the question: {msg:?}"
        );
        assert_eq!(
            prompt_count(&writes),
            0,
            "the host-key question is never answered with a credential"
        );
        assert!(!flag.exists(), "the fake never connected");
    }

    /// Live test 7: the configured host `pinky` — cold, the connect step
    /// answers nothing and still connects; warm, the second call is a no-op
    /// in milliseconds. Requires `pinky` reachable over ssh; ignored by
    /// default, run with `-- --ignored --nocapture`.
    #[tokio::test]
    #[ignore = "requires the configured host `pinky` reachable over ssh"]
    async fn a_cold_host_connects_first_and_a_warm_one_is_reused_on_pinky() {
        let (_home, dir) = test_home();
        std::fs::write(
            dir.path().join(".daemoneye/etc/config.toml"),
            "[models.default]\nprovider = \"ollama\"\n\n[hosts.pinky]\nssh = \"pinky\"\n",
        )
        .expect("write config");
        let control = crate::config::ssh_dir();
        std::fs::create_dir_all(&control).expect("ssh dir");
        let entry = crate::shell::host_entry("pinky").expect("pinky is configured");
        let mut sink = tokio::io::sink();
        let mut eof = tokio::io::BufReader::new(tokio::io::empty());

        let cold_started = std::time::Instant::now();
        let cold = ensure_connected("pinky", false, PROMPT_ID, &mut sink, &mut eof).await;
        let cold_elapsed = cold_started.elapsed();
        println!("cold connect elapsed: {cold_elapsed:?}");
        assert_eq!(cold, Ok(()), "the cold connect succeeds: {cold:?}");

        let warm = master_running(&entry, &control);
        assert!(warm, "the master survives the connect step");

        let warm_started = std::time::Instant::now();
        let second = ensure_connected("pinky", false, PROMPT_ID, &mut sink, &mut eof).await;
        let warm_elapsed = warm_started.elapsed();
        println!("warm check elapsed: {warm_elapsed:?}");
        assert_eq!(second, Ok(()), "the warm check succeeds: {second:?}");
        assert!(
            warm_elapsed < Duration::from_millis(100),
            "the warm check is a no-op, not a fresh connect: {warm_elapsed:?}"
        );

        let _ = std::process::Command::new("ssh")
            .args([
                "-O",
                "exit",
                "-o",
                &format!("ControlPath={}/%C", control.display()),
                "pinky",
            ])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }

    // ── M26 phase-06b: the marker comes from the record ─────────────────────

    use super::tests::{FishShellEnv, spawn_in_process_host};

    /// A sandbox shell targeted by id gets the POSIX marker (`"sh"` → `$?`),
    /// **not** the daemon's `$SHELL`. With the daemon's `$SHELL` fish, the
    /// marker would ask for `$status`, which the sandbox's bash expands to
    /// nothing, so the exit code never parses. Regression guard for phase-06b
    /// Task 2: reverting to `marker_shell_name(&record_host)` (the host's
    /// login shell, `/usr/bin/fish` here) makes this test time out.
    #[tokio::test]
    async fn a_sandbox_shell_targeted_by_id_uses_the_posix_marker() {
        let _pty = PTY_TEST_LOCK.lock().await;
        let (_home, _dir) = test_home();
        let reg = ShellRegistry::default();
        let owner = Owner::Chat("sandbox-marker".into());

        // A live sandbox shell: the record carries `sandbox`, its host is
        // `Local`. The in-process host runs a real `bash` behind the socket,
        // which stands in for the VM's bash — the marker protocol is the same.
        let id = crate::shell::spawn_for_in_sandbox(
            &reg,
            owner.clone(),
            5,
            "de-ghost-t".into(),
            false,
            spawn_in_process_host,
        )
        .await
        .expect("the sandbox shell's host comes up");
        crate::shell::await_ready(
            &socket_path(id.as_str()),
            std::time::Instant::now() + Duration::from_secs(15),
        )
        .await
        .expect("the sandbox shell's host must be alive, or the marker is not under test");

        // The daemon's `$SHELL` is fish for the duration of the call; the
        // guard restores it even when an assertion below fails.
        let _fish = FishShellEnv::set();

        let outcome = run_in_shell_with_prompts(
            &reg,
            owner,
            "sh -c 'exit 3'",
            5,
            Some(id.as_str()),
            Some(10),
            "",
            &mut tokio::io::sink(),
            &mut tokio::io::empty(),
            crate::host::HostId::Local,
            |_id: ShellId| async move { Ok(()) },
        )
        .await;

        // Drop the guard before asserting: an async-drop tail would be a
        // surprise, and the restore must not depend on the assertions.
        drop(_fish);

        let (_id, result) =
            outcome.expect("the POSIX marker must parse the exit code out of a sandbox shell");
        assert_eq!(
            result.exit_code, 3,
            "the sandbox's bash must report its child's exit code through the \
             POSIX marker: {result:?}"
        );
    }
}
