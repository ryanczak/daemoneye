use anyhow::Result;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::cli::input::*;
use crate::cli::render::*;
use crate::cli::render_ratatui::RatatuiRendererStdout;
use crate::config::Config;

use super::approval::SessionApproval;
use super::chat_presence;
use super::ipc_client::new_session_id;
use super::pane::resolve_target_pane;
use super::slash;
use super::stream::{AskTmuxCtx, QueryArgs, RatatuiQueryCtx, TokenCtx, ask_with_session_ratatui};

/// `/log` and its log id: `Some(Some(id))` for `/log <id>`, `Some(None)` for a
/// bare `/log`, `None` for anything else — `/logs` included.
fn log_command(query: &str) -> Option<Option<&str>> {
    let mut words = query.split_whitespace();
    if words.next() != Some("/log") {
        return None;
    }
    Some(words.next())
}

/// `/attach` and its shell id: `Some(Some(id))` for `/attach <id>`,
/// `Some(None)` for a bare `/attach`, `None` for anything else.
fn attach_command(query: &str) -> Option<Option<&str>> {
    let mut words = query.split_whitespace();
    if words.next() != Some("/attach") {
        return None;
    }
    Some(words.next())
}

/// `/shell` and its host: `Some(None)` for a bare `/shell` (the daemon
/// host), `Some(Some(host))` for `/shell <host>`, `None` for anything else —
/// `/shells` included.
fn shell_command(query: &str) -> Option<Option<&str>> {
    let mut words = query.split_whitespace();
    if words.next() != Some("/shell") {
        return None;
    }
    Some(words.next())
}

/// What to attach to: `Some(Some(id))` for `/attach <id>`, `Some(None)` for
/// the picker (`/agents`, or a bare `/attach`), `None` for anything else.
fn attach_target(query: &str) -> Option<Option<&str>> {
    if query.trim() == "/agents" {
        return Some(None);
    }
    attach_command(query)
}

const HELP_TEXT: &str = "\
Commands:
  /help      show this list           /exit      quit
  /clear     reset session            /refresh   resync host context
  /model     list or switch model     /pane      list or pin target pane
  /approvals list/on/off/revoke       /prompt    list or switch system prompt
  /limits    show active limits       /session   save/load/list/delete/rename
  /shells    list the agent's shells  /hosts     configured hosts and ssh state
  /tail      live view of a shell     (alias /watch; f follow, esc to close)
  /logs      past shell recordings    [host] [--owner chat|ghost] [--since 2h]
  /log       open one recording       (/log <id>; n/p commands, space replays)
  /attach    type into a live shell   (/attach <id>; ctrl-d detach, twice for EOF)
  /shell     open your own shell      (/shell [host]; attaches; agents see it)
  /agents    pick a shell to attach   (alias: a bare /attach; ↑↓ enter esc)

Aliases: /models /panes /sessions /approval /new /quit (and help, ? open help).

At a tool-approval prompt, type a message instead of Y/A/N to redirect the agent.
Tool output is capped at 10 lines on screen (… N more lines · ctrl+o opens the full transcript).

Up/Down navigate the input; at the top/bottom edge they recall history.
";

pub(super) async fn run_chat_inner(resume_id: Option<String>) -> Result<()> {
    let start_time = std::time::Instant::now();
    // `--resume <id>` reuses the given id; a fresh chat mints one (M27 phase 03).
    let session_id = match resume_id.as_deref() {
        Some(id) => id.to_string(),
        None => new_session_id(),
    };
    // None = use daemon's configured default prompt; Some(name) = override.
    let current_prompt: Option<String> = None;
    let stdin = crate::cli::input::AsyncStdin::new()?;
    let mut input_state = InputState::new();
    let chat_start_config = Config::load().unwrap_or_default();
    let mut approval = SessionApproval::from_config(&chat_start_config.approvals);
    // Register the SIGWINCH listener before doing anything that depends on
    // terminal size.  tokio queues signals from the moment the listener is
    // created, so no resize event can slip through the gap between process
    // start and our first poll.
    let mut sigwinch = {
        use tokio::signal::unix::{SignalKind, signal};
        signal(SignalKind::window_change())?
    };

    // Resolve the tmux session name and target pane for foreground commands
    // before any terminal setup.  This may prompt the user once if the window
    // has no sibling pane and they opt to split or pick from another window.
    let tmux_session = crate::tmux::current_session_name();
    let pane_id_opt = std::env::var("TMUX_PANE").ok();
    let target_pane: Option<String> = match (&pane_id_opt, &tmux_session) {
        (Some(my_pane), Some(session)) => resolve_target_pane(my_pane, session),
        _ => None,
    };
    let mut chat_width: usize;
    if let Some(ref pane_id) = pane_id_opt {
        chat_width = crate::tmux::query_pane_width(pane_id).unwrap_or_else(|_| terminal_width());
    } else {
        chat_width = terminal_width();
    }

    // When running inside tmux a new split pane triggers one or more SIGWINCH
    // signals as the layout is negotiated.  Wait here until no SIGWINCH has
    // arrived for SETTLE_MS milliseconds so we know the final dimensions before
    // printing anything.  Re-query on every signal so we always end up with
    // the correct settled size.
    if pane_id_opt.is_some() {
        const SETTLE_MS: u64 = 500;
        while tokio::time::timeout(std::time::Duration::from_millis(SETTLE_MS), sigwinch.recv())
            .await
            .is_ok()
        {
            // Another resize — update dims and restart the quiet timer.
            chat_width = match &pane_id_opt {
                Some(pid) => {
                    crate::tmux::query_pane_width(pid).unwrap_or_else(|_| terminal_width())
                }
                None => terminal_width(),
            };
        } // stable for SETTLE_MS — proceed
    }

    // Ratatui path: create the inline-viewport renderer (enters raw mode
    // internally).
    let mut renderer = match RatatuiRendererStdout::new(start_time) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Failed to initialise ratatui renderer: {e}");
            return Err(e.into());
        }
    };

    // Chat presence (M27 phase 03): attach before the first prompt. The reply
    // carries the catch-up brief when the session was away ≥ 30 s; a failed
    // attach must not stop chat from running.
    let (attach_brief, parked) = match chat_presence::attach(&session_id).await {
        Ok((brief, held)) => (
            brief,
            Some(held.park_with_default_reconnect(session_id.to_string())),
        ),
        Err(e) => {
            log::warn!("chat presence attach failed (chat continues): {e}");
            (None, None)
        }
    };
    if let Some(brief) = attach_brief {
        let _ = renderer.commit(&format!("\n⚙ {brief}\n"));
    }

    let result = run_chat_inner_raw(
        session_id,
        current_prompt,
        RawChatArgs {
            handles: InputHandles {
                state: &mut input_state,
                stdin: &stdin,
                sigwinch: &mut sigwinch,
            },
            term: TerminalCtx { chat_width },
            approval: &mut approval,
            tmux: TmuxCtx {
                session: tmux_session,
                pane: target_pane,
            },
            renderer,
            parked,
        },
    )
    .await;
    // Ratatui renderer is consumed by run_chat_inner_raw (restores on exit).
    result
}

struct InputHandles<'a> {
    state: &'a mut InputState,
    stdin: &'a AsyncStdin,
    sigwinch: &'a mut tokio::signal::unix::Signal,
}

struct TerminalCtx {
    chat_width: usize,
}

struct TmuxCtx {
    session: Option<String>,
    pane: Option<String>,
}

struct RatatuiCtx<'a> {
    input_state: &'a mut InputState,
    stdin: &'a AsyncStdin,
    sigwinch: &'a mut tokio::signal::unix::Signal,
    renderer: RatatuiRendererStdout,
    chat_width: usize,
    session_id: String,
    current_prompt: Option<String>,
    approval: &'a mut SessionApproval,
    tmux_session: Option<String>,
    target_pane: Option<String>,
    /// The parked presence connection (M27 phase 03b): its background task
    /// forwards every pushed `Response::SystemMsg` onto the channel; the chat
    /// loop selects on it and renders each push through the renderer it owns.
    /// Kept whole (guard + receiver) so the presence hold lives exactly as
    /// long as the loop — bug-03b-1.
    parked: Option<super::chat_presence::ParkedPresence>,
}

/// Everything `run_chat_inner_raw` needs that is not a plain value: the input
/// handles, the terminal width, the tmux context and the parked presence
/// connection. Grouped so the function stays at seven arguments (clippy's
/// `too_many_arguments`), which phase 04 left it at.
struct RawChatArgs<'a> {
    handles: InputHandles<'a>,
    term: TerminalCtx,
    approval: &'a mut SessionApproval,
    tmux: TmuxCtx,
    renderer: RatatuiRendererStdout,
    parked: Option<super::chat_presence::ParkedPresence>,
}

async fn run_chat_inner_raw(
    session_id: String,
    current_prompt: Option<String>,
    args: RawChatArgs<'_>,
) -> Result<()> {
    let RawChatArgs {
        handles,
        term,
        approval,
        tmux,
        renderer,
        parked,
    } = args;
    let InputHandles {
        state: input_state,
        stdin,
        sigwinch,
    } = handles;
    let TerminalCtx { chat_width } = term;
    let TmuxCtx {
        session: tmux_session,
        pane: target_pane,
    } = tmux;

    run_chat_ratatui(RatatuiCtx {
        input_state,
        stdin,
        sigwinch,
        renderer,
        chat_width,
        session_id,
        current_prompt,
        approval,
        tmux_session,
        target_pane,
        parked,
    })
    .await
}
#[path = "chat_loop.rs"]
mod chat_loop;
use chat_loop::run_chat_ratatui;
// The loop reaches these sibling modules as `super::…`, through this import.
use super::{agents, ipc_client, resume_hint};

fn banner_lines(chat_width: usize, palette: &crate::cli::palette::Palette) -> Vec<Line<'static>> {
    let logo: &[&str] = &[
        "                        ▄      ▄",
        "                       ██▄    ▄██",
        "                      █████▄▄█████",
        "                   ▄████████████████▄",
        "                  ████████████████████",
        "                 ████████  ▀▀  ████████",
        "                ██████▀   ▄██▄   ▀██████",
        "                █████    ███ ██    █████",
        "                █████    ▀████▀    █████",
        "                ██████▄   ▀██▀   ▄██████",
        "                 ████████▄▄  ▄▄████████",
        "                  ████████████████████",
        "                   ▀████▀▀████▀▀████▀",
        "                   ▄▀  █  █  █  █  ▀▄",
        "                  █    █  █  █  █    █",
        "                 ▄▀   ▄▀  █  █  ▀▄   ▀▄",
        "                 █   █    █  █    █   █",
        "",
        "████▄   ▄▄▄  ▄▄▄▄▄ ▄▄   ▄▄  ▄▄▄  ▄▄  ▄▄ ██████ ▄▄ ▄▄ ▄▄▄▄▄",
        "██  ██ ██▀██ ██▄▄  ██▀▄▀██ ██▀██ ███▄██ ██▄▄   ▀███▀ ██▄▄",
        "████▀  ██▀██ ██▄▄▄ ██   ██ ▀███▀ ██ ▀██ ██▄▄▄▄   █   ██▄▄▄",
    ];
    let subtitle = "AGENTIC OPERATOR";

    let logo_w = logo.iter().map(|l| l.chars().count()).max().unwrap_or(0);
    let pad = " ".repeat((chat_width.saturating_sub(logo_w)) / 2);

    let blood_red = Style::default()
        .fg(palette.red())
        .add_modifier(Modifier::BOLD);
    let deep_yellow = Style::default().fg(palette.yellow());
    let title_style = Style::default()
        .fg(Color::White)
        .add_modifier(Modifier::BOLD);
    let dim = Style::default().add_modifier(Modifier::DIM);

    // Eye highlights: which string to color per line index.
    let eye_markers: &[(usize, &str)] = &[(6, "▄██▄"), (7, "███ ██"), (8, "▀████▀"), (9, "▀██▀")];

    let mut lines: Vec<Line<'static>> = Vec::with_capacity(logo.len() + 4);

    lines.push(Line::raw(""));

    for (i, &raw) in logo.iter().enumerate() {
        let eye_opt = eye_markers
            .iter()
            .find(|&&(idx, _)| idx == i)
            .map(|&(_, s)| s);

        let spans: Vec<Span<'static>> = if let Some(eye) = eye_opt {
            // Line has an eye highlight: split into pre/eye/post.
            if let Some(byte_pos) = raw.find(eye) {
                let pre = raw[..byte_pos].to_owned();
                let post = raw[byte_pos + eye.len()..].to_owned();
                vec![
                    Span::styled(pad.clone(), Style::default()),
                    Span::styled(pre, blood_red),
                    Span::styled(eye.to_owned(), deep_yellow),
                    Span::styled(post, blood_red),
                ]
            } else {
                vec![
                    Span::styled(pad.clone(), Style::default()),
                    Span::styled(raw.to_owned(), blood_red),
                ]
            }
        } else if i >= 18 {
            // DAEMONEYE word-mark lines.
            vec![
                Span::styled(pad.clone(), Style::default()),
                Span::styled(raw.to_owned(), title_style),
            ]
        } else {
            vec![
                Span::styled(pad.clone(), Style::default()),
                Span::styled(raw.to_owned(), blood_red),
            ]
        };

        lines.push(Line::from(spans));
    }

    // Subtitle: same block offset as logo lines, then centered within logo_w.
    let sub_pad = pad.clone() + &" ".repeat(logo_w.saturating_sub(subtitle.chars().count()) / 2);
    lines.push(Line::from(vec![
        Span::raw(sub_pad),
        Span::styled(subtitle, dim),
    ]));

    // Blank line then slash-command hint.
    lines.push(Line::raw(""));

    let hint_plain = "/help  — list commands      /exit  — quit";
    let hint_pad = " ".repeat((chat_width.saturating_sub(hint_plain.chars().count())) / 2);
    lines.push(Line::from(vec![
        Span::raw(hint_pad),
        Span::styled("/help".to_owned(), Style::default().fg(Color::Cyan)),
        Span::styled("  — list commands      ".to_owned(), dim),
        Span::styled("/exit".to_owned(), Style::default().fg(Color::Cyan)),
        Span::styled("  — quit".to_owned(), dim),
    ]));

    lines.push(Line::raw(""));

    lines
}

/// Build the chat-history attribution label from identity parts.
/// `host` is domain-stripped (`pinky.home.planetfoo.org` → `pinky`).
/// A missing/`"unknown"` host degrades to the bare user; a missing user
/// degrades to the literal `you`.
fn user_host_label(user: Option<&str>, host: Option<&str>) -> String {
    let Some(user) = user.filter(|u| !u.is_empty()) else {
        return "you".to_string();
    };
    let short = host
        .map(|h| h.split('.').next().unwrap_or("").to_string())
        .unwrap_or_default();
    if short.is_empty() || short == "unknown" {
        user.to_string()
    } else {
        format!("{user}@{short}")
    }
}

/// The label for this CLI process's host — the machine `daemoneye chat`
/// runs on, which can differ from the daemon host.
fn local_user_host() -> String {
    let user = std::env::var("USER").ok();
    let host = crate::daemon::utils::daemon_hostname();
    user_host_label(user.as_deref(), Some(&host))
}

fn echo_body(query: &str) -> Vec<String> {
    query.lines().map(str::to_string).collect()
}

/// Return `true` when a query should be echoed into scrollback.
///
/// Client-only commands (`/exit`, `/help`, `/clear`, …) never reach the model
/// so they must not be echoed.  Prose that merely starts with `/` (a file path)
/// or with a keyword like `help` or `clear` *does* reach the model and is
/// echoed.
fn should_echo(query: &str) -> bool {
    !matches!(
        query,
        "/exit" | "/quit" | "exit" | "quit" | "/help" | "help" | "?" | "/?" | "/clear" | "/new"
    )
}

/// Bottom-border context label for the user-echo panel.
fn turn_budget_label(turn: usize, prompt_tokens: u32, context_window: u32) -> String {
    let budget = if prompt_tokens == 0 {
        "new session".to_string()
    } else {
        let pct = (prompt_tokens as f64 / f64::from(context_window.max(1)) * 100.0) as u32;
        format!("{prompt_tokens} / {context_window} ({pct}%)")
    };
    format!("turn {turn} · {budget}")
}

#[cfg(test)]
mod tests {
    use super::{HELP_TEXT, attach_command, attach_target, log_command, shell_command};

    #[test]
    fn help_text_lists_shells() {
        assert!(
            HELP_TEXT.contains("/shells"),
            "HELP_TEXT must list '/shells'"
        );
    }

    #[test]
    fn help_text_lists_tail() {
        assert!(HELP_TEXT.contains("/tail"), "HELP_TEXT must list '/tail'");
        assert!(
            HELP_TEXT.contains("/watch"),
            "HELP_TEXT must name the /watch alias"
        );
    }

    #[test]
    fn help_text_lists_hosts() {
        assert!(HELP_TEXT.contains("/hosts"), "HELP_TEXT must list '/hosts'");
    }

    #[test]
    fn attach_target_opens_the_picker_for_agents_and_bare_attach() {
        assert_eq!(attach_target("/agents"), Some(None));
        assert_eq!(attach_target("  /agents "), Some(None));
        assert_eq!(attach_target("/attach"), Some(None));
        assert_eq!(attach_target("/attach s3"), Some(Some("s3")));
        for other in ["/agentsx", "/agents s3", "/agent", "/shells"] {
            assert_eq!(attach_target(other), None, "{other:?} opens nothing");
        }
        assert!(
            HELP_TEXT.contains("/agents"),
            "HELP_TEXT must list '/agents'"
        );
    }

    #[test]
    fn shell_command_matches_shell_and_not_shells() {
        assert_eq!(
            shell_command("/shell"),
            Some(None),
            "bare /shell is the daemon host"
        );
        assert_eq!(shell_command("/shell pinky"), Some(Some("pinky")));
        assert_eq!(shell_command("  /shell   pinky "), Some(Some("pinky")));
        for other in ["/shells", "/shells mine", "/shellx", "shell pinky", "/sh"] {
            assert_eq!(shell_command(other), None, "{other:?} is not /shell");
        }
        assert!(
            HELP_TEXT.contains("/shell "),
            "HELP_TEXT must list '/shell'"
        );
    }

    #[test]
    fn attach_command_matches_attach_only() {
        assert_eq!(
            attach_command("/attach"),
            Some(None),
            "a bare /attach opens the picker"
        );
        assert_eq!(attach_command("/attach s3"), Some(Some("s3")));
        assert_eq!(attach_command("  /attach   s12 "), Some(Some("s12")));
        for other in [
            "/attachx s3",
            "/attach-s3",
            "attach s3",
            "/agents",
            "/tail s3",
        ] {
            assert_eq!(attach_command(other), None, "{other:?} is not /attach");
        }
        assert!(
            HELP_TEXT.contains("/attach "),
            "HELP_TEXT must list '/attach'"
        );
    }

    #[test]
    fn log_command_matches_log_and_not_logs() {
        assert_eq!(log_command("/log"), Some(None), "bare /log asks for usage");
        assert_eq!(log_command("/log s3-17"), Some(Some("s3-17")));
        assert_eq!(log_command("/log   s3-17  "), Some(Some("s3-17")));
        for other in ["/logs", "/logs pinky", "/login", "/log-x", "log s3-17"] {
            assert_eq!(log_command(other), None, "{other:?} is not /log");
        }
        assert!(HELP_TEXT.contains("/log "), "HELP_TEXT must list '/log'");
    }

    #[test]
    fn help_text_lists_logs() {
        assert!(HELP_TEXT.contains("/logs"), "HELP_TEXT must list '/logs'");
    }

    #[test]
    fn help_text_documents_aliases_and_behaviors() {
        // Aliases
        for alias in [
            "/models",
            "/panes",
            "/sessions",
            "/approval",
            "/new",
            "/quit",
        ] {
            assert!(
                HELP_TEXT.contains(alias),
                "HELP_TEXT must contain alias '{alias}'"
            );
        }
        // Approval-prompt redirect
        assert!(
            HELP_TEXT.contains("redirect"),
            "HELP_TEXT must document redirect behavior"
        );
        // Tool-output cap
        assert!(
            HELP_TEXT.contains("10 lines"),
            "HELP_TEXT must document the 10-line tool-output cap"
        );
    }

    #[test]
    fn echo_body_splits_multiline_query() {
        use super::echo_body;

        assert_eq!(echo_body("one line"), vec!["one line"]);

        let parts = echo_body("first\nsecond\nthird");
        assert_eq!(parts, vec!["first", "second", "third"]);

        assert_eq!(echo_body("trailing\n"), vec!["trailing"]);
    }

    #[test]
    fn echo_skips_client_only_commands() {
        use super::should_echo;

        // Must NOT echo — client-only commands
        for cmd in [
            "/exit", "/quit", "exit", "quit", "/help", "help", "?", "/?", "/clear", "/new",
        ] {
            assert!(!should_echo(cmd), "must not echo: {cmd}");
        }

        // Must echo — prose queries
        for q in [
            "what is this error",
            "/etc/hosts is missing",
            "help me debug this",
            "clearly this is wrong",
        ] {
            assert!(should_echo(q), "must echo: {q}");
        }
    }

    #[test]
    fn user_host_label_joins_user_and_shorthost() {
        use super::user_host_label;
        assert_eq!(
            user_host_label(Some("matt"), Some("pinky.home.planetfoo.org")),
            "matt@pinky"
        );
    }

    #[test]
    fn user_host_label_missing_user_falls_back_to_you() {
        use super::user_host_label;
        assert_eq!(user_host_label(None, Some("pinky")), "you");
        assert_eq!(user_host_label(Some(""), Some("pinky")), "you");
    }

    #[test]
    fn user_host_label_unknown_host_degrades_to_bare_user() {
        use super::user_host_label;
        assert_eq!(user_host_label(Some("matt"), Some("unknown")), "matt");
        assert_eq!(user_host_label(Some("matt"), Some("")), "matt");
    }

    // ── turn_budget_label ──────────────────────────────────────────────

    #[test]
    fn turn_budget_label_new_session() {
        use super::turn_budget_label;
        assert_eq!(turn_budget_label(1, 0, 200_000), "turn 1 · new session");
    }

    #[test]
    fn turn_budget_label_reports_usage() {
        use super::turn_budget_label;
        assert_eq!(
            turn_budget_label(3, 50_000, 200_000),
            "turn 3 · 50000 / 200000 (25%)"
        );
    }
}
