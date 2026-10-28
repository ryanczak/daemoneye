//! Credentials and connecting: the sudo retry bookkeeping, the credential
//! prompt round trip, the cold-host connect step (`drive_connect`,
//! `ensure_connected`) and `run_sudo_command`.

use super::*;

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
pub(in super::super) async fn ensure_connected<W, R>(
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
pub(super) async fn run_sudo_command<'p, W, R>(
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
