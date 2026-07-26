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

use crate::daemon::session::{SessionStore, with_sessions};

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
    with_sessions(ctx.sessions, |store| {
        if let Some(entry) = store.get_mut(sid) {
            entry
                .artifacts_created
                .push(crate::session_store::ArtifactRef {
                    kind: kind.to_string(),
                    name: name.to_string(),
                    at_turn: ctx.turn_count,
                });
        }
    });
}

// ---------------------------------------------------------------------------
// Shared test utilities (test-only)
// ---------------------------------------------------------------------------

#[cfg(test)]
pub(crate) mod testutil {
    use crate::util::UnpoisonExt;

    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    pub struct TmpHome(std::path::PathBuf);

    impl TmpHome {
        pub fn new() -> Self {
            let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let p = std::env::temp_dir().join(format!("de_know_test_{}_{}", std::process::id(), n));
            std::fs::create_dir_all(&p).unwrap();
            TmpHome(p)
        }
        pub fn path(&self) -> &std::path::Path {
            &self.0
        }
    }

    impl Drop for TmpHome {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    pub fn with_home<F: FnOnce()>(tmp: &TmpHome, f: F) {
        let _guard = crate::TEST_HOME_LOCK.lock().unwrap_or_log();
        let old = std::env::var("HOME").ok();
        unsafe {
            std::env::set_var("HOME", tmp.path());
        }
        f();
        match old {
            Some(v) => unsafe {
                std::env::set_var("HOME", v);
            },
            None => unsafe {
                std::env::remove_var("HOME");
            },
        }
    }
}
