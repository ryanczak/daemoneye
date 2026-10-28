//! `send_input`: composing typed text and named keys and writing them into a live shell.

use super::*;

// ---------------------------------------------------------------------------
// send_input (M22 phase-05)
// ---------------------------------------------------------------------------

/// How long `send_input` may wait for the host to acknowledge the write.
/// Same reasoning as `SIGNAL_TIMEOUT`, and the same value: the ack is the
/// socket round-trip, not the program reacting.
const INPUT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// The words the approval panel shows about what the shell is doing — the
/// `input_target_note` sentence for the `send_input` panel's `foreground:`
/// line.
///
/// The probe's [`Foreground`] classification, not the coarser `ShellState`:
/// `Running` collapsed `claude` in raw mode and `grep` reading a pipe into
/// one sentence, and said nothing about the screen the keys land on. The
/// wording per case is the guard table's (D9) chat column — the guard
/// informs and the human decides.
pub fn input_target_note(fg: &crate::shell::Foreground) -> String {
    match fg {
        crate::shell::Foreground::Prompt => {
            "shell is at its prompt, so the input WILL be run as a shell command, \
             without run_terminal_command's approval, capture or exit code"
                .to_string()
        }
        crate::shell::Foreground::Program { cmd, tui: false } => {
            format!("the input goes to {cmd}'s stdin and is NOT executed as a shell command")
        }
        crate::shell::Foreground::Program { cmd, tui: true } => {
            format!("the input goes to the raw-mode TUI {cmd} as keystrokes")
        }
        crate::shell::Foreground::Paused { cmd } => {
            format!("{cmd}'s job is stopped, so the input will sit unread until it resumes")
        }
        crate::shell::Foreground::Exited | crate::shell::Foreground::Unknown => {
            "the shell may be gone, so nothing may read the input".to_string()
        }
    }
}

/// The last `rows` rows of a screen read, trailing blank rows dropped and
/// every line passed through `mask_sensitive` — the panel shows the user
/// what the keys will land on without leaking what the screen happens to
/// hold. Pure: the caller supplies the screen text.
pub fn screen_tail(text: &str, rows: usize) -> String {
    let kept: Vec<&str> = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .collect::<Vec<_>>();
    let start = kept.len().saturating_sub(rows);
    kept[start..]
        .iter()
        .map(|l| mask_sensitive(l))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The full `send_input` approval panel: what goes out, then the
/// `foreground:` line naming what holds the shell, then — only for a TUI —
/// the masked last rows of its screen. `screen` is the `ScreenText.text` the
/// executor arm already read; `None` (an unreadable screen) shows nothing
/// rather than a guess.
pub fn input_panel(
    shell: &str,
    text: &str,
    input: &ComposedInput,
    fg: &crate::shell::Foreground,
    screen: Option<&str>,
) -> String {
    let mut out = format!("send_input into shell {shell}: {text:?}");
    if !input.keys.is_empty() {
        out.push_str(&format!(" keys [{}]", input.keys.join(", ")));
    }
    if input.bracketed {
        out.push_str(" as a bracketed paste");
    }
    if input.enter {
        out.push_str(" (Enter sent)");
    }
    out.push_str(&format!(" — foreground: {fg} — {}", input_target_note(fg)));
    if let crate::shell::Foreground::Program { tui: true, .. } = fg
        && let Some(screen) = screen
    {
        out.push_str(&format!(
            "\nscreen (last 5 rows):\n{}",
            screen_tail(screen, 5)
        ));
    }
    out
}

/// The bytes `send_input` sends for one call, and the facts the result
/// sentence and the approval panel name about them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComposedInput {
    /// Everything that reaches the shell: the text (bracketed when the paste
    /// rule brackets it), then the encoded keys, then the Enter `\r`.
    pub bytes: Vec<u8>,
    /// Whether the text went out wrapped in the shell's paste brackets.
    pub bracketed: bool,
    /// The key names, verbatim from the call.
    pub keys: Vec<String>,
    /// The key names that went out as CSI u — the form a program may ignore,
    /// so the result tells the model to check the screen.
    pub csi_u: Vec<String>,
    /// Whether a trailing Enter `\r` is part of `bytes`.
    pub enter: bool,
}

/// Compose `send_input`'s bytes: `paste_bytes(text)`, then
/// `encode_keys(keys)`, then the Enter `\r`.
///
/// Errors pass the encoder's and the paste rule's messages through
/// unchanged; the one own error is a call with nothing in it — empty text,
/// no keys, no Enter — which would be a round trip that sent zero bytes.
/// Nothing has been sent when this returns `Err`; the executor's arm turns
/// it into the tool result before any approval prompt.
pub fn compose_input(
    text: &str,
    paste: Option<bool>,
    keys: &[String],
    enter: bool,
    modes: crate::shell::ShellModes,
) -> anyhow::Result<ComposedInput> {
    if text.is_empty() && keys.is_empty() && !enter {
        return Err(anyhow::anyhow!(
            "nothing to send: the text is empty, no keys are named and enter is \
             false — name what the shell should receive"
        ));
    }
    let (text_bytes, bracketed) = crate::shell::paste_bytes(text, paste, modes)?;
    let encoded = crate::shell::encode_keys(keys, modes)?;
    let mut bytes = text_bytes;
    bytes.extend_from_slice(&encoded.bytes);
    if enter {
        bytes.push(b'\r');
    }
    Ok(ComposedInput {
        bytes,
        bracketed,
        keys: keys.to_vec(),
        csi_u: encoded.csi_u,
        enter,
    })
}

/// Write `text` (plus a newline when `enter`) into one of `caller`'s live PTY
/// shells and return the sentence the model reads.
///
/// The public form uses the daemon's registry and the standard input timeout;
/// the `_with` form takes the registry, the timeout and the send step,
/// mirroring the `open_shell` / `open_shell_with` split. Injecting `send` is
/// what makes the newline and the absolute-wrong-target assertions exact
/// checks on the bytes rather than inferences from a message.
pub async fn send_input(shell: &str, input: &ComposedInput, caller: &Owner) -> String {
    send_input_with(
        crate::shell::registry(),
        shell,
        input,
        caller,
        INPUT_TIMEOUT,
        |bytes| async move {
            crate::shell::send_input(&socket_path(shell), &bytes, INPUT_TIMEOUT).await
        },
    )
    .await
}

/// `send_input` with the registry, timeout and send step injected.
pub(super) async fn send_input_with<S, Fut>(
    reg: &ShellRegistry,
    shell: &str,
    input: &ComposedInput,
    caller: &Owner,
    input_timeout: Duration,
    send: S,
) -> String
where
    S: Fn(Vec<u8>) -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<()>>,
{
    let _ = input_timeout;
    // Ownership first, and on `Owner::key()` rather than the inner string: a
    // chat session and a ghost job can share an id string, and the kinds are
    // what the cap groups, so they are what the owner check must compare.
    let owner_in_registry: Option<String> = with_shells(reg, |store| {
        store.get(shell).map(|record| record.owner.key())
    });
    let Some(owner_key) = owner_in_registry else {
        let live = live_shell_ids(reg);
        return if live.is_empty() {
            format!(
                "Error: no shell with id {shell:?} is in the registry (no shells are \
                 live — call list_shells to confirm)."
            )
        } else {
            format!(
                "Error: no shell with id {shell:?} is in the registry. Live shell ids: \
                 {}. Call list_shells to refresh.",
                live.join(", ")
            )
        };
    };
    if !caller.may_drive(&owner_key) {
        return format!(
            "Error: shell {shell} is not yours (it belongs to {owner_key}), so this \
             session cannot type into it. If it needs input, the session that \
             owns it does that; list_shells shows which shells are whose."
        );
    }
    if let Err(e) = send(input.bytes.clone()).await {
        return format!(
            "Error: shell {shell} did not accept the input, so nothing was typed: \
             {}. The write is not half-delivered (the host acks the whole frame \
             or rejects it); check the shell with list_shells and retry.",
            format_err_chain(&e)
        );
    }
    let n = input.bytes.len();
    let mut tail = if input.enter {
        ", Enter sent".to_string()
    } else {
        ", no Enter".to_string()
    };
    if !input.keys.is_empty() {
        tail.push_str(&format!(", keys: {}", input.keys.join(", ")));
    }
    if input.bracketed {
        tail.push_str(", as a bracketed paste");
    }
    let mut out = format!(
        "Sent {} byte{} to shell {}{tail}. Read the effect with read_shell, \
         read_shell_screen or watch_shell — this tool returns no output and \
         does not wait for the program to react.",
        n,
        if n == 1 { "" } else { "s" },
        shell,
    );
    if !input.csi_u.is_empty() {
        out.push_str(&format!(
            " {} went out as CSI u (ESC [<codepoint>;<modifier> u), which some \
             programs ignore — read the screen with read_shell_screen to see \
             what the program made of them.",
            input.csi_u.join(", ")
        ));
    }
    out
}
