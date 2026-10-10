mod agents;
mod artifacts;
mod ghost;
mod memory;
mod shell;

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
pub(super) use shell::{
    close_shell, find_in_shells, input_target_note, list_shell_logs, list_shells, open_shell,
    read_shell, read_shell_log, send_input, watch_shell,
};
// Wider than the group above on purpose: the prompt builders reach it from
// `daemon`, and a `pub(super)` binding cannot be widened on re-export.
pub use shell::{
    ContextBreadth, hosts_context_block, open_shell_id, resolve_context_scope, shell_context_block,
    shells_only_context,
};

/// `close_shell_with` is `pub(crate)` in `shell.rs`; the sandboxed chat
/// background path (07) and the read-file live test (07a) reach it through
/// this re-export. Not cfg(test)-gated: the chat path is production code.
pub(crate) use shell::close_shell_with;

/// The registry-swap seam for the same tests: point `open_shell` at a fixture
/// registry for the test's duration. cfg(test)-gated for the same reason.
#[cfg(test)]
pub(crate) use shell::open_shell_test_seam;

/// The 08c ghost teardown: close a ghost's shells when its turn ends. Only
/// the owner's records are touched (test 6 pins that); wired `pub(crate)`
/// because `daemon/ghost.rs` sits outside this module tree.
pub(crate) use shell::close_owner_shells;

/// The live check's ssh spawn fixture, re-exported for `file_ops/read.rs`'s
/// ignored pinky test — same reason as `close_shell_with` above.
#[cfg(test)]
pub(crate) use shell::spawn_pinky_ssh;

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
        let _guard = crate::test_home_guard();
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
