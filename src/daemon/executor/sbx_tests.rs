use super::*;
use crate::config::SandboxProfile;

fn probe(exit_ok: bool, stdout: &str, stderr: &str) -> Probe {
    Probe {
        spawn_error: None,
        exit_ok,
        stdout: stdout.to_string(),
        stderr: stderr.to_string(),
    }
}

#[test]
fn staged_script_command_points_at_the_sandbox_copy() {
    assert_eq!(
        staged_script_command("restart.sh", " --now"),
        "/de/scripts/restart.sh --now"
    );
    assert_eq!(
        staged_script_command("a.sh", ""),
        "/de/scripts/a.sh",
        "an argument-less invocation appends nothing"
    );
    assert_eq!(
        staged_script_command("restart.sh", " --now"),
        format!("{STAGED_SCRIPTS_DIR}/restart.sh --now"),
        "the dir constant is the source of the path"
    );
}

#[test]
fn stage_args_match_the_measured_shapes() {
    assert_eq!(
        stage_prep_args("de-ghost-r1"),
        vec![
            "exec".to_string(),
            "-u".to_string(),
            "root".to_string(),
            "de-ghost-r1".to_string(),
            "sh".to_string(),
            "-c".to_string(),
            "mkdir -p /de/scripts && chown agent:agent /de/scripts".to_string(),
        ]
    );
    assert_eq!(
        stage_cp_args("/h/s.sh", "de-ghost-r1", "s.sh"),
        vec![
            "cp".to_string(),
            "/h/s.sh".to_string(),
            "de-ghost-r1:/de/scripts/s.sh".to_string(),
        ]
    );
}

#[test]
fn script_name_is_safe_rejects_path_and_shell_characters() {
    for ok in ["ok.sh", "backup-db_2.sh"] {
        assert!(script_name_is_safe(ok), "`{ok}` is a safe name");
    }
    for bad in [
        "", "../x", "a/b", "a b", "a;b", "a&b", "a|b", "$(x)", "a`b", "a'b", "a\"b", "a\nb",
    ] {
        assert!(!script_name_is_safe(bad), "`{bad:?}` is not a safe name");
    }
}

#[test]
fn script_invocation_skips_sudo_and_unknown_scripts() {
    let exists = |n: &str| n == "restart.sh";
    assert_eq!(
        script_invocation("restart.sh --now", exists),
        Some(("restart.sh".to_string(), " --now".to_string())),
        "a bare script name followed by its arguments"
    );
    assert_eq!(
        script_invocation("restart.sh", exists),
        Some(("restart.sh".to_string(), String::new())),
        "a bare script name with no arguments"
    );
    assert_eq!(
        script_invocation("sudo restart.sh --now", exists),
        None,
        "sudo is never staged"
    );
    assert_eq!(
        script_invocation("unknown.sh --flag", exists),
        None,
        "an unknown script is an ordinary command"
    );
    assert_eq!(
        script_invocation("ls -la", exists),
        None,
        "a basename that is not a script is an ordinary command"
    );
}

#[test]
fn script_invocation_accepts_an_absolute_path_under_scripts_dir() {
    let _home = crate::test_home_guard();
    let exists = |n: &str| n == "restart.sh";
    let cmd = format!(
        "{}/restart.sh --now",
        crate::config::scripts_dir().display()
    );
    assert_eq!(
        script_invocation(&cmd, exists),
        Some(("restart.sh".to_string(), " --now".to_string())),
        "a resolved path inside the scripts directory stages like a bare name"
    );
}

#[test]
fn script_invocation_rejects_an_absolute_path_outside_scripts_dir() {
    let exists = |n: &str| n == "restart.sh";
    assert_eq!(
        script_invocation("/usr/bin/restart.sh --now", exists),
        None,
        "an absolute path outside the scripts directory is never staged"
    );
}

fn spawn_probe(kind: std::io::ErrorKind) -> Probe {
    Probe {
        spawn_error: Some(kind),
        exit_ok: false,
        stdout: String::new(),
        stderr: String::new(),
    }
}

fn profile(network: &str, allow: &[&str], deny: &[&str]) -> SandboxProfile {
    SandboxProfile {
        network: network.to_string(),
        proxy_allow: allow.iter().map(|s| s.to_string()).collect(),
        proxy_deny: deny.iter().map(|s| s.to_string()).collect(),
    }
}

#[test]
fn sandbox_name_prefixes_by_kind() {
    assert_eq!(sandbox_name(SandboxKind::Ghost, "r1"), "de-ghost-r1");
    assert_eq!(sandbox_name(SandboxKind::Chat, "s1"), "de-chat-s1");
}

#[test]
fn sandbox_name_sanitizes_the_id() {
    assert_eq!(
        sandbox_name(SandboxKind::Ghost, "Run_7.x/y z"),
        "de-ghost-run-7-x-y-z"
    );
    assert_eq!(sandbox_name(SandboxKind::Ghost, "é"), "de-ghost--");
    assert_eq!(sandbox_name(SandboxKind::Ghost, ""), "de-ghost-x");
}

#[test]
fn sandbox_name_caps_at_63_characters() {
    let id = "a".repeat(100);
    let name = sandbox_name(SandboxKind::Ghost, &id);
    assert_eq!(name.len(), 63);
    assert!(name.starts_with("de-ghost-"));
}

#[test]
fn sandbox_name_never_contains_an_underscore() {
    for id in ["a_b", "__", "x_y_z"] {
        let name = sandbox_name(SandboxKind::Ghost, id);
        assert!(!name.contains('_'), "id `{id}` gave `{name}`");
    }
}

#[test]
fn is_daemon_sandbox_matches_only_daemon_prefixes() {
    for name in ["de-ghost-r1", "de-chat-s1"] {
        assert!(is_daemon_sandbox(name), "`{name}` should match");
    }
    for name in [
        "de-probe-a",
        "de-ghost",
        "my-de-ghost-1",
        "De-Ghost-1",
        "claude-daemoneye",
    ] {
        assert!(!is_daemon_sandbox(name), "`{name}` should not match");
    }
}

#[test]
fn network_plan_none_ignores_lists() {
    let p = profile("none", &["a.example"], &["b.example"]);
    assert_eq!(network_plan(&p), Ok(NetworkPlan::None));
}

#[test]
fn network_plan_proxy_copies_and_trims() {
    let p = profile("proxy", &[" a.example ", "", "  "], &["b.example"]);
    assert_eq!(
        network_plan(&p),
        Ok(NetworkPlan::Proxy {
            allow: vec!["a.example".to_string()],
            deny: vec!["b.example".to_string()],
        })
    );
}

#[test]
fn network_plan_rejects_unknown_modes() {
    for mode in ["None", " none", "", "host"] {
        let p = profile(mode, &[], &[]);
        let err = network_plan(&p).expect_err("unknown mode should be an Err");
        assert!(err.contains(mode), "message `{err}` misses `{mode}`");
    }
}

#[test]
fn create_args_none_denies_everything() {
    assert_eq!(
        create_args("de-ghost-r1", &NetworkPlan::None, "1g", 2.0),
        vec![
            "create",
            "--name",
            "de-ghost-r1",
            "-q",
            "--pull",
            "missing",
            "--memory",
            "1g",
            "--cpus",
            "2",
            "--deny-network",
            "**",
            "shell",
        ]
    );
}

#[test]
fn create_args_proxy_lists_each_deny_and_no_allow() {
    let plan = NetworkPlan::Proxy {
        allow: vec!["a.example".to_string(), "b.example".to_string()],
        deny: vec!["a.example".to_string(), "b.example".to_string()],
    };
    let argv = create_args("n", &plan, "1g", 2.0);
    let refs: Vec<&str> = argv.iter().map(String::as_str).collect();
    assert_eq!(
        refs,
        vec![
            "create",
            "--name",
            "n",
            "-q",
            "--pull",
            "missing",
            "--memory",
            "1g",
            "--cpus",
            "2",
            "--deny-network",
            "a.example",
            "--deny-network",
            "b.example",
            "shell",
        ]
    );
}

#[test]
fn create_args_proxy_with_no_deny_has_no_deny_flag() {
    let plan = NetworkPlan::Proxy {
        allow: vec!["a.example".to_string()],
        deny: vec![],
    };
    let argv = create_args("n", &plan, "1g", 2.0);
    assert!(!argv.iter().any(|a| a == "--deny-network"));
}

#[test]
fn create_args_always_pulls_missing_and_ends_with_shell() {
    let plans = [
        NetworkPlan::None,
        NetworkPlan::Proxy {
            allow: vec!["a.example".to_string()],
            deny: vec!["b.example".to_string()],
        },
    ];
    for plan in &plans {
        let argv = create_args("n", plan, "2g", 4.0);
        assert_eq!(argv.last().unwrap(), "shell");
        let i = argv.iter().position(|a| a == "--pull").unwrap();
        assert_eq!(argv[i + 1], "missing");
    }
}

#[test]
fn create_args_renders_cpus_as_an_integer() {
    let plan = NetworkPlan::None;
    for (cpus, expected) in [
        (2.0, "2"),
        (1.5, "2"),
        (0.0, "1"),
        (-3.0, "1"),
        (f64::NAN, "1"),
        (f64::INFINITY, "1"),
    ] {
        let argv = create_args("n", &plan, "1g", cpus);
        let i = argv.iter().position(|a| a == "--cpus").unwrap();
        assert_eq!(argv[i + 1], expected, "cpus {cpus}");
        assert!(!argv[i + 1].contains('.'), "cpus {cpus} has a decimal");
    }
}

#[test]
fn exec_and_policy_args_match_the_measured_shapes() {
    assert_eq!(
        policy_allow_args("de-ghost-r1", "api.github.com"),
        vec![
            "policy",
            "allow",
            "network",
            "--sandbox",
            "de-ghost-r1",
            "api.github.com"
        ]
    );
    assert_eq!(
        shell_exec_args("de-ghost-r1", None),
        vec!["exec", "-it", "de-ghost-r1", "bash"]
    );
    assert_eq!(
        shell_exec_args("de-ghost-r1", Some("/w")),
        vec!["exec", "-it", "-w", "/w", "de-ghost-r1", "bash"]
    );
    assert_eq!(
        exec_args("de-ghost-r1", &["kill", "-STOP", "--", "-42"]),
        vec!["exec", "de-ghost-r1", "kill", "-STOP", "--", "-42"]
    );
}

#[test]
fn stop_and_rm_args_are_empty_for_no_names() {
    assert!(stop_args(&[]).is_empty());
    assert!(rm_args(&[]).is_empty());
    let names = vec!["a".to_string(), "b".to_string()];
    assert_eq!(stop_args(&names), vec!["stop", "a", "b"]);
    assert_eq!(rm_args(&names), vec!["rm", "-f", "a", "b"]);
}

#[test]
fn parse_ls_json_reads_the_measured_shape() {
    let text = r#"{"sandboxes": [
            {"name": "a", "id": "id-a", "agent": "shell", "status": "running",
             "last_used_at": "t1", "created_at": "t2", "workspaces": ["/w"]},
            {"name": "b", "id": "id-b", "agent": "shell", "status": "exited",
             "last_used_at": "t3", "created_at": "t4"}
        ]}"#;
    let listed = parse_ls_json(text).expect("measured shape parses");
    assert_eq!(listed.len(), 2);
    assert_eq!(
        listed[0],
        SandboxInfo {
            name: "a".into(),
            status: "running".into()
        }
    );
    assert_eq!(
        listed[1],
        SandboxInfo {
            name: "b".into(),
            status: "exited".into()
        }
    );
}

#[test]
fn parse_ls_json_reads_an_empty_list() {
    assert_eq!(parse_ls_json(r#"{"sandboxes": []}"#), Some(Vec::new()));
}

#[test]
fn parse_ls_json_rejects_non_json_and_wrong_shapes() {
    assert_eq!(parse_ls_json("No sandboxes found."), None);
    assert_eq!(parse_ls_json("{}"), None);
    assert_eq!(parse_ls_json("[]"), None);
}

#[test]
fn stale_daemon_sandboxes_skips_live_and_foreign() {
    let listed = vec![
        SandboxInfo {
            name: "de-ghost-a".into(),
            status: "running".into(),
        },
        SandboxInfo {
            name: "de-ghost-b".into(),
            status: "running".into(),
        },
        SandboxInfo {
            name: "de-probe-a".into(),
            status: "running".into(),
        },
        SandboxInfo {
            name: "claude-x".into(),
            status: "running".into(),
        },
    ];
    let live = vec!["de-ghost-b".to_string()];
    assert_eq!(
        stale_daemon_sandboxes(&listed, &live),
        vec!["de-ghost-a".to_string()]
    );
}

#[test]
fn classify_preflight_accepts_the_measured_outputs() {
    let version = probe(
        true,
        "sbx version: v0.46.0 991967dc90ce0d9a440cd1df1bdf3e395c5a2693\n",
        "",
    );
    let daemon = probe(true, "Status: running\n", "");
    assert_eq!(
        classify_preflight(&version, &daemon),
        Ok("v0.46.0".to_string())
    );
}

#[test]
fn classify_preflight_maps_each_failure() {
    let daemon = probe(true, "Status: running\n", "");
    let version = probe(
        true,
        "sbx version: v0.46.0 991967dc90ce0d9a440cd1df1bdf3e395c5a2693\n",
        "",
    );

    // Version probe wins: keychain WARN printed by a still-successful run.
    let warn = probe(
        true,
        "sbx version: v0.46.0 x\n",
        "WARN the OS keychain is locked; unlock your keyring and retry\n",
    );
    assert_eq!(
        classify_preflight(&warn, &daemon),
        Err(SbxUnavailable::KeychainLocked)
    );

    let auth = probe(
        true,
        "sbx version: v0.46.0 x\n",
        "error: Not authenticated to Docker\n",
    );
    assert_eq!(
        classify_preflight(&auth, &daemon),
        Err(SbxUnavailable::NotAuthenticated)
    );

    let both = probe(
        true,
        "sbx version: v0.46.0 x\n",
        "Not authenticated to Docker; the OS keychain is locked\n",
    );
    assert_eq!(
        classify_preflight(&both, &daemon),
        Err(SbxUnavailable::KeychainLocked)
    );

    // Daemon probe wins over a good version probe.
    let daemon_locked = probe(
        true,
        "Status: running\n",
        "WARN the OS keychain is locked\n",
    );
    assert_eq!(
        classify_preflight(&version, &daemon_locked),
        Err(SbxUnavailable::KeychainLocked)
    );
    let daemon_auth = probe(
        true,
        "Status: running\n",
        "error: Not authenticated to Docker\n",
    );
    assert_eq!(
        classify_preflight(&version, &daemon_auth),
        Err(SbxUnavailable::NotAuthenticated)
    );

    assert_eq!(
        classify_preflight(&spawn_probe(std::io::ErrorKind::NotFound), &daemon),
        Err(SbxUnavailable::NotInstalled)
    );
    assert_eq!(
        classify_preflight(&version, &spawn_probe(std::io::ErrorKind::PermissionDenied)),
        Err(SbxUnavailable::SpawnFailed {
            detail: "PermissionDenied".to_string()
        })
    );

    let stopped = probe(true, "Status: stopped\n", "");
    assert_eq!(
        classify_preflight(&version, &stopped),
        Err(SbxUnavailable::DaemonNotRunning {
            detail: "Status: stopped".to_string()
        })
    );

    let wrong = probe(true, "sbx 0.46\n", "");
    assert_eq!(
        classify_preflight(&wrong, &daemon),
        Err(SbxUnavailable::UnrecognisedVersion {
            output: "sbx 0.46".to_string()
        })
    );
}

#[test]
fn describe_unavailable_names_the_fix() {
    let cases = vec![
        (SbxUnavailable::NotInstalled, vec!["sbx"]),
        (
            SbxUnavailable::SpawnFailed {
                detail: "d".to_string(),
            },
            vec!["d"],
        ),
        (SbxUnavailable::NotAuthenticated, vec!["sbx login"]),
        (SbxUnavailable::KeychainLocked, vec!["keychain", "unlock"]),
        (
            SbxUnavailable::DaemonNotRunning {
                detail: "x".to_string(),
            },
            vec!["sbx daemon start", "x"],
        ),
        (
            SbxUnavailable::UnrecognisedVersion {
                output: "y".to_string(),
            },
            vec!["y"],
        ),
    ];
    for (reason, must_contain) in cases {
        let msg = describe_unavailable(&reason);
        assert!(
            msg.starts_with("sandbox unavailable: "),
            "`{msg}` has no prefix for {reason:?}"
        );
        for needle in must_contain {
            assert!(
                msg.contains(needle),
                "`{msg}` misses `{needle}` for {reason:?}"
            );
        }
    }
}

#[test]
fn tagged_shell_exec_args_match_the_measured_shape() {
    let argv = tagged_shell_exec_args("de-ghost-r1", "s3");
    let refs: Vec<&str> = argv.iter().map(String::as_str).collect();
    assert_eq!(
        refs,
        vec!["exec", "-it", "-e", "DE_SHELL=s3", "de-ghost-r1", "bash"]
    );
}

#[test]
fn vm_signal_args_pass_tag_and_op_as_positionals() {
    for (op, word) in [
        (VmOp::Find, "FIND"),
        (VmOp::Stop, "STOP"),
        (VmOp::Cont, "CONT"),
        (VmOp::Hup, "HUP"),
    ] {
        let argv = vm_signal_args("de-ghost-r1", "s3", op);
        let refs: Vec<&str> = argv.iter().map(String::as_str).collect();
        assert_eq!(
            refs,
            vec![
                "exec",
                "de-ghost-r1",
                "sh",
                "-c",
                VM_SIGNAL_SCRIPT,
                "vmsig",
                "s3",
                word
            ],
            "op {word}"
        );
    }
}

#[test]
fn vm_signal_script_is_valid_posix_sh() {
    let status = std::process::Command::new("sh")
        .args(["-n", "-c", VM_SIGNAL_SCRIPT])
        .status()
        .unwrap_or_else(|e| panic!("sh should spawn: {e}"));
    assert!(status.success(), "sh -n rejected the script: {status}");
}

#[test]
fn vm_signal_script_matches_the_measured_text() {
    for line in [
        r#"[ "$sid" = "$pid" ] || continue"#,
        r#"tr '\0' '\n' < "/proc/$ppid/environ" 2>/dev/null | grep -qx "DE_SHELL=$id" && continue"#,
        r#"[ -n "$bash_pid" ] || { echo "no-shell"; exit 3; }"#,
        r#"STOP) kill -STOP "$bash_pid"; [ "$fg" != "$bash_pid" ] && kill -STOP -- "-$fg"; echo "stopped bash=$bash_pid fg=$fg";;"#,
    ] {
        assert!(
            VM_SIGNAL_SCRIPT.contains(line),
            "script misses the measured line `{line}`"
        );
    }
}

#[test]
fn parse_vm_find_reads_the_measured_lines() {
    let idle = parse_vm_find("bash=366 fg=366 state=S\n").expect("idle line parses");
    assert_eq!(
        idle,
        VmShellState {
            bash_pid: 366,
            fg_pgid: 366,
            state: 'S'
        }
    );
    assert!(!idle.is_paused());
    assert!(!idle.has_foreground_job());

    let running = parse_vm_find("bash=366 fg=414 state=S").expect("running line parses");
    assert_eq!(running.fg_pgid, 414);
    assert!(running.has_foreground_job());
    assert!(!running.is_paused());

    let paused = parse_vm_find("bash=366 fg=450 state=T").expect("paused line parses");
    assert!(paused.is_paused());
    assert!(paused.has_foreground_job());
}

#[test]
fn parse_vm_find_rejects_other_output() {
    for text in [
        "no-shell",
        "",
        "bash=x fg=1 state=S",
        "bash=1 fg=1",
        "bash=1 fg=1 state=ST",
        "fg=1 bash=1 state=S",
    ] {
        assert_eq!(parse_vm_find(text), None, "text `{text}` should reject");
    }
}

#[test]
fn vm_op_outcome_maps_exit_codes() {
    assert_eq!(
        vm_op_outcome(Some(0), "stopped bash=1 fg=2", ""),
        VmOpOutcome::Done
    );
    assert_eq!(
        vm_op_outcome(Some(3), "no-shell\n", ""),
        VmOpOutcome::NoShell
    );
    assert_eq!(
        vm_op_outcome(Some(3), "other", ""),
        VmOpOutcome::Failed("other".to_string())
    );
    assert_eq!(
        vm_op_outcome(Some(1), "", "error: sandbox 'x' not found"),
        VmOpOutcome::Failed("error: sandbox 'x' not found".to_string())
    );
    assert_eq!(
        vm_op_outcome(None, "", ""),
        VmOpOutcome::Failed("killed by a signal".to_string())
    );
}

#[test]
fn sbx_target_from_host_reads_a_tagged_shell() {
    let args: Vec<String> = tagged_shell_exec_args("de-ghost-r1", "s3");
    assert_eq!(
        sbx_target_from_host("/usr/bin/sbx", &args),
        Some(SbxTarget {
            sandbox: "de-ghost-r1".to_string(),
            tag: "s3".to_string()
        })
    );
    assert_eq!(
        sbx_target_from_host("sbx", &args),
        Some(SbxTarget {
            sandbox: "de-ghost-r1".to_string(),
            tag: "s3".to_string()
        })
    );
}

#[test]
fn sbx_target_from_host_rejects_everything_else() {
    let tagged: Vec<String> = tagged_shell_exec_args("de-ghost-r1", "s3");
    assert_eq!(
        sbx_target_from_host("/usr/bin/ssh", &tagged),
        None,
        "wrong program"
    );
    let untagged = vec![
        "exec".to_string(),
        "-it".to_string(),
        "de-ghost-r1".to_string(),
        "bash".to_string(),
    ];
    assert_eq!(sbx_target_from_host("sbx", &untagged), None, "untagged");
    let wrong_var = vec![
        "exec".to_string(),
        "-it".to_string(),
        "-e".to_string(),
        "FOO=s3".to_string(),
        "de-ghost-r1".to_string(),
        "bash".to_string(),
    ];
    assert_eq!(sbx_target_from_host("sbx", &wrong_var), None, "wrong var");
    let empty_tag = vec![
        "exec".to_string(),
        "-it".to_string(),
        "-e".to_string(),
        "DE_SHELL=".to_string(),
        "de-ghost-r1".to_string(),
        "bash".to_string(),
    ];
    assert_eq!(sbx_target_from_host("sbx", &empty_tag), None, "empty tag");
    let seventh: Vec<String> = tagged
        .iter()
        .chain(["extra".to_string()].iter())
        .cloned()
        .collect();
    assert_eq!(sbx_target_from_host("sbx", &seventh), None, "seventh arg");
    assert_eq!(
        sbx_target_from_host("/usr/bin/sbx-helper", &tagged),
        None,
        "sbx-helper program"
    );
}

#[test]
fn sbx_target_round_trips_the_builder() {
    let args = tagged_shell_exec_args("de-chat-x", "s7");
    assert_eq!(
        sbx_target_from_host("sbx", &args),
        Some(SbxTarget {
            sandbox: "de-chat-x".to_string(),
            tag: "s7".to_string()
        })
    );
}

#[test]
fn policy_log_args_match_the_measured_shape() {
    assert_eq!(
        policy_log_args("de-ghost-r1"),
        vec![
            "policy".to_string(),
            "log".to_string(),
            "de-ghost-r1".to_string(),
            "--json".to_string(),
        ]
    );
}

#[test]
fn parse_policy_log_reads_the_measured_output() {
    let text = r#"{
  "blocked_hosts": [
    {
      "host": "example.com:443",
      "vm_name": "de-probe-audit",
      "proxy_type": "forward",
      "rule": "no applicable policies for op(action=net:connect:tcp, resource=net:domain:example.com:443)",
      "last_seen": "2026-10-07T13:22:09.930609713-07:00",
      "since": "2026-10-07T13:22:09.930609713-07:00",
      "count_since": 1,
      "reason": "No matching allow rule (default deny)"
    }
  ],
  "allowed_hosts": [
    {
      "host": "github.com:443",
      "vm_name": "de-probe-audit",
      "proxy_type": "forward-bypass",
      "rule": "",
      "last_seen": "2026-10-07T13:22:10.315805171-07:00",
      "since": "2026-10-07T13:22:10.315805171-07:00",
      "count_since": 1
    },
    {
      "host": "security.ubuntu.com:80",
      "vm_name": "de-probe-audit",
      "proxy_type": "forward",
      "rule": "",
      "last_seen": "2026-10-07T13:22:10.14940568-07:00",
      "since": "2026-10-07T13:22:10.14940568-07:00",
      "count_since": 1
    },
    {
      "host": "download.docker.com:443",
      "vm_name": "de-probe-audit",
      "proxy_type": "forward-bypass",
      "rule": "",
      "last_seen": "2026-10-07T13:22:09.71884377-07:00",
      "since": "2026-10-07T13:22:09.71884377-07:00",
      "count_since": 1
    }
  ]
}"#;

    let audit = parse_policy_log(text, "de-probe-audit").expect("the measured output parses");

    assert_eq!(
        audit.blocked,
        vec![EgressEntry {
            host: "example.com:443".to_string(),
            vm_name: "de-probe-audit".to_string(),
            count_since: 1,
        }],
        "the blocked host carries its count"
    );
    assert_eq!(audit.allowed.len(), 3, "three allowed hosts");
    assert_eq!(audit.allowed[0].host, "github.com:443");
    assert_eq!(
        audit.allowed[0],
        EgressEntry {
            host: "github.com:443".to_string(),
            vm_name: "de-probe-audit".to_string(),
            count_since: 1,
        },
        "the extra fields, including `reason`, must not break parsing"
    );
}

#[test]
fn parse_policy_log_drops_other_sandboxes_and_rejects_garbage() {
    let text = r#"{"blocked_hosts": [], "allowed_hosts": [
            {"host": "a.example:443", "vm_name": "de-ghost-a", "count_since": 2},
            {"host": "b.example:443", "vm_name": "de-ghost-b", "count_since": 5}
        ]}"#;

    let audit = parse_policy_log(text, "de-ghost-a").expect("the log parses");

    assert_eq!(
        audit,
        EgressAudit {
            allowed: vec![EgressEntry {
                host: "a.example:443".to_string(),
                vm_name: "de-ghost-a".to_string(),
                count_since: 2,
            }],
            blocked: Vec::new(),
        },
        "the entry naming another sandbox is dropped"
    );

    assert_eq!(parse_policy_log("not json", "de-ghost-a"), None);

    assert_eq!(
        parse_policy_log(
            r#"{"blocked_hosts": [], "allowed_hosts": []}"#,
            "de-ghost-a"
        ),
        Some(EgressAudit::default()),
        "a sandbox that never existed still parses to an empty audit"
    );
}

#[test]
fn egress_summary_lists_hosts_sorted_with_counts() {
    let audit =
        |allowed: Vec<EgressEntry>, blocked: Vec<EgressEntry>| EgressAudit { allowed, blocked };
    let entry = |host: &str, count: u64| EgressEntry {
        host: host.to_string(),
        vm_name: "de-ghost-r1".to_string(),
        count_since: count,
    };

    assert_eq!(
        egress_summary(&audit(
            vec![entry("zeta.io:443", 2), entry("alpha.io:443", 1)],
            vec![entry("example.com:443", 1)],
        )),
        "sandbox egress: allowed alpha.io:443 (1), zeta.io:443 (2); blocked example.com:443 (1)",
        "hosts render sorted by host, each with its count"
    );
    assert_eq!(
        egress_summary(&audit(vec![entry("a.io:443", 3)], Vec::new())),
        "sandbox egress: allowed a.io:443 (3); blocked none",
        "an empty side renders as none"
    );
    assert_eq!(
        egress_summary(&EgressAudit::default()),
        "sandbox egress: no network requests",
        "an empty audit is the no-requests note"
    );
}

#[test]
fn egress_event_carries_the_sandbox_and_hosts() {
    let audit = EgressAudit {
        allowed: vec![EgressEntry {
            host: "github.com:443".to_string(),
            vm_name: "de-ghost-r1".to_string(),
            count_since: 1,
        }],
        blocked: vec![EgressEntry {
            host: "example.com:443".to_string(),
            vm_name: "de-ghost-r1".to_string(),
            count_since: 1,
        }],
    };

    let event = egress_event("de-ghost-r1", &audit);

    assert_eq!(event["sandbox"], "de-ghost-r1");
    assert_eq!(event["allowed"][0]["host"], "github.com:443");
    assert_eq!(event["allowed"][0]["count_since"], 1);
    assert_eq!(event["blocked"].as_array().map(Vec::len), Some(1));
}
