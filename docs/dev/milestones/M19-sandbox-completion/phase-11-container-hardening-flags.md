# Phase 11: Container hardening — the flags, the digest pin, and a spawn record

**Milestone:** M19 — Sandbox Completion
**Status:** todo
**Depends on:** nothing (independent of the proxy chain and of 02's staging)
**Estimated diff:** ~230 lines including tests, across four files
**Tags:** language=rust, kind=feature, size=m

## Goal

Six gaps between what the design promises and what `run_args` actually sets,
each measured absent today and each measured effective against the real
`daemoneye-agent-base` image on the daemon host:

1. **`--memory-swap`** — docker defaults it to **2×** `--memory`, so the
   documented 1 GiB cap permits 2 GiB.
2. **`--read-only`** plus a writable `/tmp` tmpfs — the image's filesystem is
   not a scratch space.
3. **`--cap-drop=ALL`** and **`--security-opt=no-new-privileges`** — the
   second is the one with teeth: the process is already uid 1000, but Alpine
   ships setuid busybox links, and this closes that escalation path.
4. **`--pull=never`** — `sandbox_preflight` fails closed on a missing image,
   but caches its verdict in a `OnceLock` for the daemon's lifetime, so an
   image deleted *after* startup leaves a window in which docker would resolve
   the name against `docker.io`.
5. **A digest-pinned base image** — `containers/Dockerfile` says
   `FROM alpine:3.22`, a moving tag, where the design pins by digest.
6. **A `container_run` event at spawn** — `events.jsonl` records `job_start`,
   `job_complete` and `gc_window` for a background job, and nothing that says a
   container ran or which image it was. This is the audit anchor phase-10's
   live checks need, and it is an exit criterion in its own right.

## Architecture references

- `docs/dev/milestones/M19-sandbox-completion/README.md` § Phases, **11
  intent** — items 1–4 and the three folded in on 2026-08-30 (digest pin,
  `container_run` event, staleness warning). Items 1–6 above are this phase;
  **the staleness warning and `requires_tools` are not** (§ Out of scope).
- Same README, exit criteria: *"Every sandboxed container run is recorded in
  `events.jsonl` at spawn — job id, session, image id, network mode — so a
  live check can be anchored to a record rather than to a `docker ps`
  snapshot."* Task 3 is that criterion.
- `CLAUDE.md` § "Important Invariants": *"Every `events.jsonl` record carries
  `ts`, `event`, and `pid`; `log_event` stamps `pid` itself, so call sites
  must not pass one."* The payload in Task 2 passes neither.
- Same README, § Notes, the Docker Sandboxes comparison — the anti-pattern
  named there stands: **an agent sandbox never mounts the Docker API socket.**
  Nothing in this phase goes near it.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any file.
4. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

Measured on the tree at drafting time (2026-08-31, commit `699d54f`). **The
whole change was prototyped end-to-end before this doc was written, built,
linted, tested and mutated, and the flag set was run against real containers,
then reverted** — every code block in § Spec is that prototype after
`cargo fmt --all`, and every count in § Acceptance criteria was read off it.

- `cargo test --lib` → **1522 passed; 0 failed; 4 ignored**. All four gates
  green.
- Every added string is absent from `src/daemon/executor/container.rs`.
  `grep -cF` for `"--memory-swap".to_string(),`, `"--read-only".to_string(),`,
  `"no-new-privileges".to_string(),`, `"never".to_string(),`,
  `"ALL".to_string(),`, `mode=1777` and `pub fn container_run_event(` → **0**,
  seven for seven.
- `run_args` sets exactly **one** `--tmpfs` today (`sed -n '/^pub fn
  run_args/,/^}/p' … | grep -c '"--tmpfs"'` → `1`).
- `grep -c 'container_run' src/daemon/background/run.rs` → **0**.
- `grep -c '^FROM alpine:'` → **1** in `containers/Dockerfile` and **1** in
  `containers/proxy/Dockerfile`; `grep -c '^FROM alpine@sha256:'` → **0** in
  both.
- `grep -rc "allow(dead_code)" src/ | awk -F: '{s+=$2} END {print s}'` → **6**.

### Live measurements (architect, rootless Docker on the daemon host)

Run 2026-08-31 against the real `daemoneye-agent-base`, all containers removed
afterwards. **Every flag below was verified effective in-kernel or in
`docker inspect`, not merely accepted by the CLI, and each has its contrast
case.** Nothing in this phase spawns docker during tests.

1. **The whole flag set runs, and the toolchain still works.** With
   `--read-only --cap-drop ALL --security-opt no-new-privileges --pull never`
   and both tmpfs mounts:

   ```text
   whoami: 1000:1000
   CapEff:  0000000000000000
   CapBnd:  0000000000000000
   NoNewPrivs:  1
   de/work writable: yes
   /tmp writable: yes
   touch: cannot touch '/rootfs-probe': Read-only file system
   python ok
   git version 2.49.1
   curl 8.14.1 (x86_64-alpine-linux-musl) …
   ```

2. **`--memory-swap`, with its contrast.** `docker inspect` on a container run
   with the flag: `Memory=1073741824 MemorySwap=1073741824`. The **same
   container without it**: `Memory=1073741824 MemorySwap=2147483648`. Two
   gigabytes where the config says one.

3. **The flags reach the runtime.** `ReadonlyRootfs=true CapDrop=[ALL]
   SecurityOpt=[no-new-privileges]`.

4. **`--pull=never`, with its contrast.** Against an image that does not exist
   locally: with the flag, `docker: Error response from daemon: No such image:
   alpine:3.99` — a local failure. Without it: `Unable to find image
   'alpine:3.99' locally` followed by a `docker.io/library/alpine:3.99`
   resolution attempt. That reach for the registry is what the flag closes.

5. **Pinning the digest does not force a rebuild, and this was checked rather
   than assumed.** `alpine:3.22` currently resolves to
   `sha256:14358309a308569c32bdc37e2e0e9694be33a9d99e68afb0f5ff33cc1f695dce`,
   so both Dockerfiles build to **byte-identical image ids**: the agent image
   built from the pinned file is `sha256:0d02bebd…`, exactly the `image_id`
   already in `~/.daemoneye/etc/sandbox.lock`. **So `sandbox_preflight` keeps
   passing and no operator action is required by this phase.** (Had the ids
   differed, every sandboxed command would have been refused until
   `daemoneye sandbox build` was re-run — worth stating, because that is the
   failure this measurement ruled out.)

6. **The image id for the record comes from the lockfile, not a probe.**
   `sandbox_preflight` refuses to run anything when the live image differs
   from `sandbox.lock`, so whenever a job reaches the spawn site the two
   agree, and the lock is readable without spawning a process. Confirmed live:
   `sandbox.lock`'s `image_id` and `docker image inspect --format '{{.Id}}'`
   are the same string today.

## Gotchas

1. **`--read-only` and the `/tmp` tmpfs arrive together or not at all.** A
   read-only root with no writable `/tmp` breaks ordinary tooling in ways that
   surface as unrelated errors much later. `/de/work` stays `0700` and private
   to the run_as uid; `/tmp` is `1777`, because that is what programs expect of
   it. One test pins both modes for exactly this reason.

2. **`sandbox_exec_run_args_match_the_prototyped_vector` is *supposed* to
   change here.** Task 1 edits its pinned argv vector, and that edit **is**
   the phase's real acceptance test — unlike phases 04, 05 and 13, where the
   same vector was pinned as unchanged. Update it to the vector given in
   Task 1 verbatim. Do **not** change any other existing test.

3. **Flag order inside the vector is load-bearing for that test, not for
   docker.** Paste `run_args` exactly as Task 1 gives it; a correct set in a
   different order fails a pinned-vector comparison for no runtime reason, and
   the fix is to match the spec rather than to loosen the test.

4. **`log_event` stamps `ts` and `pid` itself.** The `container_run` payload
   passes neither.

5. **Never restore a mutation with `git checkout`.** It discards this round's
   own uncommitted work, not just the mutation. Restore with the inverse
   `patch`, always.

## Spec

Four tasks, then four mutation pairs and the evidence capture. Every code
block below is the architect's prototype verbatim, post-`cargo fmt --all`.
Paste it; do not retype it.

### Task 1 — The flags, in `src/daemon/executor/container.rs`

Replace the whole of `run_args` with the block below. The only changes from
today are the six inserted lines-pairs and the second `--tmpfs`; everything
else is byte-identical to what is already there.

```rust
pub fn run_args(cfg: &SandboxConfig, spec: &ExecSpec, session_id: Option<&str>) -> Vec<String> {
    let Some((uid, gid)) = split_run_as(&cfg.run_as) else {
        log::warn!(
            "sandbox enabled but run_as={:?} unparseable; falling back to running on the host",
            cfg.run_as
        );
        return Vec::new();
    };
    let mut args = vec![
        "--host".to_string(),
        cfg.docker_host.clone(),
        "run".to_string(),
        "--rm".to_string(),
        "--user".to_string(),
        cfg.run_as.clone(),
        "--network".to_string(),
        spec.network.to_string(),
        "--memory".to_string(),
        cfg.limits.memory.clone(),
        // Docker defaults --memory-swap to 2x --memory, so the documented 1g
        // cap actually permits 2 GiB. Equal values disable swap entirely.
        "--memory-swap".to_string(),
        cfg.limits.memory.clone(),
        "--pids-limit".to_string(),
        cfg.limits.pids.to_string(),
        "--cpus".to_string(),
        cfg.limits.cpus.to_string(),
        // The image's filesystem is not a scratch space: /de/work and /tmp are
        // the only writable paths, both tmpfs, both owned by the run_as uid.
        "--read-only".to_string(),
        "--cap-drop".to_string(),
        "ALL".to_string(),
        // The process is already uid 1000, but Alpine ships setuid busybox
        // links; this is what closes that escalation path.
        "--security-opt".to_string(),
        "no-new-privileges".to_string(),
        // sandbox_preflight caches its verdict for the daemon's lifetime, so an
        // image deleted after startup would otherwise be resolved against a
        // registry. Fail locally instead.
        "--pull".to_string(),
        "never".to_string(),
        "--tmpfs".to_string(),
        format!(
            "{}:rw,size={},mode=0700,uid={},gid={}",
            cfg.workdir, cfg.limits.scratch, uid, gid
        ),
        "--tmpfs".to_string(),
        format!(
            "/tmp:rw,size={},mode=1777,uid={uid},gid={gid}",
            cfg.limits.scratch
        ),
        "-v".to_string(),
        format!("{}:/de/scripts:ro", stage_volume_name(spec.job_id)),
    ];
    args.push("--label".to_string());
    args.push("de.sandbox=1".to_string());
    if spec.is_ghost {
        args.push("--label".to_string());
        args.push("de.ghost=1".to_string());
    }
    if let Some(sid) = session_id {
        args.push("--label".to_string());
        args.push(format!("de.session={sid}"));
    }
    if spec.network != "none" {
        args.extend(proxy_env_args(spec.job_id));
    }
    args.push("--workdir".to_string());
    args.push(cfg.workdir.clone());
    args.push(cfg.image.clone());
    args.push("sh".to_string());
    args.push("-lc".to_string());
    args.push(spec.command.to_string());
    args
}
```

Then update the pinned vector in `sandbox_exec_run_args_match_the_prototyped_vector`
— `old_str`:

```rust
                "--memory",
                "1g",
                "--pids-limit",
                "256",
                "--cpus",
                "2",
                "--tmpfs",
                "/de/work:rw,size=2g,mode=0700,uid=1000,gid=1000",
                "-v",
```

`new_str`:

```rust
                "--memory",
                "1g",
                "--memory-swap",
                "1g",
                "--pids-limit",
                "256",
                "--cpus",
                "2",
                "--read-only",
                "--cap-drop",
                "ALL",
                "--security-opt",
                "no-new-privileges",
                "--pull",
                "never",
                "--tmpfs",
                "/de/work:rw,size=2g,mode=0700,uid=1000,gid=1000",
                "--tmpfs",
                "/tmp:rw,size=2g,mode=1777,uid=1000,gid=1000",
                "-v",
```

That is the **only** existing test this phase may edit (§ Gotchas 2).

### Task 2 — The spawn record's payload, same file

Insert the block below **immediately before** the existing line

```rust
/// The command string a `de-bg-*` window should run for `raw_cmd`.
```

```rust
/// The `container_run` event payload for a sandboxed job, or `None` when the
/// sandbox is off and no container will exist.
///
/// The image id comes from the **lockfile**, not from a fresh `image inspect`:
/// [`sandbox_preflight`] refuses to run anything when the live image differs
/// from the lock, so whenever a job reaches this point the two agree, and the
/// lock is readable without spawning a process.
///
/// This is the audit anchor a live check binds to — a `docker ps` snapshot
/// races the `--rm` teardown, a record does not.
pub fn container_run_event(
    cfg: &SandboxConfig,
    lock: Option<&SandboxLock>,
    job_id: &str,
    job_name: &str,
    network: &str,
    session: Option<&str>,
) -> Option<serde_json::Value> {
    if !cfg.enabled {
        return None;
    }
    Some(serde_json::json!({
        "session": session.unwrap_or("-"),
        "job_id": job_id,
        "job_name": job_name,
        "image": cfg.image,
        "image_id": lock.map_or("unknown", |l| l.image_id.as_str()),
        "network": network,
    }))
}
```

### Task 3 — The caller, in `src/daemon/background/run.rs`

One edit, immediately after the existing `job_start` event.

- `old_str`:
  ```rust
          "job_start",
          serde_json::json!({
              "session": session_id.as_deref().unwrap_or("-"),
              "job_name": win_name,
              "pane": pane_id,
          }),
      );
  ```
- `new_str`:
  ```rust
          "job_start",
          serde_json::json!({
              "session": session_id.as_deref().unwrap_or("-"),
              "job_name": win_name,
              "pane": pane_id,
          }),
      );
      if let Some(payload) = crate::daemon::executor::container::container_run_event(
          &config.sandbox,
          crate::daemon::executor::container::read_lock().as_ref(),
          &job_id,
          &win_name,
          &network,
          session_id.as_deref(),
      ) {
          log_event("container_run", payload);
      }
  ```

**Note the leading indentation**: the `old_str` lines above are shown with the
two-space offset this document's list uses. Match the file's real indentation
(the `"job_start",` line is indented 8 spaces in `run.rs`), not this block's.
`log_event`, `job_id`, `win_name` and `network` are all already in scope here —
add no imports and no bindings.

### Task 4 — The digest pin, in both Dockerfiles

`containers/Dockerfile` and `containers/proxy/Dockerfile` each open with
`FROM alpine:3.22`. Replace that line in **both** files with:

```
FROM alpine@sha256:14358309a308569c32bdc37e2e0e9694be33a9d99e68afb0f5ff33cc1f695dce
```

That is the digest `alpine:3.22` resolves to today, read from the daemon host
(§ Live measurements 5). Change nothing else in either file.

### Task 5 — Tests, appended to `container.rs`'s existing `mod tests`

Append the whole block below at the end of the existing `mod tests`, before
its closing brace. Seven tests. No new test file.

```rust
    /// Every flag this phase adds, as an adjacent (flag, value) pair or a bare
    /// switch. Measured effective against the real image on the daemon host
    /// 2026-08-31 — see the phase doc's § Live measurements.
    #[test]
    fn sandbox_run_args_carry_every_hardening_flag() {
        let cfg = SandboxConfig::default();
        let args = run_args(
            &cfg,
            &ExecSpec {
                job_id: "j1",
                network: "none",
                is_ghost: false,
                command: "echo hi",
            },
            None,
        );
        let pair = |flag: &str, value: &str| args.windows(2).any(|w| w[0] == flag && w[1] == value);
        assert!(pair("--memory-swap", "1g"), "{args:?}");
        assert!(pair("--cap-drop", "ALL"), "{args:?}");
        assert!(pair("--security-opt", "no-new-privileges"), "{args:?}");
        assert!(pair("--pull", "never"), "{args:?}");
        assert!(args.iter().any(|a| a == "--read-only"), "{args:?}");
    }

    #[test]
    fn sandbox_run_args_cap_swap_at_the_memory_limit() {
        // Docker defaults --memory-swap to 2x --memory. Measured: without the
        // flag a 1g container reports MemorySwap=2147483648; with it, 1g.
        // The value must track limits.memory, not be a second literal.
        let mut cfg = SandboxConfig::default();
        cfg.limits.memory = "512m".to_string();
        let args = run_args(
            &cfg,
            &ExecSpec {
                job_id: "j1",
                network: "none",
                is_ghost: false,
                command: "echo hi",
            },
            None,
        );
        let swap = args
            .windows(2)
            .find(|w| w[0] == "--memory-swap")
            .map(|w| w[1].clone());
        assert_eq!(swap.as_deref(), Some("512m"), "{args:?}");
    }

    #[test]
    fn sandbox_run_args_give_a_read_only_root_two_writable_tmpfs() {
        // --read-only without a writable /tmp breaks ordinary tooling, so the
        // two arrive together or not at all. /de/work stays 0700 and private;
        // /tmp is 1777 because that is what programs expect of it.
        let cfg = SandboxConfig::default();
        let args = run_args(
            &cfg,
            &ExecSpec {
                job_id: "j1",
                network: "none",
                is_ghost: false,
                command: "echo hi",
            },
            None,
        );
        let mounts: Vec<&String> = args
            .windows(2)
            .filter(|w| w[0] == "--tmpfs")
            .map(|w| &w[1])
            .collect();
        assert_eq!(mounts.len(), 2, "{args:?}");
        assert!(
            mounts[0].starts_with("/de/work:rw,") && mounts[0].contains("mode=0700"),
            "{mounts:?}"
        );
        assert!(
            mounts[1].starts_with("/tmp:rw,") && mounts[1].contains("mode=1777"),
            "{mounts:?}"
        );
        assert!(args.iter().any(|a| a == "--read-only"), "{args:?}");
    }

    #[test]
    fn sandbox_images_pin_their_base_by_digest() {
        // The design pins the base image by digest; a moving tag would let a
        // rebuild change the contents under a lock that still matches.
        for (name, text) in [
            ("agent", include_str!("../../../containers/Dockerfile")),
            (
                "proxy",
                include_str!("../../../containers/proxy/Dockerfile"),
            ),
        ] {
            let from = text
                .lines()
                .find(|l| l.starts_with("FROM "))
                .unwrap_or_else(|| panic!("{name} Dockerfile has no FROM line"));
            assert!(
                from.contains("@sha256:"),
                "{name} Dockerfile pins a tag, not a digest: {from}"
            );
            assert!(
                !from.contains(':') || from.split("@sha256:").count() == 2,
                "{name}: {from}"
            );
        }
    }

    #[test]
    fn sandbox_container_run_event_names_the_image_and_the_network() {
        let cfg = SandboxConfig {
            enabled: true,
            ..Default::default()
        };
        let lock = SandboxLock {
            image: "daemoneye-agent-base".to_string(),
            image_id: "sha256:".to_string() + &"a".repeat(64),
            built_at: 42,
        };
        let event = container_run_event(
            &cfg,
            Some(&lock),
            "42-1712937600",
            "de-bg-42-1712937600-cargo-build",
            "de-net-42-1712937600",
            Some("s1"),
        )
        .expect("an enabled sandbox produces a record");
        assert_eq!(event["session"], "s1");
        assert_eq!(event["job_id"], "42-1712937600");
        assert_eq!(event["job_name"], "de-bg-42-1712937600-cargo-build");
        assert_eq!(event["image"], "daemoneye-agent-base");
        assert_eq!(event["image_id"], lock.image_id);
        assert_eq!(event["network"], "de-net-42-1712937600");
    }

    #[test]
    fn sandbox_container_run_event_is_absent_when_the_sandbox_is_off() {
        // No container exists, so no record claims one did.
        let cfg = SandboxConfig::default();
        assert!(!cfg.enabled);
        assert!(container_run_event(&cfg, None, "j1", "de-bg-j1", "none", None).is_none());
    }

    #[test]
    fn sandbox_container_run_event_survives_a_missing_lock() {
        // The lock is normally present — preflight refuses without one — but a
        // record that panics is worse than one that says "unknown".
        let cfg = SandboxConfig {
            enabled: true,
            ..Default::default()
        };
        let event = container_run_event(&cfg, None, "j1", "de-bg-j1", "none", None)
            .expect("an enabled sandbox produces a record");
        assert_eq!(event["image_id"], "unknown");
        assert_eq!(event["session"], "-");
    }
```

### Task 6 — Mutation pair M1: the swap cap really tracks the memory limit

Mutation edits go through your `patch` tool — **`sed -i`, `perl -i` and `>`
redirects into a source file are banned by your contract and `bash` will
refuse them. Restore with the inverse `patch`, never with `git checkout`**
(§ Gotchas 5). Append each marker and run to `/tmp/e2e-11.txt`. Run the gates
(§ End-to-end verification) only **after** all four pairs are restored.

1. **Apply.** `patch` `src/daemon/executor/container.rs`:
   - `old_str`:
     ```rust
             "--memory-swap".to_string(),
             cfg.limits.memory.clone(),
     ```
   - `new_str`:
     ```rust
             "--memory-swap".to_string(),
             "1g".to_string(),
     ```

   Then:
   ```sh
   echo "== M1 APPLIED ==" >> /tmp/e2e-11.txt
   cargo test --lib sandbox_ 2>&1 | grep -E "FAILED|^test result:" | sed 's/; finished in .*//' >> /tmp/e2e-11.txt
   grep -cF 'cfg.limits.memory.clone(),' src/daemon/executor/container.rs >> /tmp/e2e-11.txt
   ```
   Measured on the prototype: **exactly one test fails**,
   `sandbox_run_args_cap_swap_at_the_memory_limit`, and the `grep -c` prints
   `1` — it counts **2** on a correct tree (`--memory` and `--memory-swap`
   both take it) and **1** under the mutation. Stated because a `1` here is
   the *mutated* value, not a failed patch. A green suite means a profile that
   lowers `limits.memory` silently keeps the default swap ceiling.

2. **Restore.** The inverse `patch`, marker `== M1 RESTORED ==`, the same two
   commands. The `grep -c` prints `2`.

### Task 7 — Mutation pair M2: the read-only root is really set

Only after M1 is restored.

1. **Apply.** `patch` `src/daemon/executor/container.rs` — `old_str`:
   ```rust
           "--read-only".to_string(),
           "--cap-drop".to_string(),
   ```
   `new_str`:
   ```rust
           "--cap-drop".to_string(),
   ```

   Then, with the marker `== M2 APPLIED ==`, the same `cargo test` line and:
   ```sh
   grep -cF '"--read-only".to_string(),' src/daemon/executor/container.rs >> /tmp/e2e-11.txt
   ```
   Measured: **exactly three tests fail**, and they are
   `sandbox_exec_run_args_match_the_prototyped_vector`,
   `sandbox_run_args_carry_every_hardening_flag` and
   `sandbox_run_args_give_a_read_only_root_two_writable_tmpfs`. The `grep -c`
   prints `0`. **Three, not one** — this is the only flag pinned by all three
   tests. A different number means the patch landed somewhere else; record a
   blocker.

2. **Restore.** The inverse `patch`, marker `== M2 RESTORED ==`, the same two
   commands. The `grep -c` prints `1`.

### Task 8 — Mutation pair M3: no record when there is no container

Only after M2 is restored.

1. **Apply.** `patch` `src/daemon/executor/container.rs` — `old_str`:
   ```rust
       if !cfg.enabled {
           return None;
       }
       Some(serde_json::json!({
           "session": session.unwrap_or("-"),
   ```
   `new_str`:
   ```rust
       Some(serde_json::json!({
           "session": session.unwrap_or("-"),
   ```

   Then, with the marker `== M3 APPLIED ==`, the same `cargo test` line and:
   ```sh
   grep -c 'if !cfg.enabled {' src/daemon/executor/container.rs >> /tmp/e2e-11.txt
   ```
   Measured: **exactly one test fails**,
   `sandbox_container_run_event_is_absent_when_the_sandbox_is_off`, and the
   `grep -c` prints `3` — it counts **4** on a correct tree (three other
   functions in this file carry the same guard) and **3** under the mutation.
   Same shape as M1: the lower number is the mutated one, and the count is the
   *file's* total rather than this function's.

2. **Restore.** The inverse `patch`, marker `== M3 RESTORED ==`, the same two
   commands. The `grep -c` prints `4`.

### Task 9 — Mutation pair M4: the base image is really pinned by digest

Only after M3 is restored. A non-Rust file; `patch` works the same way.

1. **Apply.** `patch` `containers/Dockerfile`:
   - `old_str`: `FROM alpine@sha256:14358309a308569c32bdc37e2e0e9694be33a9d99e68afb0f5ff33cc1f695dce`
   - `new_str`: `FROM alpine:3.22`

   Then, with the marker `== M4 APPLIED ==`, the same `cargo test` line and:
   ```sh
   grep -c '^FROM alpine@sha256:' containers/Dockerfile >> /tmp/e2e-11.txt
   ```
   Measured: **exactly one test fails**,
   `sandbox_images_pin_their_base_by_digest`, and the `grep -c` prints `0`.

2. **Restore.** The inverse `patch`, marker `== M4 RESTORED ==`, the same two
   commands. The `grep -c` prints `1`.

The `grep -c` after **each** direction is not optional: a `patch` whose
`old_str` matches the wrong line fails silently, and a mutation that never
applied certifies a vacuous guard. **All four failing-test sets above were
measured, not estimated** — each mutation was applied to the prototype and the
suite read.

### Task 10 — Capture the end-to-end evidence

**The § End-to-end block appends (`>> /tmp/e2e-11.txt`). If you need to run it
a second time — for any reason — `rm -f /tmp/e2e-11.txt` first and run the
whole sequence again from Task 6.** Two executions otherwise leave two copies
in the file, the paste holds one, and the self-check prints `PASTE MISMATCH`.
**Never edit `/tmp/e2e-11.txt` or the pasted block to reconcile them.** Run
`cargo fmt --all` **before** the block so `fmt_exit` is a real `0`.

Every `test result:` line is piped through `sed 's/; finished in .*//'` so
per-run timings cannot cause a spurious mismatch. Do not add the suffix back.

Run the block in § End-to-end verification **verbatim and unmodified**, then
paste the resulting `/tmp/e2e-11.txt` into a new Update Log entry headed
`### Update — <date> (end-to-end verification)`. **The entry ends with the
self-check's verdict line, `PASTE MATCH`, bare on its own line after the
fenced block.**

## Acceptance criteria

**Every count below was read off the architect's prototype of this exact
change, not derived from the spec text.** Every grep below reads a file under
`src/` or `containers/`, never this doc, so the phase doc's own text cannot
satisfy one.

- [ ] In `src/daemon/executor/container.rs`, `grep -cF` prints **1** for each
      of `"--memory-swap".to_string(),`, `"--read-only".to_string(),`,
      `"no-new-privileges".to_string(),`, `"never".to_string(),`,
      `"ALL".to_string(),` and `pub fn container_run_event(`
      (**before: 0** for all six), and **3** for `mode=1777`
      (**before: 0**) — one in `run_args`, two in tests.
- [ ] `sed -n '/^pub fn run_args/,/^}/p' src/daemon/executor/container.rs | grep -c '"--tmpfs"'`
      prints `2` (**before: 1**) — `/de/work` and `/tmp`.
- [ ] `grep -cF 'cfg.limits.memory.clone(),' src/daemon/executor/container.rs`
      prints `2` (**before: 1**) — the swap cap tracks the limit rather than
      repeating a literal. This is M1's seam.
- [ ] `grep -c 'container_run' src/daemon/background/run.rs` prints `2`
      (**before: 0**) — the call and the event name.
- [ ] `grep -c '^FROM alpine@sha256:'` prints `1` in **both**
      `containers/Dockerfile` and `containers/proxy/Dockerfile`, and
      `grep -c '^FROM alpine:'` prints `0` in both (**before: 0, 0, 1, 1**).
- [ ] `cargo test --lib sandbox_run_args 2>&1 | grep -c "^test .* ok$"` prints
      `3` and
      `cargo test --lib sandbox_container_run_event 2>&1 | grep -c "^test .* ok$"`
      prints `3` (**before: 0, 0**). Counts, not exit statuses. **Both filters
      were checked against the test names they are meant to cover** — every
      test Task 5 adds matches one of them except
      `sandbox_images_pin_their_base_by_digest`, which M4 covers directly.
- [ ] `cargo test --lib` reports **1529** passing and `0 failed`
      (**before: 1522**), with `4 ignored` unchanged; and **`cargo test`
      (all targets)** is green.
- [ ] `grep -rc "allow(dead_code)" src/ | awk -F: '{s+=$2} END {print s}'`
      prints `6` (**unchanged**).
- [ ] `sed -n '1,/^#\[cfg(test)\]/p' src/daemon/executor/container.rs | grep -c '\.unwrap()\|\.expect('`
      prints `0` (**before: 0**).
- [ ] `git diff --name-only | grep -cE '^(src|containers|assets)/'` prints `4`
      — exactly the four code files this phase edits, and no fifth.
- [ ] The § End-to-end entry shows `== M1 APPLIED ==`, `== M3 APPLIED ==` and
      `== M4 APPLIED ==` each failing **exactly one** test, `== M2 APPLIED ==`
      failing **exactly three**, each the named test its task states, all four
      `RESTORED` runs green, with a `grep -c` line after each direction reading
      the value that task states.
- [ ] No new `#[allow(...)]` anywhere, no `unsafe`, no `TODO`.
- [ ] All four gates green: `cargo fmt --all`, `cargo build`,
      `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`.
- [ ] The § End-to-end entry contains the literal line `PASTE MATCH` (bare,
      with no surrounding backticks):
      `grep -c '^PASTE MATCH$' docs/dev/milestones/M19-sandbox-completion/phase-11-container-hardening-flags.md`
      prints `1`.

## Test plan

Seven unit tests in `container.rs`'s `mod tests`, given in full in Task 5, plus
the one pinned-vector edit Task 1 requires. **No other existing test changes.**
If one does, stop and record a blocker.

**The negative cases are the phase.**
`sandbox_run_args_cap_swap_at_the_memory_limit` sets `limits.memory` to
`512m` and demands the swap cap follow it, because a literal `"1g"` passes the
default-config test and silently fails every profile that lowers the limit
(M1 proves it). `sandbox_run_args_give_a_read_only_root_two_writable_tmpfs`
pins **both** mount modes and their order — `0700` for `/de/work`, `1777` for
`/tmp` — because a read-only root with the wrong `/tmp` mode is worse than no
read-only root at all (§ Gotchas 1).
`sandbox_container_run_event_is_absent_when_the_sandbox_is_off` pins that no
record claims a container that never existed (M3), and
`sandbox_container_run_event_survives_a_missing_lock` pins `"unknown"` rather
than a panic on the path preflight is supposed to make unreachable.
`sandbox_images_pin_their_base_by_digest` reads both Dockerfiles through
`include_str!`, so the pin is on the real files (M4).

Two seams have **no** mutation pair, deliberately, and the architect measured
each rather than leaving it unstated:

- **Deleting the `/tmp` tmpfs** fails **two** tests —
  `sandbox_exec_run_args_match_the_prototyped_vector` and
  `sandbox_run_args_give_a_read_only_root_two_writable_tmpfs`. Covered by the
  `--tmpfs` count criterion above.
- **Rendering a missing lock as `""` instead of `"unknown"`** fails exactly
  `sandbox_container_run_event_survives_a_missing_lock`. Covered by that test
  running green.

`run_args` is pure and fully unit-tested; nothing in this phase spawns docker
during tests. The flag set's real-world effect was measured by the architect
against live containers (§ Live measurements) and is re-verified at milestone
close.

## End-to-end verification

Run this block verbatim from the repo root, **after** Tasks 6–9 have appended
their mutation markers to `/tmp/e2e-11.txt` and all four pairs are restored.

```sh
{
echo "== A. named tests =="
cargo test --lib sandbox_run_args 2>&1 | grep -E "^test |^test result:" | sed 's/; finished in .*//'; echo "cargo_exit=${PIPESTATUS[0]}"
cargo test --lib sandbox_container_run_event 2>&1 | grep -E "^test |^test result:" | sed 's/; finished in .*//'; echo "cargo_exit=${PIPESTATUS[0]}"
cargo test --lib sandbox_images_pin 2>&1 | grep -E "^test |^test result:" | sed 's/; finished in .*//'; echo "cargo_exit=${PIPESTATUS[0]}"
echo "== B. full suite, all targets =="
cargo test 2>&1 | grep -E "^test result:" | sed 's/; finished in .*//'; echo "cargo_exit=${PIPESTATUS[0]}"
echo "== C. gates =="
cargo fmt --all -- --check > /dev/null 2>&1; echo "fmt_exit=$?"
cargo clippy --all-targets --all-features -- -D warnings > /dev/null 2>&1; echo "clippy_exit=$?"
echo "== D. structural greps =="
C=src/daemon/executor/container.rs
R=src/daemon/background/run.rs
echo -n "memory-swap flag (1):           "; grep -cF '"--memory-swap".to_string(),' "$C"
echo -n "read-only flag (1):             "; grep -cF '"--read-only".to_string(),' "$C"
echo -n "no-new-privileges (1):          "; grep -cF '"no-new-privileges".to_string(),' "$C"
echo -n "pull never (1):                 "; grep -cF '"never".to_string(),' "$C"
echo -n "cap-drop ALL (1):               "; grep -cF '"ALL".to_string(),' "$C"
echo -n "mode=1777 (3):                  "; grep -cF 'mode=1777' "$C"
echo -n "tmpfs in run_args (2):          "; sed -n '/^pub fn run_args/,/^}/p' "$C" | grep -c '"--tmpfs"'
echo -n "M1 seam (2):                    "; grep -cF 'cfg.limits.memory.clone(),' "$C"
echo -n "container_run_event fn (1):     "; grep -c 'pub fn container_run_event(' "$C"
echo -n "run.rs container_run (2):       "; grep -c 'container_run' "$R"
echo -n "agent digest (1):               "; grep -c '^FROM alpine@sha256:' containers/Dockerfile
echo -n "proxy digest (1):               "; grep -c '^FROM alpine@sha256:' containers/proxy/Dockerfile
echo -n "agent tag (0):                  "; grep -c '^FROM alpine:' containers/Dockerfile
echo -n "proxy tag (0):                  "; grep -c '^FROM alpine:' containers/proxy/Dockerfile
echo -n "allow total (6):                "; grep -rc "allow(dead_code)" src/ | awk -F: '{s+=$2} END {print s}'
echo -n "prod unwrap container.rs (0):   "; sed -n '1,/^#\[cfg(test)\]/p' "$C" | grep -c '\.unwrap()\|\.expect('
echo -n "code files changed (4):         "; git diff --name-only | grep -cE '^(src|containers|assets)/'
} >> /tmp/e2e-11.txt 2>&1
cat /tmp/e2e-11.txt
```

Paste the whole of `/tmp/e2e-11.txt` — mutation markers included — into your
Update Log entry as a fenced block, then run the self-check and paste its
verdict line into the same entry **bare, on its own line, with no backticks**:

```sh
D=docs/dev/milestones/M19-sandbox-completion/phase-11-container-hardening-flags.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-11.txt
diff /tmp/pasted-11.txt /tmp/e2e-11.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

**Run the block exactly as written.** If a label in it has gone stale against
the criteria, that is a spec defect — record a blocker naming it rather than
editing the block.

## Authorizations

- Edit **only** these four files: `src/daemon/executor/container.rs`,
  `src/daemon/background/run.rs`, `containers/Dockerfile` and
  `containers/proxy/Dockerfile` — plus this phase doc's Update Log. No other
  file, no other doc.
- Run `cargo fmt --all`, `cargo build`,
  `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`.
- No `#[allow(...)]` may be added or removed, and no `#[ignore]` may be added
  or removed.
- **The pinned vector in `sandbox_exec_run_args_match_the_prototyped_vector`
  is the one existing test you may edit**, to exactly the value Task 1 gives.
  Change no other existing test's assertions.
- **Do not run `docker`, `podman`, or any container command** — including
  `daemoneye sandbox build` — and do not start, stop or query a system
  service. Every runtime behaviour this phase depends on was measured by the
  architect (§ Live measurements) and is re-verified at milestone close. In
  particular **do not try to re-derive the alpine digest**; Task 4 gives it.
- Mutation edits go through `patch`. **Never `git checkout` a file to restore
  it** — it discards this round's own uncommitted work (§ Gotchas 5).
- **Append to the Update Log; never edit or delete an existing entry.** When
  flipping this doc's `Status:` line, change **only** that line — the line
  above it is `**Milestone:** M19 — Sandbox Completion` and must survive (a
  mis-anchored status patch ate it in phase-03; see `bugs/bug-phase-03-1.md`).
  After the flip, `grep -c '^\*\*Status:\*\*' <this doc>` must print `1` and
  `grep -c '^\*\*Milestone:\*\*' <this doc>` must print `1`.
- **When you insert a function or a block next to an existing one, check what
  sits immediately above your insertion point.** A `///` doc comment attaches
  to whatever item follows it, so inserting an item mid-comment silently
  reassigns the whole block to your new code and leaves the original
  undocumented. That is `bugs/bug-phase-13-1.md`, filed one phase ago. Task 2's
  anchor is a doc-comment line for exactly this reason: insert **before** it,
  so the comment stays with the function it describes.
- **Never edit `/tmp/e2e-11.txt` or the pasted evidence block after capture,
  for any reason** (Task 10). On a `PASTE MISMATCH`, delete the artifact and
  re-run the sequence; if a mismatch survives a clean re-run, record a
  blocker. This is `bugs/bug-phase-04-1.md`.
- **If you cannot finish honestly — an acceptance criterion is unsatisfiable, a
  mutation leaves the suite green or fails a different number of tests than the
  spec states, *or* a gate is red for a reason this phase did not cause —
  record a blocker Update Log entry naming the exact criterion, and stop.
  Reporting the blocker *is* the successful outcome.**
- **Record what you decide, not what you wish had been decided.** Every claim
  in your completion summary must be one the reviewer can re-run as a command
  from this doc. **Do not describe a criterion as met without reading its
  output, and if a pasted number disagrees with the value the criterion
  states, say so in your summary rather than reporting overall conformance.**

## Out of scope

- **The >90-day image staleness warning** in `retention_warnings()` and the
  **`requires_tools` runbook frontmatter** with its fail-fast check. Both are
  named in the README's 11 intent, item 7; neither is a `run_args` flag, and
  `requires_tools` is a runbook-parsing feature with its own design. They need
  a phase of their own — the milestone README records this.
- **`--userns`, seccomp profiles, AppArmor, and `--runtime=runsc` (gVisor).**
  gVisor is a phase-10 *measurement*, not a phase-11 flag; decide nothing about
  it here.
- **Any change to what the images contain.** Task 4 pins the base by digest and
  changes nothing else — no package added, no package removed. A rebuild is
  **not** required by this phase and must not be attempted (§ Live
  measurements 5).
- **Verifying the proxy image against `proxy.lock` at run time.** `start_proxy`
  does not consult it today; that is a real gap, recorded in the milestone
  README, and closing it is a behaviour change with its own design question
  (what a *missing* lock should do to an existing install).
- **`respawn.rs` / foreground / remote execution** — unchanged, as in every
  sandbox phase.
- **The `container_run` record's consumers.** Writing it is this phase;
  reading it in a live check is phase-10.
- `CLAUDE.md`, `README.md`, the design doc — the phase-10 doc sweep.

## Update Log
