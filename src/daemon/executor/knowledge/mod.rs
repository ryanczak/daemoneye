mod agents;
mod artifacts;
mod ghost;
mod memory;
mod pane;

pub(super) use agents::{
    CreateAgentArgs, await_agent_result, create_agent, delete_agent, list_agents_tool, read_agent,
};
pub(super) use artifacts::{
    delete_runbook, delete_script, list_runbooks, list_scripts, read_runbook, read_script,
    write_runbook, write_script,
};
pub(super) use ghost::spawn_ghost;
pub(super) use memory::{
    UpdateMemoryRequest, add_memory, delete_memory, list_memories, read_memory, search_repository,
    update_memory,
};
pub(super) use pane::{close_bg_window, list_panes, watch_pane};

use crate::daemon::session::SessionStore;

// ── ArtifactCtx + track_artifact moved verbatim from old lines 20–46 ──
pub(super) struct ArtifactCtx<'a> {
    pub session_id: Option<&'a str>,
    pub sessions: &'a SessionStore,
    pub saved_name: Option<&'a str>,
    pub turn_count: usize,
    pub is_ghost: bool,
    pub namespaces: &'a [&'a str],
}

fn track_artifact(ctx: &ArtifactCtx<'_>, kind: &str, name: &str) {
    if ctx.is_ghost {
        return;
    }
    let Some(sid) = ctx.session_id else { return };
    let Ok(mut store) = ctx.sessions.lock() else {
        return;
    };
    if let Some(entry) = store.get_mut(sid) {
        entry
            .artifacts_created
            .push(crate::session_store::ArtifactRef {
                kind: kind.to_string(),
                name: name.to_string(),
                at_turn: ctx.turn_count,
            });
    }
}
