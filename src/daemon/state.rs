//! Consolidated runtime state for a DaemonEye daemon instance (`DaemonState`).

use std::sync::Arc;
use std::time::Instant;

use crate::bus::Bus;
use crate::config::{Config, ConfigHandle};
use crate::daemon::session::{PresenceRegistry, SessionStore};
use crate::daemon::stats::DaemonMetrics;
use crate::scheduler::ScheduleStore;
use crate::shell::ShellRegistry;
use crate::tmux::cache::SessionCache;
use crate::webhook::AlertRegistry;

/// Consolidated runtime state for a DaemonEye daemon instance.
///
/// Constructed once in [`crate::daemon::run_daemon`] (or via
/// [`DaemonState::new_for_test`] in isolated unit/integration tests) and
/// shared across the IPC accept loop, background supervisor tasks, scheduler,
/// and webhook server via `Arc<DaemonState>`.
pub struct DaemonState {
    pub config: Arc<ConfigHandle>,
    pub sessions: SessionStore,
    pub presence: PresenceRegistry,
    pub shells: ShellRegistry,
    pub schedules: Arc<ScheduleStore>,
    pub tmux_cache: Arc<SessionCache>,
    pub bus: Arc<Bus>,
    pub alerts: AlertRegistry,
    pub metrics: Arc<DaemonMetrics>,
    pub started_at: Instant,
}

impl DaemonState {
    /// Construct a `DaemonState` from the daemon's initialized subsystems.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        config: Arc<ConfigHandle>,
        sessions: SessionStore,
        presence: PresenceRegistry,
        shells: ShellRegistry,
        schedules: Arc<ScheduleStore>,
        tmux_cache: Arc<SessionCache>,
        bus: Arc<Bus>,
        alerts: AlertRegistry,
        metrics: Arc<DaemonMetrics>,
        started_at: Instant,
    ) -> Arc<Self> {
        Arc::new(Self {
            config,
            sessions,
            presence,
            shells,
            schedules,
            tmux_cache,
            bus,
            alerts,
            metrics,
            started_at,
        })
    }

    /// Construct a fresh, isolated `Arc<DaemonState>` for tests with no shared
    /// globals across instances.
    pub fn new_for_test(config: Config) -> Arc<Self> {
        Arc::new(Self {
            config: Arc::new(ConfigHandle::new(config)),
            sessions: SessionStore::new(),
            presence: PresenceRegistry::new(),
            shells: ShellRegistry::default(),
            schedules: Arc::new(ScheduleStore::new_empty()),
            tmux_cache: Arc::new(SessionCache::new("")),
            bus: Arc::new(Bus::new()),
            alerts: AlertRegistry::new(),
            metrics: Arc::new(DaemonMetrics::new()),
            started_at: Instant::now(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_for_test_creates_isolated_state_instances() {
        let a = DaemonState::new_for_test(Config::default());
        let b = DaemonState::new_for_test(Config::default());

        a.metrics.ghosts.inc_launched();
        assert_eq!(a.metrics.ghosts.launched(), 1);
        assert_eq!(b.metrics.ghosts.launched(), 0);
        assert_eq!(a.presence.count("s1"), 0);
        assert!(a.alerts.snapshot().is_empty());
    }
}
