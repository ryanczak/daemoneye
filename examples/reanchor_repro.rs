//! Headless repro harness for the input-border corruption on tmux window
//! switches. Mirrors the chat loop's renderer usage: commit content, draw the
//! live region, and on FocusGained (ESC [ I) call reanchor() + draw() exactly
//! like `read_input_line_inner_ratatui` does.
//!
//! Drive it from outside via `tmux send-keys -H 1b 5b 49` and compare
//! `tmux capture-pane` output before/after.

use daemoneye::cli::input::{AsyncStdin, InputLine, Key, read_key};
use daemoneye::cli::render::StatusBarState;
use daemoneye::cli::render_ratatui::RatatuiRenderer;

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    let n_lines: usize = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(30);

    let mut renderer = RatatuiRenderer::new(std::time::Instant::now())?;
    for i in 1..=n_lines {
        renderer.commit(&format!("content line {i:03}")).ok();
    }

    let input = InputLine::new();
    let sb = StatusBarState {
        session_id: "repro-session",
        approval_hint: "",
        model: "repro-model",
        prompt_tokens: 0,
        context_window: 100_000,
        daemon_up: true,
        tools_total: 0,
        cost_usd: 0.0,
        has_untracked: false,
    };
    renderer.draw(&input, &sb).ok();

    let stdin = AsyncStdin::new()?;
    let mut sigwinch =
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::window_change())?;
    loop {
        let key = tokio::select! {
            _ = sigwinch.recv() => {
                renderer.reanchor();
                renderer.draw(&input, &sb).ok();
                continue;
            }
            k = read_key(&stdin) => k,
        };
        match key {
            Some(Key::FocusGained) => {
                renderer.reanchor();
                renderer.draw(&input, &sb).ok();
            }
            Some(Key::Char('c')) => {
                // Commit one more content line, like a streamed turn would.
                renderer.commit("late content line").ok();
                renderer.draw(&input, &sb).ok();
            }
            Some(Key::Char('p')) => {
                // Commit a bordered panel, like a user-turn echo.
                renderer
                    .commit_panel(
                        "matt@repro",
                        &["panel body one".into(), "panel body two".into()],
                        false,
                    )
                    .ok();
                renderer.draw(&input, &sb).ok();
            }
            Some(Key::Char('q')) | Some(Key::CtrlC) | None => break,
            _ => {}
        }
    }
    renderer.restore();
    Ok(())
}
