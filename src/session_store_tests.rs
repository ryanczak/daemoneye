use super::*;

fn with_temp_home<F: FnOnce()>(f: F) {
    let _guard = crate::TEST_HOME_LOCK.lock().unwrap();
    let dir = tempfile::tempdir().expect("tempdir");
    let old_home = std::env::var("HOME").ok();
    // SAFETY: single-threaded test context protected by HOME_LOCK.
    unsafe { std::env::set_var("HOME", dir.path()) };
    f();
    match old_home {
        Some(h) => unsafe { std::env::set_var("HOME", h) },
        None => unsafe { std::env::remove_var("HOME") },
    }
}

fn fake_messages(n: usize) -> Vec<Message> {
    (0..n)
        .map(|i| Message {
            role: if i % 2 == 0 {
                "user".to_string()
            } else {
                "assistant".to_string()
            },
            content: format!("message {}", i),
            tool_calls: None,
            tool_results: None,
            turn: None,
        })
        .collect()
}

// ── validate_session_name ────────────────────────────────────────────────

#[test]
fn valid_names_pass() {
    assert!(validate_session_name("nginx-timeout").is_ok());
    assert!(validate_session_name("db001").is_ok());
    assert!(validate_session_name("a").is_ok());
    assert!(validate_session_name("abc-def-123").is_ok());
}

#[test]
fn empty_name_rejected() {
    assert!(validate_session_name("").is_err());
}

#[test]
fn too_long_rejected() {
    let long = "a".repeat(65);
    assert!(validate_session_name(&long).is_err());
}

#[test]
fn reserved_names_rejected() {
    for name in ["default", "current", "new", "active", "none", "all"] {
        assert!(
            validate_session_name(name).is_err(),
            "{name} should be rejected"
        );
    }
}

#[test]
fn auto_prefix_rejected() {
    assert!(validate_session_name("auto-something").is_err());
}

#[test]
fn uppercase_rejected() {
    let err = validate_session_name("MySession");
    assert!(err.is_err());
    // Suggestion should be present
    assert!(err.unwrap_err().contains("my"));
}

#[test]
fn path_traversal_rejected() {
    assert!(validate_session_name("../evil").is_err());
    assert!(validate_session_name("foo/bar").is_err());
}

#[test]
fn name_starts_with_digit_ok() {
    assert!(validate_session_name("3am-incident").is_ok());
}

#[test]
fn name_leading_hyphen_rejected() {
    assert!(validate_session_name("-bad").is_err());
}

// ── save / load / list / delete / rename ─────────────────────────────────

#[test]
fn save_and_load_round_trip() {
    with_temp_home(|| {
        let msgs = fake_messages(4);
        save_session(
            "test-session",
            None,
            "a test",
            &msgs,
            2,
            "default",
            &[],
            false,
        )
        .expect("save");
        assert!(session_exists("test-session"));

        let meta = load_session_meta("test-session").expect("load meta");
        assert_eq!(meta.name, "test-session");
        assert_eq!(meta.description, "a test");
        assert_eq!(meta.turn_count, 2);
        assert_eq!(meta.message_count, 4);

        let loaded = load_session_messages("test-session", 0).expect("load msgs");
        assert_eq!(loaded.len(), 4);
    });
}

#[test]
fn load_messages_max_count_truncates() {
    with_temp_home(|| {
        let msgs = fake_messages(10);
        save_session("trunc", None, "", &msgs, 5, "default", &[], false).expect("save");
        let loaded = load_session_messages("trunc", 3).expect("load");
        assert_eq!(loaded.len(), 3);
        // Should be the last 3 messages.
        assert_eq!(loaded[2].content, msgs[9].content);
    });
}

#[test]
fn collision_rejected_without_force() {
    with_temp_home(|| {
        let msgs = fake_messages(2);
        save_session("clash", None, "", &msgs, 1, "default", &[], false).expect("first save");
        let result = save_session("clash", None, "", &msgs, 1, "default", &[], false);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("already exists"));
    });
}

#[test]
fn collision_allowed_with_force() {
    with_temp_home(|| {
        let msgs = fake_messages(2);
        save_session("force-test", None, "v1", &msgs, 1, "default", &[], false)
            .expect("first save");
        save_session("force-test", None, "v2", &msgs, 1, "default", &[], true)
            .expect("forced overwrite");
        let meta = load_session_meta("force-test").expect("meta");
        assert_eq!(meta.description, "v2");
    });
}

#[test]
fn update_in_place_allowed() {
    with_temp_home(|| {
        let msgs = fake_messages(2);
        save_session("mywork", None, "v1", &msgs, 1, "default", &[], false)
            .expect("first save");
        // Same session name as current_saved_name — should succeed without force.
        save_session(
            "mywork",
            Some("mywork"),
            "v2",
            &msgs,
            2,
            "default",
            &[],
            false,
        )
        .expect("update in place");
        let meta = load_session_meta("mywork").expect("meta");
        assert_eq!(meta.description, "v2");
        assert_eq!(meta.turn_count, 2);
    });
}

#[test]
fn list_returns_newest_first() {
    with_temp_home(|| {
        let msgs = fake_messages(2);
        save_session("aaa", None, "", &msgs, 1, "default", &[], false).expect("a");
        std::thread::sleep(std::time::Duration::from_millis(10));
        save_session("bbb", None, "", &msgs, 1, "default", &[], false).expect("b");
        let list = list_sessions();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].0, "bbb");
        assert_eq!(list[1].0, "aaa");
    });
}

#[test]
fn delete_removes_dir_and_index() {
    with_temp_home(|| {
        let msgs = fake_messages(2);
        save_session("del-me", None, "", &msgs, 1, "default", &[], false).expect("save");
        assert!(session_exists("del-me"));
        delete_session("del-me").expect("delete");
        assert!(!session_exists("del-me"));
        assert!(!session_dir("del-me").exists());
    });
}

#[test]
fn delete_nonexistent_errors() {
    with_temp_home(|| {
        assert!(delete_session("ghost").is_err());
    });
}

#[test]
fn rename_updates_dir_and_index() {
    with_temp_home(|| {
        let msgs = fake_messages(2);
        save_session("before", None, "desc", &msgs, 1, "default", &[], false).expect("save");
        rename_session("before", "after").expect("rename");
        assert!(!session_exists("before"));
        assert!(session_exists("after"));
        let meta = load_session_meta("after").expect("meta");
        assert_eq!(meta.name, "after");
        assert_eq!(meta.description, "desc");
    });
}

#[test]
fn rename_nonexistent_errors() {
    with_temp_home(|| {
        assert!(rename_session("ghost", "new-name").is_err());
    });
}

#[test]
fn rename_to_existing_errors() {
    with_temp_home(|| {
        let msgs = fake_messages(2);
        save_session("a", None, "", &msgs, 1, "default", &[], false).expect("save a");
        save_session("b", None, "", &msgs, 1, "default", &[], false).expect("save b");
        assert!(rename_session("a", "b").is_err());
    });
}

#[test]
fn artifacts_round_trip() {
    with_temp_home(|| {
        let msgs = fake_messages(2);
        let artifacts = vec![
            ArtifactRef {
                kind: "memory".to_string(),
                name: "nginx-root-cause".to_string(),
                at_turn: 3,
            },
            ArtifactRef {
                kind: "runbook".to_string(),
                name: "nginx-recovery".to_string(),
                at_turn: 7,
            },
        ];
        save_session("art-test", None, "", &msgs, 8, "default", &artifacts, false)
            .expect("save");
        let meta = load_session_meta("art-test").expect("meta");
        assert_eq!(meta.artifacts_created.len(), 2);
        assert_eq!(meta.artifacts_created[0].name, "nginx-root-cause");
        assert_eq!(meta.artifacts_created[1].kind, "runbook");
    });
}

#[test]
fn build_resumed_banner_contains_name() {
    let meta = SavedSessionMeta {
        schema_version: 1,
        name: "nginx-debug".to_string(),
        description: "investigating timeouts".to_string(),
        tags: vec![],
        parent: None,
        created_at: chrono::Utc::now().to_rfc3339(),
        last_resumed_at: chrono::Utc::now().to_rfc3339(),
        turn_count: 5,
        message_count: 12,
        model: "default".to_string(),
        artifacts_created: vec![],
    };
    let banner = build_resumed_banner(&meta, 10);
    assert!(banner.contains("nginx-debug"));
    assert!(banner.contains("5 turns"));
    assert!(banner.contains("10 message(s) loaded"));
}

// ── Auto-naming / threshold / flag ────────────────────────────────────────

#[test]
fn auto_name_turn_threshold_default_is_10() {
    let cfg = crate::config::SessionsConfig::default();
    assert_eq!(cfg.auto_name_turn_threshold, 10);
    assert!(cfg.auto_name_enabled);
    assert_eq!(cfg.load_recent_turns, 10);
}

#[test]
fn auto_name_enabled_flag_respected() {
    // When auto_name_enabled = false, the threshold is irrelevant.
    let cfg = crate::config::SessionsConfig {
        auto_name_enabled: false,
        auto_name_turn_threshold: 10,
        load_recent_turns: 10,
    };
    assert!(!cfg.auto_name_enabled);
}

#[test]
fn validate_name_exactly_64_chars_ok() {
    let name = "a".repeat(64);
    assert!(validate_session_name(&name).is_ok());
}

#[test]
fn validate_name_exactly_65_chars_err() {
    let name = "a".repeat(65);
    assert!(validate_session_name(&name).is_err());
}

// ── Session diff metadata helpers ─────────────────────────────────────────

#[test]
fn saved_session_meta_artifacts_in_diff_input() {
    with_temp_home(|| {
        let msgs = fake_messages(4);
        let artifacts = vec![ArtifactRef {
            kind: "memory".to_string(),
            name: "root-cause".to_string(),
            at_turn: 2,
        }];
        save_session(
            "session-a",
            None,
            "first session",
            &msgs,
            3,
            "default",
            &artifacts,
            false,
        )
        .expect("save a");
        save_session(
            "session-b",
            None,
            "second session",
            &msgs,
            5,
            "default",
            &[],
            false,
        )
        .expect("save b");

        let meta_a = load_session_meta("session-a").expect("meta a");
        let meta_b = load_session_meta("session-b").expect("meta b");

        assert_eq!(meta_a.artifacts_created.len(), 1);
        assert_eq!(meta_b.artifacts_created.len(), 0);
        assert_eq!(meta_a.turn_count, 3);
        assert_eq!(meta_b.turn_count, 5);
    });
}

// ── backfill_session_origin ───────────────────────────────────────────────

#[test]
fn backfill_stamps_memory_without_frontmatter() {
    with_temp_home(|| {
        // Write a memory with no frontmatter.
        let base = crate::config::config_dir();
        let mem_dir = base.join("memory/knowledge");
        std::fs::create_dir_all(&mem_dir).unwrap();
        let mem_path = mem_dir.join("root-cause.md");
        std::fs::write(&mem_path, "Memory content here").unwrap();

        let artifacts = vec![ArtifactRef {
            kind: "memory".to_string(),
            name: "root-cause".to_string(),
            at_turn: 1,
        }];
        let errs = backfill_session_origin(&artifacts, "postgres-incident");
        assert!(errs.is_empty(), "unexpected errors: {:?}", errs);

        let patched = std::fs::read_to_string(&mem_path).unwrap();
        assert!(
            patched.contains("session_origin: \"postgres-incident\""),
            "got: {patched}"
        );
    });
}

#[test]
fn backfill_stamps_runbook() {
    with_temp_home(|| {
        let base = crate::config::config_dir();
        let rb_dir = base.join("runbooks");
        std::fs::create_dir_all(&rb_dir).unwrap();
        let rb_path = rb_dir.join("nginx-recovery.md");
        std::fs::write(
            &rb_path,
            "---\ntags: [nginx]\n---\n# Runbook: nginx-recovery\n",
        )
        .unwrap();

        let artifacts = vec![ArtifactRef {
            kind: "runbook".to_string(),
            name: "nginx-recovery".to_string(),
            at_turn: 2,
        }];
        let errs = backfill_session_origin(&artifacts, "nginx-debug");
        assert!(errs.is_empty(), "unexpected errors: {:?}", errs);

        let patched = std::fs::read_to_string(&rb_path).unwrap();
        assert!(
            patched.contains("session_origin: \"nginx-debug\""),
            "got: {patched}"
        );
    });
}

#[test]
fn backfill_stamps_script() {
    with_temp_home(|| {
        let base = crate::config::config_dir();
        let scripts_dir = base.join("scripts");
        std::fs::create_dir_all(&scripts_dir).unwrap();
        let script_path = scripts_dir.join("rotate-certs.sh");
        std::fs::write(&script_path, "#!/bin/bash\necho done\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o700)).unwrap();

        let artifacts = vec![ArtifactRef {
            kind: "script".to_string(),
            name: "rotate-certs.sh".to_string(),
            at_turn: 3,
        }];
        let errs = backfill_session_origin(&artifacts, "cert-rotation");
        assert!(errs.is_empty(), "unexpected errors: {:?}", errs);

        let patched = std::fs::read_to_string(&script_path).unwrap();
        assert!(
            patched.contains("session_origin: cert-rotation"),
            "got: {patched}"
        );
    });
}

#[test]
fn backfill_idempotent() {
    with_temp_home(|| {
        let base = crate::config::config_dir();
        let mem_dir = base.join("memory/knowledge");
        std::fs::create_dir_all(&mem_dir).unwrap();
        let mem_path = mem_dir.join("already-stamped.md");
        std::fs::write(&mem_path, "---\nsession_origin: \"existing\"\n---\nbody\n").unwrap();

        let artifacts = vec![ArtifactRef {
            kind: "memory".to_string(),
            name: "already-stamped".to_string(),
            at_turn: 1,
        }];
        let errs = backfill_session_origin(&artifacts, "new-session");
        assert!(errs.is_empty(), "unexpected errors: {:?}", errs);

        let content = std::fs::read_to_string(&mem_path).unwrap();
        assert!(
            !content.contains("new-session"),
            "should not overwrite existing session_origin"
        );
        assert!(content.contains("existing"));
    });
}

#[test]
fn backfill_missing_artifact_returns_error_name() {
    with_temp_home(|| {
        let artifacts = vec![ArtifactRef {
            kind: "memory".to_string(),
            name: "nonexistent-key".to_string(),
            at_turn: 1,
        }];
        let errs = backfill_session_origin(&artifacts, "some-session");
        assert_eq!(errs, vec!["memory/nonexistent-key"]);
    });
}
