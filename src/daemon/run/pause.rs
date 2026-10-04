//! Runs paused from attached mode (M23). `Ctrl-p` stops a shell's foreground
//! job; the run that owns the shell is marked paused here and parks at its
//! next turn boundary. `Ctrl-p` again resumes the job and leaves the run a
//! one-shot notice of how long it was paused, which it reads when it wakes.
//!
//! Typing into the shell while attached is noted here too: the run is told,
//! at its next turn boundary, how many bytes the user typed and where. Only
//! the count is kept — never what was typed.
//!
//! Keyed by run id, like the cancel registry. An entry is swept when the
//! run's `ActiveRunGuard` drops.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use super::RunId;
use crate::util::UnpoisonExt;

/// One run's attached-mode state: paused now (the shell and since when), the
/// notice a resume leaves for the run (the shell and how long), and the
/// bytes the user typed since the run last looked (the first shell and the
/// total).
#[derive(Default)]
struct PauseEntry {
    paused: Option<(String, tokio::time::Instant)>,
    notice: Option<(String, Duration)>,
    typed: Option<(String, usize)>,
}

static PAUSES: OnceLock<Mutex<HashMap<String, PauseEntry>>> = OnceLock::new();

fn pauses() -> &'static Mutex<HashMap<String, PauseEntry>> {
    PAUSES.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Mark `run_id` paused by the user through `shell`. A run already paused
/// keeps its first shell and start time.
pub fn pause_run(run_id: &RunId, shell: &str) {
    let mut map = pauses().lock().unwrap_or_log();
    let entry = map.entry(run_id.as_str().to_string()).or_default();
    if entry.paused.is_none() {
        entry.paused = Some((shell.to_string(), tokio::time::Instant::now()));
    }
}

/// Resume `run_id`: clear the pause and leave the run a notice of how long
/// it lasted. Returns that duration, or `None` when the run was not paused.
pub fn resume_run(run_id: &RunId) -> Option<Duration> {
    let mut map = pauses().lock().unwrap_or_log();
    let entry = map.get_mut(run_id.as_str())?;
    let (shell, since) = entry.paused.take()?;
    let lasted = since.elapsed();
    entry.notice = Some((shell, lasted));
    Some(lasted)
}

/// Whether `run_id` is paused now.
pub fn is_paused(run_id: &RunId) -> bool {
    pauses()
        .lock()
        .unwrap_or_log()
        .get(run_id.as_str())
        .is_some_and(|entry| entry.paused.is_some())
}

/// Take the notice a resume left for `run_id`: the shell and how long it was
/// paused. `None` once taken, or when there was no pause.
pub fn take_pause_notice(run_id: &RunId) -> Option<(String, Duration)> {
    pauses()
        .lock()
        .unwrap_or_log()
        .get_mut(run_id.as_str())
        .and_then(|entry| entry.notice.take())
}

/// Note that the user typed `bytes` bytes into `shell` while attached to a
/// shell `run_id` owns. Counts add up until the run takes them; the first
/// shell named is kept.
pub fn note_typed(run_id: &RunId, shell: &str, bytes: usize) {
    let mut map = pauses().lock().unwrap_or_log();
    let entry = map.entry(run_id.as_str().to_string()).or_default();
    let (first, total) = entry.typed.take().unwrap_or((shell.to_string(), 0));
    entry.typed = Some((first, total + bytes));
}

/// Take what the user typed for `run_id` since it last looked: the shell and
/// the byte count. `None` once taken, or when nothing was typed.
pub fn take_typed_notice(run_id: &RunId) -> Option<(String, usize)> {
    pauses()
        .lock()
        .unwrap_or_log()
        .get_mut(run_id.as_str())
        .and_then(|entry| entry.typed.take())
}

/// Note typing for the run in flight for the owner of an attached shell.
/// Returns the run noted.
pub fn note_owner_typed(owner_key: &str, shell: &str, bytes: usize) -> Option<RunId> {
    let run_id = super::active::owner_run(owner_key)?;
    note_typed(&run_id, shell, bytes);
    Some(run_id)
}

/// Forget everything about `run_id`. Called when its `ActiveRunGuard` drops.
pub(super) fn clear_pause(run_id: &str) {
    pauses().lock().unwrap_or_log().remove(run_id);
}

/// Pause the run in flight for the owner of an attached shell (`chat:<s>` or
/// `ghost:<s>`). Returns the run paused.
pub fn pause_owner_run(owner_key: &str, shell: &str) -> Option<RunId> {
    let run_id = super::active::owner_run(owner_key)?;
    pause_run(&run_id, shell);
    Some(run_id)
}

/// Resume the run in flight for the owner of an attached shell. Returns the
/// run resumed, if it was paused.
pub fn resume_owner_run(owner_key: &str) -> Option<RunId> {
    let run_id = super::active::owner_run(owner_key)?;
    resume_run(&run_id).map(|_| run_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::run::cancel::test_support::fresh_run_id;

    #[tokio::test(start_paused = true)]
    async fn pause_resume_leaves_one_notice_of_how_long() {
        let run = fresh_run_id();
        assert!(!is_paused(&run));
        assert_eq!(
            resume_run(&run),
            None,
            "resuming an unpaused run is nothing"
        );

        pause_run(&run, "s5");
        assert!(is_paused(&run));
        tokio::time::advance(Duration::from_secs(10)).await;
        pause_run(&run, "s9");
        tokio::time::advance(Duration::from_secs(30)).await;
        assert_eq!(
            resume_run(&run),
            Some(Duration::from_secs(40)),
            "a second pause keeps the first start"
        );
        assert!(!is_paused(&run));
        assert_eq!(
            take_pause_notice(&run),
            Some(("s5".to_string(), Duration::from_secs(40))),
            "the notice names the first shell"
        );
        assert_eq!(take_pause_notice(&run), None, "the notice is one-shot");

        pause_run(&run, "s5");
        clear_pause(run.as_str());
        assert!(!is_paused(&run), "a cleared run is not paused");
    }

    #[test]
    fn pause_owner_run_reaches_the_owners_run_in_flight() {
        let run = fresh_run_id();
        let session = "pause-owner-session-1";
        let _active_guard = crate::daemon::run::register_active(session, &run);
        assert_eq!(
            pause_owner_run(&format!("ghost:{session}"), "s5"),
            Some(run.clone())
        );
        assert!(is_paused(&run));
        assert_eq!(
            resume_owner_run(&format!("ghost:{session}")),
            Some(run.clone())
        );
        assert!(!is_paused(&run));
        assert_eq!(
            resume_owner_run(&format!("ghost:{session}")),
            None,
            "a run that is not paused is not resumed"
        );
        assert_eq!(pause_owner_run("adopted", "s5"), None);
        assert_eq!(pause_owner_run("chat:pause-owner-idle", "s5"), None);
    }

    #[test]
    fn typed_notices_add_up_and_are_taken_once() {
        let run = fresh_run_id();
        assert_eq!(take_typed_notice(&run), None);
        note_typed(&run, "s5", 3);
        note_typed(&run, "s6", 4);
        assert_eq!(
            take_typed_notice(&run),
            Some(("s5".to_string(), 7)),
            "counts add up under the first shell"
        );
        assert_eq!(take_typed_notice(&run), None, "the notice is one-shot");

        let session = "typed-owner-session-1";
        let _active_guard = crate::daemon::run::register_active(session, &run);
        assert_eq!(
            note_owner_typed(&format!("ghost:{session}"), "s5", 2),
            Some(run.clone())
        );
        assert_eq!(take_typed_notice(&run), Some(("s5".to_string(), 2)));
        assert_eq!(note_owner_typed("adopted", "s5", 2), None);
    }

    #[test]
    fn dropping_the_active_guard_clears_the_pause() {
        let run = fresh_run_id();
        {
            let _active_guard = crate::daemon::run::register_active("pause-guard-session-1", &run);
            pause_run(&run, "s5");
            assert!(is_paused(&run));
        }
        assert!(!is_paused(&run), "a finished run leaves no pause behind");
    }
}
