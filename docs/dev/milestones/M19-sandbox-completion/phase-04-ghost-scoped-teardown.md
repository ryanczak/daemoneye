# Phase 04: Reclaim one ghost's containers on exit, and nobody else's

**Milestone:** M19 — Sandbox Completion
**Status:** todo
**Depends on:** phase-01 (`resolve_is_ghost`), phase-03 (`respawn.rs` is a sandbox call site)
**Estimated diff:** ~260 lines including tests
**Tags:** language=rust, kind=feature, size=m

## Goal

A ghost shell's sandboxed containers carry `de.sandbox=1` and `de.ghost=1`, and
that is **all** they carry. Nothing can ask the runtime "which containers
belong to *this* ghost", so when a ghost exits its containers wait for the next
daemon start's `sweep_sandbox_leftovers` — which removes **every** sandbox
container, including a live interactive session's.

This phase gives each sandboxed container a `de.session=<session_id>` label and
adds a ghost-scoped teardown on the ghost's exit path that removes exactly the
containers that ghost created. It also wires `[sandbox.ghost_defaults]
destroy_on_exit`, which `SandboxConfig` has parsed since M18 and which no
execution path has ever read.

**The negative case is the phase.** A teardown that reclaims a *sibling*
ghost's containers, or an interactive session's, is worse than no teardown — so
the selector is pinned by tests asserting what it must **not** match, and by a
mutation that removes the session filter.

## Architecture references

- `docs/design/agent-container-sandboxing.md` § "D4 — Mount policy" and the M18
  sweep design: labels are the selector, and `de.sandbox=1` is the daemon-wide
  one. This phase adds the session-scoped one beneath it.
- `CLAUDE.md` § "Container sandbox" — *"Containers … carry `--label
  de.sandbox=1` (plus `de.ghost=1` for ghost sessions), … and are swept at
  daemon start"*. The start-up sweep is unchanged; this is a second, narrower
  reclamation with a different trigger.
- `docs/dev/milestones/M19-sandbox-completion/README.md` § Notes — the gap
  recorded while drafting phase-03: *"`[sandbox.ghost_defaults]` is parsed and
  consulted by nothing."* This phase closes half of it; `mount_scripts` is
  still unwired and still has no phase.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any file.
4. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

Measured on the tree at drafting time (2026-08-29, commit `3467223`). **The
whole change below was prototyped end-to-end before this doc was written** —
every count in § Acceptance criteria was read off that prototype, not derived.

- `cargo test --lib` → **1471 passed; 0 failed; 4 ignored**. All four gates
  green.
- `grep -c "de.session" src/daemon/executor/container.rs` → **0**. The only
  labels emitted are `de.sandbox=1` (always) and `de.ghost=1` (ghosts), at
  `container.rs:768-773`:

  ```rust
  args.push("--label".to_string());
  args.push("de.sandbox=1".to_string());
  if spec.is_ghost {
      args.push("--label".to_string());
      args.push("de.ghost=1".to_string());
  }
  args.push("--workdir".to_string());
  ```

- `grep -rc "ghost_defaults" src/ | awk -F: '{s+=$2} END {print s}'` → **9**,
  **all** in `src/config/` (the struct and its parse tests).
- `grep -c "fn ghost_teardown" src/daemon/executor/container.rs` → **0**;
  `grep -c "fn should_teardown_ghost" …` → **0**;
  `grep -c "container::" src/daemon/ghost.rs` → **0** — `ghost.rs` does not
  reference the sandbox at all today.
- **`ExecSpec` has 22 literal construction sites**
  (`container.rs:20`, `run.rs:1`, `respawn.rs:1`), 21 of which share the anchor
  `is_ghost: false,` / `is_ghost: true,`. **Adding a field to `ExecSpec` would
  break all 22 with no unique `old_str` for any of them.** The session id is
  therefore a **parameter**, not a field. Do not touch `ExecSpec`.
- `run_args` (`container.rs:737`) and `sandbox_window_command` (`:789`) each
  gain one trailing parameter. Their call sites: **23 inside `mod tests`**,
  plus one production site each.
- `run_args`'s exact output is pinned by
  `sandbox_exec_run_args_match_the_prototyped_vector` (`container.rs:1104`).
  With `None` the vector is byte-identical, so **that test passes unchanged** —
  verified on the prototype.
- The ghost exit point is `trigger_ghost_turn` (`ghost.rs:287-298`), which runs
  the whole session (`do_ghost_turn` holds the turn loop) and is called once
  per ghost from `webhook/process.rs:469`, `scheduled.rs:116`,
  `stream.rs:1203`, and recursively at `ghost.rs:960` for a nested spawn:

  ```rust
  pub async fn trigger_ghost_turn(
      session_id: &str,
      sessions: &SessionStore,
      config: &Config,
      cache: &Arc<SessionCache>,
      schedule_store: &Arc<ScheduleStore>,
  ) -> Result<()> {
      let result = do_ghost_turn(session_id, sessions, config, cache, schedule_store).await;
      write_mailbox_on_exit(session_id, sessions, result.as_ref().err()).await;
      result
  }
  ```

  `write_mailbox_on_exit` is the established "on exit, clean or failed" hook —
  the teardown goes directly beside it, and `config.sandbox` is already in hand.
- Ghost session ids are `format!("ghost-{alert_name}-{uuid}")` (`ghost.rs:185`),
  so `alert_name` — from the webhook payload — is **inside the label value**.
  § Live measurement 3 is why that is safe and why this phase adds no validator.
- The sweep builder to copy is `sweep_container_list_args` (`container.rs:591`);
  `sweep_container_rm_args` (`:603`) is reused as-is:

  ```rust
  pub fn sweep_container_list_args(cfg: &SandboxConfig) -> Vec<String> {
      vec![
          "--host".to_string(),
          cfg.docker_host.clone(),
          "ps".to_string(),
          "-aq".to_string(),
          "--filter".to_string(),
          "label=de.sandbox=1".to_string(),
      ]
  }
  ```

- `Command`, `Duration` and `bounded_output_with` are imported at
  `container.rs:1-4`.

### Live measurements (architect, rootless Docker on the daemon host)

Three throwaway containers: **A** `de.ghost=1` + `de.session=ghost-aaa`,
**B** `de.ghost=1` + `de.session=ghost-aaa-extra`, **C** `de.sandbox=1` only
(an interactive session). All removed afterwards.

1. **`label=k=v` is an exact match, not a prefix or substring.**

   ```
   $ docker ps -a --filter label=de.session=ghost-aaa --format '{{.Names}}'
   de-probe-A
   ```

   **B did not match**, though its value has `ghost-aaa` as a prefix. This is
   the opposite of docker's `name=` filter, which *is* a substring match — the
   reason `stale_stage_volumes` exists rather than a `--filter name=de-stage-`.
   Do not add a prefix guard; the runtime already gives exactness.
2. **Repeated `--filter label=` clauses are ANDed**, and `rm -f` removes a
   **running** container:

   ```
   $ docker ps -a --filter label=de.ghost=1 --filter label=de.session=ghost-aaa --format '{{.Names}}'
   de-probe-A
   $ docker rm -f e23ff226d122   # A was running `sleep 60`
   e23ff226d122
   rm_exit=0
   $ docker ps -a --filter label=de.sandbox=1 --format '{{.Names}} {{.State}}'
   de-probe-C running
   de-probe-B running
   ```

   The sibling ghost and the interactive session survived. That is what this
   phase must preserve and what the tests pin.
3. **A label value containing `=` or a space round-trips**, because docker
   splits `--label k=v` on the *first* `=` only:

   ```
   $ docker run --label 'de.session=ghost-a=b-1' … ; docker inspect -f '{{json .Config.Labels}}' …
   {"de.ghost":"1","de.sandbox":"1","de.session":"ghost-a=b-1"}
   $ docker ps -a --filter 'label=de.session=ghost-a=b-1' --format '{{.Names}}'
   de-probe-E
   $ docker ps -a --filter 'label=de.session=ghost-a'      --format '{{.Names}}'
   (nothing)
   ```

   An alert named `disk full` also round-tripped and matched. **I expected to
   need a label-safety validator and measured that I do not** — every argument
   goes through `Command::args`, never a shell. A sanitizer here could only
   make a legitimate session unmatchable.
4. A list filter that matches nothing prints nothing and **exits 0**; an empty
   result is not an error to report.
5. Volumes auto-created by `-v name:/path` carry **no labels**, so volumes
   cannot be reclaimed by session — and need not be: phase-02 and phase-03
   remove `de-stage-<job_id>` at each job's completion.

## Gotchas

1. **`cargo build` does NOT catch the call sites this phase breaks — measured.**
   After both signatures changed and **all 23** test call sites were still
   two-argument, `cargo build` printed
   `Finished dev profile … build_errors=0`. `cargo build` compiles the lib
   only; the test call sites live behind `#[cfg(test)]`. Use
   **`cargo clippy --all-targets --all-features`** (or `cargo test`) to see
   them — it reports one `error[E0061]` per site with its line. Iterate that
   command until it is clean; do not conclude from a green `cargo build` that
   the change is complete.

2. **Do not add a field to `ExecSpec`.** 22 literals, 21 sharing an anchor
   (§ Current state). The session id is a parameter. If you find yourself
   editing an `ExecSpec` literal, stop — that is the sign you took the wrong
   route.

3. **`run_args`'s pinned vector test must pass untouched.** With `None` the
   argv is byte-identical to today's. If
   `sandbox_exec_run_args_match_the_prototyped_vector` fails, the label is
   being emitted unconditionally — a defect in the new code, not a test to
   update.

4. **The session label must be pushed *before* the image argument.** Docker
   reads options up to the image name; a `--label` after `cfg.image` would be
   handed to the container's `sh`. Push it in the same block as `de.ghost=1`,
   before the `--workdir` push.

5. **Teardown is best-effort and must never fail a ghost.** It runs on the exit
   path of every ghost, including a failed one. Log and continue; never
   propagate an error into `trigger_ghost_turn`'s `Result`. Use
   `bounded_output_with` exactly as `sweep_sandbox_leftovers` does.

6. **Do not call it from `do_ghost_turn`.** That is the turn loop; a teardown
   there would fire mid-session and destroy containers a later turn still
   needs. The hook goes in `trigger_ghost_turn`, next to
   `write_mailbox_on_exit`.

7. **Do not touch `sweep_sandbox_leftovers` or the start-up sweep.** They stay
   daemon-wide.

8. **The docker call is blocking; `trigger_ghost_turn` is async.** Wrap it in
   `tokio::task::spawn_blocking(...).await` and discard the `JoinError` with
   `let _ =`. Do **not** route it through `tmux::off_runtime` — its 5 s bound
   is sized for tmux.

9. **Name the new tests `sandbox_session_label_*`, not `session_label_*` —
   measured.** `cargo test --lib session_label` also matches the pre-existing
   `approval_panel_sudo_session_label` in `cli::render_ratatui`, which would
   make the "4 ok" criterion read 5.

## Spec

### Task 1 — `run_args` takes the owning session, in `src/daemon/executor/container.rs`

Change the signature and doc comment (`container.rs:735-737`):

```rust
/// argv for the sandboxed run. Pure — the caller prepends the runtime binary
/// and spawns it.
///
/// `session_id` becomes a `de.session=<id>` label, which is what lets a
/// ghost's own containers be reclaimed on exit without touching a sibling
/// ghost's or an interactive session's. `None` emits no such label.
pub fn run_args(
    cfg: &SandboxConfig,
    spec: &ExecSpec,
    session_id: Option<&str>,
) -> Vec<String> {
```

and add the emission inside the existing label block, after the `de.ghost=1`
push and before the `--workdir` push:

```rust
    if let Some(sid) = session_id {
        args.push("--label".to_string());
        args.push(format!("de.session={sid}"));
    }
```

Nothing else in the body changes.

### Task 2 — `sandbox_window_command` takes it too, same file

Change the signature (`container.rs:789`), keeping the doc comment above it:

```rust
pub fn sandbox_window_command(
    cfg: &SandboxConfig,
    spec: &ExecSpec,
    raw_cmd: &str,
    session_id: Option<&str>,
) -> String {
```

and its one internal call:

```rust
    let run = run_args(cfg, spec, session_id);
```

### Task 3 — The two production call sites pass their session id

**`src/daemon/background/run.rs:223`** — `session_id` is the function's own
`Option<String>` parameter:

```rust
            sandboxed_cmd = crate::daemon::executor::container::sandbox_window_command(
                &config.sandbox,
                &spec,
                cmd,
                session_id.as_deref(),
            );
```

**`src/daemon/background/respawn.rs:86`** — the identical change; `session_id`
there is also an `Option<String>` parameter.

### Task 4 — Append `None` at every test call site, same file

Every `run_args(` and `sandbox_window_command(` call inside
`container.rs`'s `mod tests` gains a trailing `None` argument. There are
**23** of them. Two shapes occur:

```rust
        let args = run_args(&cfg, &spec);              // becomes: run_args(&cfg, &spec, None)
```

```rust
        let plain = run_args(
            &cfg,
            &ExecSpec { … },
        );
        // becomes the same call with `None,` as the last line before the `)`
```

**Do not change any assertion, any expected vector, or any `ExecSpec`
literal** — only the argument list. Run
`cargo clippy --all-targets --all-features`; it names every remaining site
with `error[E0061]` and a line number. Iterate until it is clean (§ Gotchas 1
— `cargo build` will *not* show these).

### Task 5 — The teardown, same file

Insert directly after `stale_stage_volumes` (`container.rs:638-647`) and
before the `/// Remove orphaned sandbox containers …` doc comment of
`sweep_sandbox_leftovers`:

```rust
/// Whether a ghost's containers should be destroyed when it exits: the
/// sandbox must be on, and `[sandbox.ghost_defaults] destroy_on_exit` must
/// not have been turned off.
pub fn should_teardown_ghost(cfg: &SandboxConfig) -> bool {
    cfg.enabled && cfg.ghost_defaults.destroy_on_exit
}

/// argv listing the containers one ghost session owns, running or not.
///
/// All three filters are load-bearing and docker ANDs them: `de.sandbox=1`
/// keeps it to this daemon's containers, `de.ghost=1` makes an interactive
/// session's container unmatchable, and `de.session=<id>` is an **exact**
/// value match, so a sibling ghost whose id merely shares a prefix is not
/// selected.
pub fn ghost_teardown_list_args(cfg: &SandboxConfig, session_id: &str) -> Vec<String> {
    vec![
        "--host".to_string(),
        cfg.docker_host.clone(),
        "ps".to_string(),
        "-aq".to_string(),
        "--filter".to_string(),
        "label=de.sandbox=1".to_string(),
        "--filter".to_string(),
        "label=de.ghost=1".to_string(),
        "--filter".to_string(),
        format!("label=de.session={session_id}"),
    ]
}

/// Remove every container belonging to one ghost session. Blocking — call it
/// off the async runtime. Best-effort: every failure is logged and none is
/// propagated, because this runs on a ghost's exit path.
pub fn teardown_ghost_containers(cfg: &SandboxConfig, session_id: &str) {
    if !should_teardown_ghost(cfg) {
        return;
    }
    let mut cmd = Command::new(&cfg.runtime);
    cmd.args(ghost_teardown_list_args(cfg, session_id));
    let listed = match bounded_output_with(&mut cmd, Duration::from_secs(30)) {
        Ok(out) => out,
        Err(e) => {
            log::warn!("ghost container teardown list failed for {session_id}: {e}");
            return;
        }
    };
    let ids: Vec<String> = String::from_utf8_lossy(&listed.stdout)
        .lines()
        .map(|line| line.trim())
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect();
    if ids.is_empty() {
        return;
    }
    let count = ids.len();
    let mut cmd = Command::new(&cfg.runtime);
    cmd.args(sweep_container_rm_args(cfg, &ids));
    match bounded_output_with(&mut cmd, Duration::from_secs(30)) {
        Ok(_) => log::info!("ghost teardown removed {count} container(s) for {session_id}"),
        Err(e) => log::warn!("ghost container teardown remove failed for {session_id}: {e}"),
    }
}
```

### Task 6 — Call it when a ghost exits, in `src/daemon/ghost.rs`

In `trigger_ghost_turn`, between the `write_mailbox_on_exit` line and `result`:

```rust
    let (sbx, sid) = (config.sandbox.clone(), session_id.to_string());
    let _ = tokio::task::spawn_blocking(move || {
        crate::daemon::executor::container::teardown_ghost_containers(&sbx, &sid)
    })
    .await;
    result
```

The `let _ =` is deliberate: a `JoinError` must not change the ghost's own
outcome (§ Gotchas 5). With the sandbox disabled — or
`destroy_on_exit = false` — `teardown_ghost_containers` returns before
spawning anything, so this is a no-op and today's behaviour is unchanged.

### Task 7 — Tests in `container.rs`'s existing `mod tests`

Seven tests, named exactly as below, appended at the end of the module. They
are the prototype's tests and pass on the prototype tree.

```rust
#[test]
fn sandbox_session_label_is_absent_without_a_session() {
    let cfg = SandboxConfig::default();
    let spec = ExecSpec {
        job_id: "j1",
        network: "none",
        is_ghost: true,
        command: "echo hi",
    };
    assert!(
        !run_args(&cfg, &spec, None)
            .iter()
            .any(|a| a.starts_with("de.session=")),
        "no session label without a session"
    );
}

#[test]
fn sandbox_session_label_rides_beside_the_ghost_label() {
    let cfg = SandboxConfig::default();
    let spec = ExecSpec {
        job_id: "j1",
        network: "none",
        is_ghost: true,
        command: "echo hi",
    };
    let args = run_args(&cfg, &spec, Some("ghost-aaa"));
    assert!(args.iter().any(|a| a == "de.ghost=1"), "{args:?}");
    assert!(args.iter().any(|a| a == "de.session=ghost-aaa"), "{args:?}");
    let label = args
        .iter()
        .position(|a| a == "de.session=ghost-aaa")
        .expect("label");
    let image = args
        .iter()
        .position(|a| a == "daemoneye-agent-base")
        .expect("image");
    assert!(
        label < image,
        "the label must precede the image or docker hands it to the container: {args:?}"
    );
}

#[test]
fn sandbox_session_label_keeps_a_value_containing_an_equals_sign() {
    // Ghost ids embed the alert name (`ghost-<alert>-<uuid>`), and docker
    // splits `--label k=v` on the first `=` only — measured, not assumed.
    let cfg = SandboxConfig::default();
    let spec = ExecSpec {
        job_id: "j1",
        network: "none",
        is_ghost: true,
        command: "echo hi",
    };
    let args = run_args(&cfg, &spec, Some("ghost-a=b-1"));
    assert!(
        args.iter().any(|a| a == "de.session=ghost-a=b-1"),
        "{args:?}"
    );
}

#[test]
fn sandbox_session_label_reaches_the_window_command() {
    let cfg = SandboxConfig {
        enabled: true,
        ..Default::default()
    };
    let spec = ExecSpec {
        job_id: "j1",
        network: "none",
        is_ghost: true,
        command: "echo hi",
    };
    let line = sandbox_window_command(&cfg, &spec, "echo hi", Some("ghost-aaa"));
    assert!(line.contains("de.session=ghost-aaa"), "{line}");
    assert!(
        !sandbox_window_command(&cfg, &spec, "echo hi", None).contains("de.session"),
        "no session means no label in the window command either"
    );
}

#[test]
fn ghost_teardown_selects_one_session_and_not_its_neighbours() {
    let cfg = SandboxConfig::default();
    let args = ghost_teardown_list_args(&cfg, "ghost-aaa");
    assert!(
        args.iter().any(|a| a == "label=de.session=ghost-aaa"),
        "{args:?}"
    );
    assert!(
        !args.iter().any(|a| a == "label=de.session=ghost-aaa-extra"),
        "a sibling ghost must never be named: {args:?}"
    );
    assert!(
        !args.iter().any(|a| a == "label=de.session=ghost-bbb"),
        "another ghost must never be named: {args:?}"
    );
}

#[test]
fn ghost_teardown_is_scoped_to_this_daemons_ghosts() {
    let cfg = SandboxConfig::default();
    let args = ghost_teardown_list_args(&cfg, "ghost-aaa");
    assert!(args.iter().any(|a| a == "label=de.sandbox=1"), "{args:?}");
    assert!(
        args.iter().any(|a| a == "label=de.ghost=1"),
        "without the ghost filter an interactive session's container could match: {args:?}"
    );
    assert_eq!(args.first().map(String::as_str), Some("--host"), "{args:?}");
    assert!(
        args.iter().any(|a| a == "-aq"),
        "stopped containers count too: {args:?}"
    );
}

#[test]
fn ghost_teardown_honours_destroy_on_exit_and_the_sandbox_flag() {
    let on = SandboxConfig {
        enabled: true,
        ..Default::default()
    };
    assert!(should_teardown_ghost(&on), "default destroy_on_exit is true");

    let off = SandboxConfig {
        enabled: false,
        ..Default::default()
    };
    assert!(
        !should_teardown_ghost(&off),
        "sandbox off means nothing to reclaim"
    );

    let no_destroy = SandboxConfig {
        enabled: true,
        ghost_defaults: crate::config::SandboxGhostDefaults {
            destroy_on_exit: false,
            ..Default::default()
        },
        ..Default::default()
    };
    assert!(
        !should_teardown_ghost(&no_destroy),
        "the operator turned it off"
    );
}
```

`crate::config::SandboxGhostDefaults` resolves: the struct is declared at
`src/config/types.rs:472` and `src/config/mod.rs:16` carries
`pub use types::*;`. Verified on the prototype. Do **not** add a re-export.

### Task 8 — Mutation pair M1: the session filter is real

Mutation edits go through your `patch` tool — **`sed -i`, `perl -i` and `>`
redirects into a source file are banned by your contract and `bash` will
refuse them.** Append each marker and run to `/tmp/e2e-04.txt`. Run the gates
(§ End-to-end verification) only **after** both pairs are restored.

1. **Apply.** `patch` `src/daemon/executor/container.rs`:
   - `old_str`: `        format!("label=de.session={session_id}"),`
   - `new_str`: `        "label=de.sandbox=1".to_string(),`

   Then:
   ```sh
   echo "== M1 APPLIED ==" >> /tmp/e2e-04.txt
   cargo test --lib ghost_teardown 2>&1 | grep -E "FAILED|^test result:" >> /tmp/e2e-04.txt
   grep -c 'format!("label=de.session={session_id}")' src/daemon/executor/container.rs >> /tmp/e2e-04.txt
   ```
   Measured on the prototype: **exactly 1 failed**, naming
   `ghost_teardown_selects_one_session_and_not_its_neighbours`, and the
   `grep -c` prints `0`. A mutation that leaves the suite green means the
   selector is unguarded and a ghost teardown could reclaim every ghost's
   containers — record a blocker.

2. **Restore.** The inverse `patch`, then:
   ```sh
   echo "== M1 RESTORED ==" >> /tmp/e2e-04.txt
   cargo test --lib ghost_teardown 2>&1 | grep -E "FAILED|^test result:" >> /tmp/e2e-04.txt
   grep -c 'format!("label=de.session={session_id}")' src/daemon/executor/container.rs >> /tmp/e2e-04.txt
   ```
   Now the 3 tests pass and the `grep -c` prints `1`.

### Task 9 — Mutation pair M2: `destroy_on_exit` is really consulted

Only after M1 is restored.

1. **Apply.** `patch` `src/daemon/executor/container.rs`:
   - `old_str`: `    cfg.enabled && cfg.ghost_defaults.destroy_on_exit`
   - `new_str`: `    cfg.enabled`

   Then:
   ```sh
   echo "== M2 APPLIED ==" >> /tmp/e2e-04.txt
   cargo test --lib ghost_teardown 2>&1 | grep -E "FAILED|^test result:" >> /tmp/e2e-04.txt
   grep -c 'cfg.ghost_defaults.destroy_on_exit' src/daemon/executor/container.rs >> /tmp/e2e-04.txt
   ```
   Measured on the prototype: **exactly 1 failed**, naming
   `ghost_teardown_honours_destroy_on_exit_and_the_sandbox_flag`, and the
   `grep -c` prints `0`. That test is what proves the config key is read
   rather than merely parsed.

2. **Restore.** The inverse `patch`, then:
   ```sh
   echo "== M2 RESTORED ==" >> /tmp/e2e-04.txt
   cargo test --lib ghost_teardown 2>&1 | grep -E "FAILED|^test result:" >> /tmp/e2e-04.txt
   grep -c 'cfg.ghost_defaults.destroy_on_exit' src/daemon/executor/container.rs >> /tmp/e2e-04.txt
   ```
   The `grep -c` prints `1`.

The `grep -c` after **each** direction is not optional: a `patch` whose
`old_str` matches the wrong line fails silently, and a mutation that never
applied certifies a vacuous guard.

**Both failure counts above were measured, not estimated.** If a mutation
fails a different number of tests than stated, do not adjust a test to match —
record a blocker naming the criterion.

### Task 10 — Capture the end-to-end evidence

Run the block in § End-to-end verification **verbatim and unmodified**, then
paste the resulting `/tmp/e2e-04.txt` into a new Update Log entry headed
`### Update — <date> (end-to-end verification)`. The server-authored
`(complete)` entry does not satisfy this. **The entry ends with the
self-check's verdict line, `PASTE MATCH`, bare on its own line after the
fenced block** — a tick in your final summary is not that line.

## Acceptance criteria

**Every count below was read off the architect's prototype of this exact
change, not derived from the spec text.**

- [ ] `grep -c 'fn ghost_teardown_list_args(' src/daemon/executor/container.rs`,
      `grep -c 'fn teardown_ghost_containers(' …` and
      `grep -c 'fn should_teardown_ghost(' …` each print `1` (**before: 0**).
- [ ] `grep -c 'pub fn run_args(' src/daemon/executor/container.rs` prints `1`
      and `grep -c 'pub fn sandbox_window_command(' …` prints `1`
      (**unchanged** — the functions gained a parameter, they were not
      duplicated; there is no `_labeled` or `_for_session` variant).
- [ ] `grep -c 'de.session' src/daemon/executor/container.rs` prints `13`
      (**before: 0**).
- [ ] `grep -c 'ghost_defaults' src/daemon/executor/container.rs` prints `3`
      (**before: 0**) — the predicate, its doc comment and its test. The key
      is now read, not only parsed.
- [ ] `grep -c 'ExecSpec {' src/daemon/executor/container.rs` prints `24` and
      `grep -rc 'ExecSpec {' --include=*.rs src/ | awk -F: '{s+=$2} END {print s}'`
      prints `26` (**before: 20, 22** — the four new tests add four literals;
      **no existing literal was edited**, which is § Gotchas 2).
- [ ] `grep -c 'session_id.as_deref(),' src/daemon/background/run.rs` prints
      `2` and the same grep on `src/daemon/background/respawn.rs` prints `2`
      (**before: 1, 1** — each file already passes
      `resolve_is_ghost(session_id.as_deref(), entry_is_ghost)` at `run.rs:60`
      / `respawn.rs:48`; the new argument is the second occurrence). The
      trailing comma is load-bearing: without it the grep also counts the
      other four bare `session_id.as_deref()` uses in each file and reads `5`.
- [ ] `grep -c 'teardown_ghost_containers' src/daemon/ghost.rs` prints `1`
      (**before: 0**) — called once, from `trigger_ghost_turn` only.
- [ ] `cargo test --lib sandbox_session_label 2>&1 | grep -c "^test .* ok$"`
      prints `4` and
      `cargo test --lib ghost_teardown 2>&1 | grep -c "^test .* ok$"` prints
      `3`. Counts, not exit statuses. (§ Gotchas 9 — the bare token
      `session_label` would read 5.)
- [ ] `cargo test --lib sandbox_exec_run_args_match_the_prototyped_vector 2>&1 | grep -c "^test .* ok$"`
      prints `1` — the pinned vector is untouched (§ Gotchas 3).
- [ ] `cargo test --lib` reports **1478** passing and `0 failed`
      (**before: 1471**), with `4 ignored` unchanged.
- [ ] `grep -rc "allow(dead_code)" src/ | awk -F: '{s+=$2} END {print s}'`
      prints `6` (**unchanged**).
- [ ] `sed -n '1,/^#\[cfg(test)\]/p' src/daemon/executor/container.rs | grep -c '\.unwrap()\|\.expect('`
      prints `0` (**before: 0**) — no new panicking idiom in production code.
- [ ] The § End-to-end entry shows `== M1 APPLIED ==` and `== M2 APPLIED ==`
      each failing **exactly one** named test — the two names given in Tasks 8
      and 9 — both `RESTORED` runs passing, with a `grep -c` line after each
      direction reading `0` then `1`.
- [ ] No new `#[allow(...)]` anywhere, no `unsafe`, no `TODO`.
- [ ] All four gates green: `cargo fmt --all`, `cargo build`,
      `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`.
- [ ] The § End-to-end entry contains the literal line `PASTE MATCH` (bare,
      with no surrounding backticks):
      `grep -c '^PASTE MATCH$' docs/dev/milestones/M19-sandbox-completion/phase-04-ghost-scoped-teardown.md`
      prints `1`.

## Test plan

Seven unit tests in `container.rs`'s existing `mod tests`, given in full in
Task 7. No new test file. **No existing test's assertions change** — Task 4
adds an argument to 23 call sites and nothing else.

**The negative cases are the phase.**
`ghost_teardown_selects_one_session_and_not_its_neighbours` pins that the argv
names this session and neither a prefix-sharing sibling nor another ghost;
`ghost_teardown_is_scoped_to_this_daemons_ghosts` pins the two filters that
keep an interactive session's container unmatchable. Together they encode
§ Live measurements 1–2 — the exactness is docker's, not ours, and these tests
are what stops someone "simplifying" the filter set. M1 proves the session
filter is load-bearing.
`sandbox_session_label_is_absent_without_a_session` pins that a `None` session
changes nothing, which is what keeps every existing pinned vector honest, and
`sandbox_session_label_keeps_a_value_containing_an_equals_sign` pins § Live
measurement 3 so nobody adds a sanitizer that would break a legitimate alert
name.

`teardown_ghost_containers` itself spawns `docker` and is **not** unit-tested,
matching how `sweep_sandbox_leftovers` and `stage_script`'s success arm are
treated. Its two pure decisions are tested: `should_teardown_ghost` (M2 proves
it) and `ghost_teardown_list_args` (M1 proves it). The spawning wrapper is
verified end-to-end by the architect at milestone close.

Behaviour is unchanged with the sandbox disabled. **If an existing test
requires an assertion change to pass, stop and record a blocker.**

## End-to-end verification

Run this block verbatim from the repo root, **after** Tasks 8 and 9 have
appended their mutation markers to `/tmp/e2e-04.txt` and both pairs are
restored.

```sh
{
echo "== A. named tests (expect 4 + 3 ok) =="
cargo test --lib sandbox_session_label 2>&1 | grep -E "^test |^test result:"; echo "cargo_exit=${PIPESTATUS[0]}"
cargo test --lib ghost_teardown 2>&1 | grep -E "^test |^test result:"; echo "cargo_exit=${PIPESTATUS[0]}"
echo "== B. the pinned run_args vector still passes =="
cargo test --lib sandbox_exec_run_args_match_the_prototyped_vector 2>&1 | grep -E "^test |^test result:"; echo "cargo_exit=${PIPESTATUS[0]}"
echo "== C. full lib suite =="
cargo test --lib 2>&1 | grep -E "^test result:"; echo "cargo_exit=${PIPESTATUS[0]}"
echo "== D. gates =="
cargo fmt --all -- --check > /dev/null 2>&1; echo "fmt_exit=$?"
cargo clippy --all-targets --all-features -- -D warnings > /dev/null 2>&1; echo "clippy_exit=$?"
echo "== E. structural greps =="
echo -n "fn ghost_teardown_list_args (1): "; grep -c 'fn ghost_teardown_list_args(' src/daemon/executor/container.rs
echo -n "fn teardown_ghost_containers (1):"; grep -c 'fn teardown_ghost_containers(' src/daemon/executor/container.rs
echo -n "fn should_teardown_ghost (1):    "; grep -c 'fn should_teardown_ghost(' src/daemon/executor/container.rs
echo -n "pub fn run_args (1):             "; grep -c 'pub fn run_args(' src/daemon/executor/container.rs
echo -n "pub fn swc (1):                  "; grep -c 'pub fn sandbox_window_command(' src/daemon/executor/container.rs
echo -n "de.session in container (13):    "; grep -c 'de.session' src/daemon/executor/container.rs
echo -n "ghost_defaults in container (3): "; grep -c 'ghost_defaults' src/daemon/executor/container.rs
echo -n "ExecSpec here (24):              "; grep -c 'ExecSpec {' src/daemon/executor/container.rs
echo -n "ExecSpec total (26):             "; grep -rc 'ExecSpec {' --include=*.rs src/ | awk -F: '{s+=$2} END {print s}'
echo -n "run.rs new arg (2):              "; grep -c 'session_id.as_deref(),' src/daemon/background/run.rs
echo -n "respawn new arg (2):             "; grep -c 'session_id.as_deref(),' src/daemon/background/respawn.rs
echo -n "ghost.rs teardown (1):           "; grep -c 'teardown_ghost_containers' src/daemon/ghost.rs
echo -n "allow total (6):                 "; grep -rc "allow(dead_code)" src/ | awk -F: '{s+=$2} END {print s}'
echo -n "prod unwrap/expect (0):          "; sed -n '1,/^#\[cfg(test)\]/p' src/daemon/executor/container.rs | grep -c '\.unwrap()\|\.expect('
} >> /tmp/e2e-04.txt 2>&1
cat /tmp/e2e-04.txt
```

Paste the whole of `/tmp/e2e-04.txt` — mutation markers included — into your
Update Log entry as a fenced block, then run the self-check and paste its
verdict line into the same entry **bare, on its own line, with no backticks**:

```sh
D=docs/dev/milestones/M19-sandbox-completion/phase-04-ghost-scoped-teardown.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-04.txt
diff /tmp/pasted-04.txt /tmp/e2e-04.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

**Run the block exactly as written.** If a label in it has gone stale against
the criteria, that is a spec defect — record a blocker naming it rather than
editing the block.

## Authorizations

- Edit `src/daemon/executor/container.rs`, `src/daemon/background/run.rs`,
  `src/daemon/background/respawn.rs` and `src/daemon/ghost.rs` only.
- Run `cargo fmt --all`, `cargo build`,
  `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`.
- No `#[allow(...)]` may be added or removed, and no `#[ignore]` may be added
  or removed.
- **Do not change any existing test's assertions, expected vectors, or
  `ExecSpec` literals.** Task 4's argument append is the only edit existing
  tests receive.
- **Do not run `docker`, `podman`, or any container command**, and do not
  start, stop or query a system service. The runtime behaviour this phase
  depends on was measured by the architect (§ Live measurements) and is
  re-verified at milestone close.
- Mutation edits go through `patch`. **Never `git checkout` a file to restore
  it** — it would discard this round's own uncommitted work.
- **Do not edit any other source file, and do not edit any doc other than this
  phase doc's Update Log.**
- **Append to the Update Log; never edit or delete an existing entry.** When
  flipping this doc's `Status:` line, change **only** that line — the line
  above it is `**Milestone:** M19 — Sandbox Completion` and must survive (a
  mis-anchored status patch ate it last phase; see `bugs/bug-phase-03-1.md`).
  After the flip, `grep -c '^\*\*Status:\*\*' <this doc>` must print `1` and
  `grep -c '^\*\*Milestone:\*\*' <this doc>` must print `1`.
- **If you cannot finish honestly — an acceptance criterion is unsatisfiable, a
  mutation leaves the suite green or fails a different number of tests than the
  spec states, *or* a gate is red for a reason this phase did not cause —
  record a blocker Update Log entry naming the exact criterion, and stop.
  Reporting the blocker *is* the successful outcome.** Do not proceed past a
  blocker you have filed.
- **Record what you decide, not what you wish had been decided.** Every claim
  in your completion summary must be one the reviewer can re-run as a command
  from this doc. Do not assert a count you have not just read, and do not
  describe the end-to-end artifact — paste it and let it speak.

## Out of scope

- **The daemon-start sweep.** `sweep_sandbox_leftovers` stays daemon-wide and
  unchanged; this is a second, narrower reclamation with a different trigger.
- **Volume reclamation by session.** Volumes auto-created by `-v name:` carry
  no labels (§ Live measurement 5), and per-job removal already covers them.
- **`ghost_defaults.mount_scripts`** — still parsed and read by nothing; still
  has no phase. Recorded in the milestone README.
- **The ghost's tmux windows.** `gc_bg_windows` owns those; a container removed
  by teardown leaves its `de-gs-*` window to the existing GC.
- **`profile.network` / `proxy_allow`** — phases 06–08. `ExecSpec.network`
  stays the literal `"none"`.
- **Interactive-session teardown.** Interactive containers now carry
  `de.session` too, but nothing reclaims them per-session: their `--rm`
  containers vanish on exit and the daemon-start sweep catches the rest. A
  status surface for them is phase-05's.
- `CLAUDE.md`, `README.md`, the design doc — the phase-10 doc sweep. The
  `CLAUDE.md` sentence listing the labels becomes incomplete with this phase
  and should be amended there.

## Update Log

<!-- entries appended below this line -->
