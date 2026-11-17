//! The terminal-program notice producer (M38 phase 06): a supervised 2 s
//! tick that probes each live shell's host over OSC 7501, diffs the records
//! against its own baseline, and publishes one `Shell` notice per
//! transition into `blocked`, `done` or `error`. With the default routes a
//! `Shell` notice reaches no sink — only the journal; phase 07's route
//! config is how a user sends them somewhere.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use crate::bus::{Notice, Severity, Source};
use crate::shell::program_status::{Record, State};
use crate::shell::{Owner, ShellRegistry, socket_path, status, with_shells};

use crate::daemon::executor::knowledge::program_line;

/// The tick's own baseline, keyed by shell id: each shell's last-seen
/// `(id, state)` pairs. It never reads or writes
/// `ShellRecord::last_programs`, which belongs to the read-side refresh.
#[derive(Debug, Default)]
pub struct ProgramWatcher {
    baseline: BTreeMap<String, BTreeSet<(Option<String>, &'static str)>>,
}

/// Whether a record's state is one the bus publishes a notice for. `Idle`
/// and `Working` are never news.
fn is_reportable(state: State) -> bool {
    matches!(state, State::Blocked | State::Done | State::Error)
}

impl ProgramWatcher {
    /// Diff `records` for `shell_id` against this shell's baseline: the
    /// reportable records whose `(id, state)` pair was not seen before,
    /// with the new pairs stored. The **first** observation of a shell is
    /// baseline only — a daemon restart must not replay states the shell
    /// already held.
    pub fn observe(&mut self, shell_id: &str, records: &[Record]) -> Vec<Record> {
        let seen: BTreeSet<(Option<String>, &'static str)> = records
            .iter()
            .map(|r| (r.id.clone(), r.state.as_str()))
            .collect();
        let first_sight = !self.baseline.contains_key(shell_id);
        let news = if first_sight {
            Vec::new()
        } else {
            let entry = &self.baseline[shell_id];
            records
                .iter()
                .filter(|r| {
                    is_reportable(r.state) && !entry.contains(&(r.id.clone(), r.state.as_str()))
                })
                .cloned()
                .collect()
        };
        self.baseline.insert(shell_id.to_string(), seen);
        news
    }

    /// Drop every shell not in `live`. A shell seen again afterwards is a
    /// first observation again.
    pub fn retain(&mut self, live: &HashSet<String>) {
        self.baseline.retain(|id, _| live.contains(id));
    }
}

/// Build the notice one new record publishes. Pure. `name` is the record's
/// `app`, or the shell id when there is none; `source_id` is the shell id;
/// the title is `"[Shell <id>] " + program_line(record)` with an empty
/// body. `session` names the chat's session id for [`Owner::Chat`] and is
/// unset for every other owner.
pub fn program_notice(shell_id: &str, owner: &Owner, record: &Record) -> Notice {
    let severity = match record.state {
        State::Blocked => Severity::Warning,
        State::Error => Severity::Error,
        _ => Severity::Info,
    };
    let name = record.app.clone().unwrap_or_else(|| shell_id.to_string());
    let mut notice = Notice::new(
        severity,
        Source::Shell,
        name,
        format!("[Shell {shell_id}] {}", program_line(record)),
    )
    .source_id(shell_id)
    .label("state", record.state.as_str());
    if let Some(kind) = record.kind {
        notice = notice.label("kind", kind.as_str());
    }
    if let Some(id) = &record.id {
        notice = notice.label("program_id", id);
    }
    if let Owner::Chat(sid) = owner {
        notice = notice.session(sid.clone());
    }
    notice
}

/// Turn one tick's probes into notices. Pure. A shell whose probe answered
/// `None` (the host did not answer) leaves that shell's baseline untouched
/// and yields nothing — a `None` must not be read as "no records", or a
/// host hiccup would replay every held state. Afterwards only the probed
/// ids stay in the baseline.
pub fn collect_notices(
    watcher: &mut ProgramWatcher,
    probed: Vec<(String, Owner, Option<Vec<Record>>)>,
) -> Vec<Notice> {
    let mut notices = Vec::new();
    let mut live: HashSet<String> = HashSet::new();
    for (shell_id, owner, records) in probed {
        live.insert(shell_id.clone());
        if let Some(records) = records {
            for record in watcher.observe(&shell_id, &records) {
                notices.push(program_notice(&shell_id, &owner, &record));
            }
        }
    }
    watcher.retain(&live);
    notices
}

/// The per-probe budget each shell host's `Status` round-trip gets, the
/// same bound `refresh_programs` applies.
const PROGRAM_PROBE_TIMEOUT: Duration = Duration::from_secs(1);

/// One tick: collect the live shells, probe their hosts all at once with no
/// lock held (a 1 s timeout each, as `refresh_programs` does; a
/// non-`Status` reply or an error is `None`), then publish what
/// [`collect_notices`] returns.
async fn program_notices_tick(reg: &ShellRegistry, watcher: &mut ProgramWatcher) {
    let targets: Vec<(String, Owner, std::path::PathBuf)> = with_shells(reg, |store| {
        store
            .values()
            .filter(|record| record.exited_unix.is_none())
            .map(|record| {
                (
                    record.id.as_str().to_string(),
                    record.owner.clone(),
                    socket_path(record.id.as_str()),
                )
            })
            .collect()
    });
    if targets.is_empty() {
        return;
    }
    let probed: Vec<(String, Owner, Option<Vec<Record>>)> =
        futures_util::future::join_all(targets.iter().map(|(id, owner, sock)| async {
            let programs = match status(sock, PROGRAM_PROBE_TIMEOUT).await {
                Ok(crate::shell::ShellResponse::Status { programs, .. }) => Some(programs),
                _ => None,
            };
            (id.clone(), owner.clone(), programs)
        }))
        .await;
    for notice in collect_notices(watcher, probed) {
        crate::bus::publish(notice);
    }
}

/// The 2 s `Shell`-notice tick (M38 phase 06), supervised like the daemon's
/// other background loops. Its own loop owns one [`ProgramWatcher`].
async fn program_notices_loop() {
    let reg = crate::shell::registry().clone();
    let mut watcher = ProgramWatcher::default();
    let mut tick = tokio::time::interval(Duration::from_secs(2));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tick.tick().await;
        program_notices_tick(&reg, &mut watcher).await;
    }
}

/// Spawn the 2 s `Shell`-notice task (M38 phase 06), supervised as
/// `"program-notices"`. Kept out of `run_daemon` so its hygiene-cap line
/// does not grow, like `spawn_session_counters_task` beside it.
pub(crate) fn spawn_program_notices_task(shutdown: Arc<AtomicBool>) {
    tokio::spawn(crate::daemon::supervise(
        "program-notices",
        shutdown,
        program_notices_loop,
    ));
}

#[cfg(test)]
mod tests {
    use super::{ProgramWatcher, collect_notices, is_reportable, program_notice};
    use crate::bus::{Severity, Source};
    use crate::shell::Owner;
    use crate::shell::program_status::{Kind, Record, State};
    use std::collections::HashSet;

    fn record(id: Option<&str>, state: State) -> Record {
        Record {
            id: id.map(str::to_string),
            state,
            kind: None,
            progress: None,
            app: Some("cargo".to_string()),
            title: None,
            msg: None,
        }
    }

    fn root(state: State) -> Record {
        record(None, state)
    }

    fn probed(
        id: &str,
        owner: &Owner,
        records: Option<Vec<Record>>,
    ) -> (String, Owner, Option<Vec<Record>>) {
        (id.to_string(), owner.clone(), records)
    }

    #[test]
    fn watcher_first_sight_is_baseline() {
        let mut w = ProgramWatcher::default();
        let news = w.observe("s1", &[root(State::Blocked)]);
        assert!(news.is_empty(), "a first sight is baseline only");
        assert!(is_reportable(State::Blocked));
    }

    #[test]
    fn watcher_reports_each_entry_into_blocked_done_error() {
        let mut w = ProgramWatcher::default();
        assert!(w.observe("s1", &[root(State::Working)]).is_empty());
        let blocked = w.observe("s1", &[root(State::Blocked)]);
        assert_eq!(blocked.len(), 1, "working → blocked is news");
        assert_eq!(blocked[0].state, State::Blocked);
        assert!(w.observe("s1", &[root(State::Blocked)]).is_empty());
        let done = w.observe("s1", &[root(State::Done)]);
        assert_eq!(done.len(), 1, "blocked → done is news");
        assert_eq!(done[0].state, State::Done);
        let error = w.observe("s1", &[root(State::Done), record(Some("p2"), State::Error)]);
        assert_eq!(error.len(), 1, "a second program's error is the only news");
        assert_eq!(error[0].id.as_deref(), Some("p2"));
    }

    #[test]
    fn watcher_never_reports_idle_or_working() {
        let mut w = ProgramWatcher::default();
        assert!(w.observe("s1", &[root(State::Idle)]).is_empty());
        assert!(w.observe("s1", &[root(State::Working)]).is_empty());
        assert!(
            w.observe("s1", &[record(Some("p"), State::Working)])
                .is_empty()
        );
    }

    #[test]
    fn watcher_forgets_a_shell_dropped_by_retain() {
        let mut w = ProgramWatcher::default();
        assert!(w.observe("s1", &[root(State::Blocked)]).is_empty());
        let mut live = HashSet::new();
        live.insert("s2".to_string());
        w.retain(&live);
        assert!(
            w.observe("s1", &[root(State::Blocked)]).is_empty(),
            "a shell seen again after retain is a first sight"
        );
    }

    #[test]
    fn program_notice_maps_state_to_severity() {
        let blocked = program_notice("s1", &Owner::User, &root(State::Blocked));
        assert_eq!(blocked.severity, Severity::Warning);
        assert_eq!(blocked.source, Source::Shell);
        assert_eq!(blocked.source_id, "s1");
        assert_eq!(
            blocked.labels.get("state").map(String::as_str),
            Some("blocked")
        );
        assert_eq!(
            blocked.labels.get("kind").map(String::as_str),
            None,
            "no kind label without a kind"
        );
        assert_eq!(
            blocked.labels.get("program_id").map(String::as_str),
            None,
            "no program_id label for the root record"
        );
        let errored = program_notice("s1", &Owner::User, &root(State::Error));
        assert_eq!(errored.severity, Severity::Error);
        let done = program_notice("s1", &Owner::User, &root(State::Done));
        assert_eq!(done.severity, Severity::Info);
        let mut with_kind = root(State::Blocked);
        with_kind.kind = Some(Kind::Permission);
        with_kind.id = Some("p1".to_string());
        let kinded = program_notice("s1", &Owner::User, &with_kind);
        assert_eq!(
            kinded.labels.get("kind").map(String::as_str),
            Some("permission")
        );
        assert_eq!(
            kinded.labels.get("program_id").map(String::as_str),
            Some("p1")
        );
    }

    #[test]
    fn program_notice_targets_only_a_chat_owner() {
        let chat = program_notice("s1", &Owner::Chat("c1".to_string()), &root(State::Done));
        assert_eq!(chat.session.as_deref(), Some("c1"));
        let ghost = program_notice("s1", &Owner::Ghost("g1".to_string()), &root(State::Done));
        assert_eq!(ghost.session, None, "a ghost's shells target no session");
        let sched = program_notice(
            "s1",
            &Owner::Scheduler("j1".to_string()),
            &root(State::Done),
        );
        assert_eq!(sched.session, None, "a scheduler shell targets no session");
    }

    #[test]
    fn program_notice_name_falls_back_to_the_shell_id() {
        let mut unnamed = root(State::Done);
        unnamed.app = None;
        let n = program_notice("s7", &Owner::User, &unnamed);
        assert_eq!(n.name, "s7", "no app falls back to the shell id");
        let named = program_notice("s7", &Owner::User, &root(State::Done));
        assert_eq!(named.name, "cargo");
        assert!(named.body.is_empty(), "the notice body stays empty");
        assert!(
            named.title.starts_with("[Shell s7] Program cargo (done)"),
            "the title carries the shell id and program_line: {}",
            named.title
        );
    }

    #[test]
    fn collect_notices_skips_an_unanswered_probe() {
        let owner = Owner::User;
        let mut w = ProgramWatcher::default();
        let records = vec![root(State::Blocked)];
        let baseline = collect_notices(&mut w, vec![probed("s1", &owner, Some(records.clone()))]);
        assert!(baseline.is_empty(), "first sight is baseline only");
        let hiccup = collect_notices(&mut w, vec![probed("s1", &owner, None)]);
        assert!(hiccup.is_empty(), "an unanswered probe yields nothing");
        let same = collect_notices(&mut w, vec![probed("s1", &owner, Some(records))]);
        assert!(
            same.is_empty(),
            "the same records after a None are not replayed"
        );
        let transition = collect_notices(
            &mut w,
            vec![probed("s1", &owner, Some(vec![root(State::Done)]))],
        );
        assert_eq!(transition.len(), 1, "a later transition is still reported");
        assert_eq!(transition[0].source_id, "s1");
    }
}
