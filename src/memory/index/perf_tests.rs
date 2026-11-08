//! Tests the phase-18 perf seams: one index connection per path, and the
//! once-per-path reconcile of an indefinitely empty corpus. The close and
//! count hooks are `#[cfg(test)]` items of the index module so the opened
//! count is read without touching production code paths.

use super::*;

/// Drop the process's cached index connection (if any) so the caller can
/// observe the next one being opened. Test-only.
#[test]
fn index_connection_is_opened_once_per_path() {
    let _guard = crate::test_home_guard();
    let tmp = tempfile::tempdir().unwrap();
    unsafe { std::env::set_var("HOME", tmp.path()) };
    crate::config::Config::ensure_dirs().ok();

    let path = crate::config::memory_index_path();
    super::close_index_conn(&path);
    std::fs::remove_file(&path).ok();

    let after_first;
    {
        let _conn = open_index();
        after_first = super::index_open_attempts();
    }
    let cache_hit_probe;
    {
        let _conn = open_index();
        cache_hit_probe = super::index_open_attempts();
    }
    assert_eq!(
        cache_hit_probe, after_first,
        "the second open on the same path returns the cached connection, not a new one"
    );
}

#[test]
fn index_connection_follows_a_home_change() {
    let guard = crate::test_home_guard();
    let first = tempfile::tempdir().unwrap();
    unsafe { std::env::set_var("HOME", first.path()) };
    crate::config::Config::ensure_dirs().ok();
    std::fs::create_dir_all(crate::config::events_dir()).unwrap();
    crate::daemon::log_event("canary", serde_json::json!({"msg": "one home only"}));
    // Hold the connection across the HOME change so the slot cannot be
    // re-keyed by the next open; the next open under the second HOME should
    // see only the second HOME's database.
    let _hold = open_index().unwrap();

    let second = tempfile::tempdir().unwrap();
    unsafe { std::env::set_var("HOME", second.path()) };
    crate::config::Config::ensure_dirs().ok();
    let conn = open_index().unwrap();
    let hits: i64 = conn
        .query_row(
            "SELECT count(*) FROM events WHERE events MATCH 'canary'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    println!("legacy hits counted: {hits}");
    assert_eq!(
        hits, 0,
        "a second HOME's index never shows the first's events"
    );
    drop(conn);
    drop(_hold);
    drop(guard);
}

#[test]
fn an_empty_corpus_is_reconciled_once_per_path() {
    let _guard = crate::test_home_guard();
    let tmp = tempfile::tempdir().unwrap();
    unsafe { std::env::set_var("HOME", tmp.path()) };
    crate::config::Config::ensure_dirs().ok();

    let before = super::corpus_reconcile_count();
    for _ in 0..3 {
        crate::memory::index::search_epochs("never compacted", 4);
    }
    let after = super::corpus_reconcile_count();
    assert_eq!(
        after - before,
        1,
        "an empty corpus reconciles once per path, not once per search"
    );
}
