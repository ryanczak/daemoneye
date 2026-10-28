//! `read_shell_screen`: the live viewport as of the call, and the screen waits (`wait_screen`, `wait_screen_change`).

use super::*;

// ---------------------------------------------------------------------------
// read_shell_screen (M28 phase 01c) — the live viewport, as of the call
// ---------------------------------------------------------------------------

/// The socket round-trip budget for one screen read: the same 5 s the other
/// one-shot shell probes use. The answer is a render, not the program
/// reacting, so it is short. `pub(crate)`: the interactive launch
/// (`shell_exec.rs`) reads the settled screen through it too.
pub(crate) const SCREEN_TIMEOUT: Duration = Duration::from_secs(5);

/// The default and clamped bounds for a screen wait (`timeout_secs` /
/// `settle_ms`). The default 30 s covers a Claude Code turn; 300 s is the
/// ceiling so a stuck wait still returns this turn. A settle below 50 ms
/// would race the host's own render; above 5 s it is a timeout by another
/// name.
pub(crate) fn wait_bounds(
    timeout_secs: Option<u64>,
    settle_ms: Option<u64>,
) -> (Duration, Option<Duration>) {
    const DEFAULT_TIMEOUT: u64 = 30;
    const MAX_TIMEOUT: u64 = 300;
    const MIN_SETTLE_MS: u64 = 50;
    const MAX_SETTLE_MS: u64 = 5000;
    let timeout = Duration::from_secs(
        timeout_secs
            .unwrap_or(DEFAULT_TIMEOUT)
            .clamp(1, MAX_TIMEOUT),
    );
    let settle = settle_ms
        .filter(|n| *n > 0)
        .map(|n| Duration::from_millis(n.clamp(MIN_SETTLE_MS, MAX_SETTLE_MS)));
    (timeout, settle)
}

/// One rendered read of a shell's screen, as the host answered it.
struct ScreenRead {
    text: String,
    rows: u16,
    cols: u16,
    cursor: (u16, u16),
    alt_screen: bool,
}

/// The poll interval between two screen reads inside `wait_screen`. Short
/// enough to notice a prompt appearing promptly, long enough not to spin the
/// host's renderer.
const WAIT_POLL: Duration = Duration::from_millis(100);

/// How a screen wait ended. A wait that runs out is a **result**, not an
/// error — a fast-redrawing screen (`top -d 0.2`) never stops redrawing and
/// that is a legitimate answer — so every branch but `Failed` renders the
/// screen.
#[derive(Debug)]
pub(super) enum WaitOutcome {
    /// `wait_for` matched the rendered text; `elapsed` is how long the wait
    /// took.
    Matched { elapsed: Duration },
    /// Two reads `settle` apart returned the same text; `elapsed` is how long
    /// the wait took.
    Settled { elapsed: Duration },
    /// `wait_for` never matched within the timeout; the deadline is named.
    NotMatched { timeout: Duration },
    /// The screen never went quiet within the timeout; the deadline is named.
    NotSettled { timeout: Duration },
    /// The read itself failed; the error sentence is passed through and the
    /// caller shows no screen at all.
    Failed(String),
}

/// Pure async core of the screen wait: poll `read` (which returns the
/// rendered, unannotated visible text) until the pattern appears, the screen
/// settles, or `timeout` runs out. With `wait_for` set the pattern is waited
/// for first and the settle second; one `timeout` bounds the whole wait.
/// `poll` and `settle` are injected so tests stay hermetic and fast.
///
/// Quiescence is measured on the **text** — two consecutive reads `settle`
/// apart that are equal — never on the annotated form or the byte stream, so
/// a cursor blink or an SGR-only redraw counts as settled.
pub(super) async fn wait_screen<R, F>(
    mut read: R,
    wait_for: Option<&regex::Regex>,
    settle: Option<Duration>,
    timeout: Duration,
    poll: Duration,
) -> WaitOutcome
where
    R: FnMut() -> F,
    F: Future<Output = Result<String, String>>,
{
    let started = Instant::now();
    let matched = |t: &str| wait_for.is_none_or(|re| re.is_match(t));
    if wait_for.is_some() {
        loop {
            if Instant::now() >= started + timeout {
                return WaitOutcome::NotMatched { timeout };
            }
            match read().await {
                Ok(text) if matched(&text) => {
                    if settle.is_none() {
                        return WaitOutcome::Matched {
                            elapsed: started.elapsed(),
                        };
                    }
                    break;
                }
                Ok(_) => {}
                Err(msg) => return WaitOutcome::Failed(msg),
            }
            tokio::time::sleep(poll).await;
        }
    }
    let Some(settle) = settle else {
        return WaitOutcome::Failed(String::new());
    };
    loop {
        let first = match read().await {
            Ok(text) => text,
            Err(msg) => return WaitOutcome::Failed(msg),
        };
        if Instant::now() >= started + timeout {
            return WaitOutcome::NotSettled { timeout };
        }
        tokio::time::sleep(settle).await;
        if Instant::now() >= started + timeout {
            return WaitOutcome::NotSettled { timeout };
        }
        let second = match read().await {
            Ok(text) => text,
            Err(msg) => return WaitOutcome::Failed(msg),
        };
        if second == first && matched(&second) {
            return WaitOutcome::Settled {
                elapsed: started.elapsed(),
            };
        }
        if Instant::now() >= started + timeout {
            return WaitOutcome::NotSettled { timeout };
        }
        tokio::time::sleep(poll).await;
    }
}

/// Pure async core of the change wait: poll `read` (the rendered, unannotated
/// visible text) until a read's text differs from `baseline`, or `timeout`
/// runs out. True means the program drew something; false means the deadline
/// passed with the screen exactly as the launch left it. A read that **fails**
/// counts as no change — `wait_screen` reports a failed read to its caller,
/// but this wait runs as the prefix of a launch whose settled read still has
/// to happen, so a flaky poll read must not abort the launch; the settled
/// read surfaces the failure itself if the shell is really gone.
/// `pub(crate)`: `launch_interactive` (`shell_exec.rs`) is the caller, via
/// the `knowledge` re-export, the same route `read_shell_screen_in` takes.
pub(crate) async fn wait_screen_change<R, F>(
    mut read: R,
    baseline: &str,
    timeout: Duration,
    poll: Duration,
) -> bool
where
    R: FnMut() -> F,
    F: Future<Output = Result<String, String>>,
{
    let started = Instant::now();
    loop {
        if Instant::now() >= started + timeout {
            return false;
        }
        match read().await {
            Ok(text) if text != baseline => return true,
            Ok(_) => {}
            Err(_) => {}
        }
        tokio::time::sleep(poll).await;
    }
}

/// Pure: the one note line that opens a waited read's output, so the model
/// knows the screen it is about to see arrived after a wait and how.
pub(super) fn wait_note(outcome: &WaitOutcome, pattern: Option<&regex::Regex>) -> Option<String> {
    let s = |d: Duration| format!("{:.1}", d.as_secs_f64());
    let pat = pattern.map_or(String::new(), |re| re.as_str().to_string());
    match outcome {
        WaitOutcome::Matched { elapsed } => Some(format!(
            "[waited {} s: the screen shows /{pat}/]",
            s(*elapsed)
        )),
        WaitOutcome::Settled { elapsed } => {
            Some(format!("[waited {} s: the screen settled]", s(*elapsed)))
        }
        WaitOutcome::NotMatched { timeout } => Some(format!(
            "[did not see /{pat}/ within {} s — showing the screen as it is now]",
            s(*timeout)
        )),
        WaitOutcome::NotSettled { timeout } => Some(format!(
            "[did not settle within {} s — the screen is still changing; \
             use wait_for for a pattern that marks the state you need]",
            s(*timeout)
        )),
        WaitOutcome::Failed(_) => None,
    }
}

/// Return a shell's rendered screen with a header naming what holds its
/// terminal, masked. The public form uses the daemon's registry and the
/// standard timeout; `read_shell_screen_in` takes both.
#[allow(clippy::too_many_arguments)]
pub async fn read_shell_screen(
    shell: &str,
    lines: Option<u64>,
    scrollback: Option<u16>,
    pattern: Option<&str>,
    annotated: bool,
    wait_for: Option<&str>,
    settle_ms: Option<u64>,
    timeout_secs: Option<u64>,
) -> String {
    read_shell_screen_in(
        crate::shell::registry(),
        shell,
        lines,
        scrollback,
        pattern,
        annotated,
        wait_for,
        settle_ms,
        timeout_secs,
        SCREEN_TIMEOUT,
    )
    .await
}

/// `read_shell_screen` against an injected registry and timeout.
///
/// `pub(crate)` rather than `pub(super)`: `shell_exec.rs`'s interactive
/// launch reads the screen through it, and `knowledge/mod.rs` re-exports it
/// — a `pub(super)` item cannot be given wider visibility on re-export.
///
/// Reads the visible screen on its own, then — only when `scrollback` is
/// asked for — reads again with the scrollback rows and keeps the part above
/// the visible screen. The host joins the two with a plain newline, so the
/// boundary is only recoverable this way, and keeping it is what lets
/// `lines` count visible rows exactly and the result label both sections.
///
/// `wait_for` / `settle_ms` / `timeout_secs` add a wait before the read: the
/// note line naming how the wait ended opens the output. A wait that runs
/// out is a result, not an error — the screen as it now stands is still the
/// answer to "what does this show?".
#[allow(clippy::too_many_arguments)]
pub(crate) async fn read_shell_screen_in(
    reg: &ShellRegistry,
    shell: &str,
    lines: Option<u64>,
    scrollback: Option<u16>,
    pattern: Option<&str>,
    annotated: bool,
    wait_for: Option<&str>,
    settle_ms: Option<u64>,
    timeout_secs: Option<u64>,
    timeout: Duration,
) -> String {
    if !with_shells(reg, |store| store.contains_key(shell)) {
        return unknown_shell_error(reg, shell);
    }
    let pattern_re = match pattern {
        Some(p) => match regex::RegexBuilder::new(p).size_limit(1 << 20).build() {
            Ok(re) => Some(re),
            Err(e) => {
                return format!(
                    "Error: invalid pattern regex {p:?}: {e}. Fix the expression and \
                     call again; the pattern is applied as a regex, not a literal."
                );
            }
        },
        None => None,
    };
    let wait_re = match wait_for {
        Some(p) => match regex::RegexBuilder::new(p).size_limit(1 << 20).build() {
            Ok(re) => Some(re),
            Err(e) => {
                return format!(
                    "Error: invalid wait_for regex {p:?}: {e}. Fix the expression and \
                     call again; wait_for is applied as a regex to the rendered screen, \
                     not a literal."
                );
            }
        },
        None => None,
    };
    let (wait_timeout, settle) = wait_bounds(timeout_secs, settle_ms);
    let wait_outcome = if wait_re.is_none() && settle.is_none() {
        None
    } else {
        let sock = socket_path(shell);
        let read = || async {
            match read_screen(&sock, shell, false, 0, SCREEN_TIMEOUT).await {
                Ok(r) => Ok(r.text),
                Err(msg) => Err(msg),
            }
        };
        Some(wait_screen(read, wait_re.as_ref(), settle, wait_timeout, WAIT_POLL).await)
    };
    let note = wait_outcome
        .as_ref()
        .and_then(|o| wait_note(o, wait_re.as_ref()));
    if let Some(WaitOutcome::Failed(msg)) = wait_outcome.as_ref() {
        return msg.clone();
    }
    let sock = socket_path(shell);
    let visible = match read_screen(&sock, shell, annotated, 0, timeout).await {
        Ok(read) => read,
        Err(msg) => return msg,
    };
    let head = match scrollback {
        Some(n) if n > 0 => match read_screen(&sock, shell, annotated, n, timeout).await {
            Ok(read) => scrollback_part(&read.text, &visible.text),
            Err(msg) => return msg,
        },
        _ => String::new(),
    };
    let status = crate::shell::status(&sock, timeout).await.ok();
    let last_line = visible
        .text
        .lines()
        .rfind(|l| !l.trim().is_empty())
        .map(str::to_string);
    let observed_at = crate::shell::now_unix();
    let foreground = with_shells(reg, |store| {
        store.get_mut(shell).map(|record| {
            let fg = crate::shell::classify_record(record, status.as_ref(), last_line.as_deref());
            record.last_foreground = Some((fg.clone(), observed_at));
            fg
        })
    });
    let Some(foreground) = foreground else {
        return unknown_shell_error(reg, shell);
    };
    let out = format_screen_read(
        shell,
        &foreground,
        &visible,
        &head,
        lines,
        pattern_re.as_ref(),
    );
    let out = match note {
        Some(line) => format!("{line}\n{out}"),
        None => out,
    };
    mask_sensitive(&out)
}

/// One `Screen` round-trip, or the error sentence the model reads. A host
/// that does not answer is an error naming the shell — never a blank screen,
/// which would read as "the shell shows nothing".
async fn read_screen(
    sock: &std::path::Path,
    shell: &str,
    annotated: bool,
    scrollback: u16,
    timeout: Duration,
) -> Result<ScreenRead, String> {
    match crate::shell::screen(sock, annotated, scrollback, timeout).await {
        Ok(crate::shell::ShellResponse::ScreenText {
            text,
            rows,
            cols,
            cursor,
            alt_screen,
            bracketed_paste: _,
            application_cursor: _,
        }) => Ok(ScreenRead {
            text,
            rows,
            cols,
            cursor,
            alt_screen,
        }),
        Ok(crate::shell::ShellResponse::Error { message }) => Err(format!(
            "Error: shell {shell} could not render its screen: {message}. A shell \
             host started before this daemon version has no screen to read; \
             read_shell shows its recorded output instead."
        )),
        Ok(other) => Err(format!(
            "Error: shell {shell} answered the screen read with an unexpected frame \
             ({other:?}), so there is no screen to show."
        )),
        Err(e) => Err(format!(
            "Error: shell {shell} did not answer the screen read: {}. Its host may \
             be gone; call list_shells.",
            format_err_chain(&e)
        )),
    }
}

/// The rendered, **unannotated** visible text of `shell`'s screen — what a
/// person at that terminal sees now, without the header or the wait note.
/// `pub(crate)`: the interactive launch (`shell_exec.rs`) reads it as the
/// baseline its change wait compares against.
pub(crate) async fn screen_text(shell: &str, timeout: Duration) -> Result<String, String> {
    read_screen(&socket_path(shell), shell, false, 0, timeout)
        .await
        .map(|r| r.text)
}

/// Pure: the rows above the visible screen in a scrollback read. The host
/// writes `<scrollback>\n<visible>`, so the scrollback is what precedes the
/// visible text. If the screen changed between the two reads the suffix no
/// longer matches, and the whole later read is returned as scrollback rather
/// than guessing at a boundary.
pub(super) fn scrollback_part(with_scrollback: &str, visible: &str) -> String {
    if visible.is_empty() {
        return with_scrollback.to_string();
    }
    match with_scrollback.strip_suffix(visible) {
        Some(head) => head.strip_suffix('\n').unwrap_or(head).to_string(),
        None => with_scrollback.to_string(),
    }
}

/// Pure: the result text. Header, then the scrollback section when there is
/// one, then the visible rows tailed by `lines`; `pattern` filters both.
fn format_screen_read(
    shell: &str,
    foreground: &crate::shell::Foreground,
    visible: &ScreenRead,
    head: &str,
    lines: Option<u64>,
    pattern: Option<&regex::Regex>,
) -> String {
    let keep = |row: &&str| pattern.is_none_or(|re| re.is_match(row));
    let mut out = format!(
        "Shell {shell} — {foreground} — {}x{}, cursor {},{}{}{}:\n",
        visible.rows,
        visible.cols,
        visible.cursor.0,
        visible.cursor.1,
        if visible.alt_screen {
            ", alt screen"
        } else {
            ""
        },
        match pattern {
            Some(re) => format!(", rows matching /{}/", re.as_str()),
            None => String::new(),
        },
    );
    if !head.is_empty() {
        let rows: Vec<&str> = head.lines().filter(keep).collect();
        out.push_str(&format!("--- scrollback ({} rows) ---\n", rows.len()));
        for row in rows {
            out.push_str(row);
            out.push('\n');
        }
        out.push_str("--- screen ---\n");
    }
    let rows: Vec<&str> = visible.text.lines().filter(keep).collect();
    let start = match lines {
        Some(n) => rows
            .len()
            .saturating_sub(usize::try_from(n).unwrap_or(usize::MAX)),
        None => 0,
    };
    for row in &rows[start..] {
        out.push_str(row);
        out.push('\n');
    }
    out
}
