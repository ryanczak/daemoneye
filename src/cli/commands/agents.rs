//! The `/agents` picker (also a bare `/attach`): the live shells, agents'
//! first, one row each as `/shells` writes them; `↑`/`↓` select, `Enter`
//! attaches, `Esc` closes. The list and the key loop are pure over their
//! inputs, so both are tested without a daemon or a terminal.

use crate::cli::input::Key;
use crate::ipc::ShellInfo;

/// The picker's list: every live shell, with its id and its `/shells` row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct AgentPicker {
    pub ids: Vec<String>,
    pub rows: Vec<String>,
    pub selected: usize,
}

impl AgentPicker {
    /// The live shells, those a ghost owns first, each group in the order the
    /// daemon listed them. `None` when no shell is live: an exited shell
    /// cannot be attached to.
    pub fn new(shells: &[ShellInfo], session_id: &str, now_unix: u64) -> Option<Self> {
        let mut live: Vec<ShellInfo> = shells
            .iter()
            .filter(|s| s.exited_unix.is_none())
            .cloned()
            .collect();
        if live.is_empty() {
            return None;
        }
        live.sort_by_key(|s| !s.owner.starts_with("ghost:"));
        Some(Self {
            ids: live.iter().map(|s| s.id.clone()).collect(),
            rows: super::slash::render_shell_list(&live, session_id, now_unix),
            selected: 0,
        })
    }

    pub fn up(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    pub fn down(&mut self) {
        if self.selected + 1 < self.ids.len() {
            self.selected += 1;
        }
    }

    pub fn selected_id(&self) -> &str {
        &self.ids[self.selected]
    }

    pub fn title(&self) -> String {
        format!(
            "agents · {}/{} · ↑↓ select · enter attach · esc close",
            self.selected + 1,
            self.ids.len()
        )
    }
}

/// What one key does in the picker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PickerAction {
    Up,
    Down,
    Attach,
    Close,
    Ignore,
}

pub(super) fn picker_key_action(key: &Key) -> PickerAction {
    match key {
        Key::Up => PickerAction::Up,
        Key::Down => PickerAction::Down,
        Key::Enter => PickerAction::Attach,
        Key::Char('\x1b') | Key::Char('q') | Key::CtrlC => PickerAction::Close,
        _ => PickerAction::Ignore,
    }
}

/// Run the picker: draw it, then apply keys until one attaches (the chosen
/// shell id) or closes it (`None`). The keys ending closes it too. `draw` is
/// called once at the start and again after every key that moves the
/// selection.
pub(super) async fn pick_agent<K, D>(
    picker: &mut AgentPicker,
    keys: &mut K,
    mut draw: D,
) -> Option<String>
where
    K: futures_util::Stream<Item = Key> + Unpin,
    D: FnMut(&AgentPicker),
{
    use futures_util::StreamExt;

    draw(picker);
    while let Some(key) = keys.next().await {
        match picker_key_action(&key) {
            PickerAction::Up => picker.up(),
            PickerAction::Down => picker.down(),
            PickerAction::Attach => return Some(picker.selected_id().to_string()),
            PickerAction::Close => return None,
            PickerAction::Ignore => continue,
        }
        draw(picker);
    }
    None
}

/// Run `/agents` (or a bare `/attach`): list the shells, and if any is live,
/// pick one in the chat viewport and attach to it.
pub(super) async fn run_agents(
    stdin: &crate::cli::input::AsyncStdin,
    sigwinch: &mut tokio::signal::unix::Signal,
    renderer: &mut crate::cli::render_ratatui::RatatuiRendererStdout,
    status: &crate::cli::render::StatusBarState<'_>,
) -> anyhow::Result<()> {
    use crate::ipc::{Request, Response};

    let shells = match super::ipc_client::request_once(Request::ShellList).await? {
        Response::ShellList { shells } => shells,
        Response::Error(msg) => anyhow::bail!(msg),
        other => anyhow::bail!("unexpected reply to ShellList: {}", other.kind()),
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let Some(mut picker) = AgentPicker::new(&shells, status.session_id, now) else {
        let _ = renderer.commit("  no live shells to attach to (see /shells)\n");
        return Ok(());
    };
    let chosen = {
        let mut keys = Box::pin(futures_util::stream::unfold(stdin, |stdin| async move {
            crate::cli::input::read_key(stdin)
                .await
                .map(|key| (key, stdin))
        }));
        pick_agent(&mut picker, &mut keys, |p| {
            let _ = renderer.draw_picker_panel(&p.title(), &p.rows, p.selected, status);
        })
        .await
    };
    match chosen {
        Some(id) => crate::cli::attach::run_attach(stdin, sigwinch, renderer, &id).await,
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_790_000_000;

    fn shell(id: &str, owner: &str, exited: bool) -> ShellInfo {
        ShellInfo {
            id: id.to_string(),
            host: "local".to_string(),
            owner: owner.to_string(),
            started_unix: NOW - 60,
            dedicated: false,
            exited_unix: exited.then_some(NOW - 10),
            last_command: None,
        }
    }

    #[test]
    fn agent_picker_lists_live_shells_ghosts_first() {
        let shells = [
            shell("s1", "chat:me", false),
            shell("s2", "ghost:job-1", false),
            shell("s3", "ghost:job-2", true),
            shell("s4", "chat:other", false),
        ];
        let picker = AgentPicker::new(&shells, "me", NOW).expect("live shells");
        assert_eq!(
            picker.ids,
            ["s2", "s1", "s4"],
            "ghosts first, exited shells left out, daemon order kept"
        );
        assert_eq!(picker.rows.len(), 3);
        assert!(picker.rows[0].starts_with("s2 "), "{:?}", picker.rows[0]);
        assert!(
            picker.rows[1].contains("owner:this session"),
            "rows are the /shells rows: {:?}",
            picker.rows[1]
        );
        assert_eq!(picker.selected, 0);
        assert!(
            AgentPicker::new(&[shell("s3", "ghost:job-2", true)], "me", NOW).is_none(),
            "no live shell, no picker"
        );
        assert!(AgentPicker::new(&[], "me", NOW).is_none());
    }

    #[test]
    fn agent_picker_moves_and_clamps() {
        let shells = [
            shell("s1", "chat:me", false),
            shell("s2", "chat:me", false),
            shell("s3", "chat:me", false),
        ];
        let mut p = AgentPicker::new(&shells, "me", NOW).unwrap();
        p.up();
        assert_eq!(p.selected_id(), "s1", "up at the top stays");
        p.down();
        p.down();
        assert_eq!(p.selected_id(), "s3");
        p.down();
        assert_eq!(p.selected_id(), "s3", "down at the bottom stays");
        assert!(p.title().starts_with("agents · 3/3"), "{}", p.title());
    }

    #[test]
    fn picker_key_action_maps_keys() {
        assert_eq!(picker_key_action(&Key::Up), PickerAction::Up);
        assert_eq!(picker_key_action(&Key::Down), PickerAction::Down);
        assert_eq!(picker_key_action(&Key::Enter), PickerAction::Attach);
        for close in [Key::Char('\x1b'), Key::Char('q'), Key::CtrlC] {
            assert_eq!(picker_key_action(&close), PickerAction::Close, "{close:?}");
        }
        for none in [Key::Char('j'), Key::Left, Key::FocusGained] {
            assert_eq!(picker_key_action(&none), PickerAction::Ignore, "{none:?}");
        }
    }

    #[tokio::test]
    async fn pick_agent_returns_the_chosen_shell_or_none() {
        let shells = [shell("s1", "chat:me", false), shell("s2", "chat:me", false)];
        let base = AgentPicker::new(&shells, "me", NOW).unwrap();

        let mut p = base.clone();
        let mut draws = Vec::new();
        let mut keys = futures_util::stream::iter([Key::Char('j'), Key::Down, Key::Enter]);
        let chosen = pick_agent(&mut p, &mut keys, |p| draws.push(p.selected)).await;
        assert_eq!(
            chosen.as_deref(),
            Some("s2"),
            "enter attaches to the selection"
        );
        assert_eq!(
            draws,
            [0, 1],
            "drawn at the start and after the move, not after an ignored key"
        );

        let mut p = base.clone();
        let mut keys = futures_util::stream::iter([Key::Down, Key::Char('\x1b')]);
        assert_eq!(
            pick_agent(&mut p, &mut keys, |_| {}).await,
            None,
            "esc closes"
        );

        let mut p = base;
        let mut keys = futures_util::stream::iter([Key::Down]);
        assert_eq!(
            pick_agent(&mut p, &mut keys, |_| {}).await,
            None,
            "the keys ending closes the picker"
        );
    }
}
