# Phase 05: `Request::ContainerStatus` and a SANDBOX section in `daemoneye status`

**Milestone:** M19 — Sandbox Completion
**Status:** done
**Depends on:** phase-04 (`de.session` label) — independent of 01–03
**Estimated diff:** ~330 lines including tests
**Tags:** language=rust, kind=feature, size=m

## Goal

The sandbox is invisible to the operator. Nothing reports whether the runtime
is reachable, whether the live image still matches `sandbox.lock`, or which
containers this daemon currently owns — `daemoneye status` has fourteen
sections and none of them mentions the sandbox.

This phase adds `Request::ContainerStatus` → `Response::ContainerStatus`, the
collector behind it, and a `SANDBOX` section in `daemoneye status`. It is the
milestone's observability exit criterion, and it is what makes phases 03, 04
and 06–08 checkable by a human rather than only by tests.

**The parse is the phase.** A container's `de.session` label carries a
webhook-supplied alert name (`ghost-{alert_name}-{uuid}`, `ghost.rs:185`), so
the listing format has to survive spaces, commas, `=` and newlines inside a
label value. § Live measurements shows the obvious approach silently
mis-attributing a container, and the one that does not.

## Architecture references

- `docs/design/agent-container-sandboxing.md` § "IPC surface" — the design of
  record, quoted verbatim:

  > `Request::ContainerStatus` → `Response::ContainerStatus { runtime_ok,
  > containers: Vec<ContainerInfo> }` — per-session/per-ghost container state
  > for `daemoneye status` and the `/panes`-style inspector.

  This phase adds two fields the design did not name — `enabled` and
  `image_detail` — because the milestone's exit criterion asks for *"runtime
  reachable, image id vs lockfile, live sandboxed containers"* and the lockfile
  comparison has nowhere else to go.
- `docs/design/agent-container-sandboxing.md` § "Executor backend" —
  `container:info` is *"runtime health for observability (`daemoneye status`)"*.
  That is this.
- `CLAUDE.md` § "Key files" — `src/ipc.rs` is *"`Request` / `Response` enums —
  the full wire protocol"*, and `src/cli/` holds terminal rendering.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any file.
4. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

Measured on the tree at drafting time (2026-08-30, commit `1a058d8`). **The
whole change was prototyped end-to-end before this doc was written** — every
count in § Acceptance criteria was read off that prototype.

- `cargo test --lib` → **1478 passed; 0 failed; 4 ignored**. All four gates
  green.
- `grep -c 'ContainerStatus' src/ipc.rs` → **0**;
  `grep -c 'ContainerStatus' src/cli/status.rs` → **0**;
  `grep -c 'fn parse_container_records(' src/daemon/executor/container.rs` → **0**.
- **Adding a `Response` variant breaks two exhaustive matches — measured, and
  the compiler is the only thing that tells you.** With the variant added and
  nothing else touched, `cargo build` reports:

  ```
  error[E0004]: non-exhaustive patterns: `ipc::Response::ContainerStatus { .. }` not covered
     --> src/cli/commands/ask.rs:103:15
  error[E0004]: non-exhaustive patterns: `ipc::Response::ContainerStatus { .. }` not covered
     --> src/cli/commands/stream.rs:353:15
  ```

  Both are the streaming clients' "responses this loop ignores" arm, a `|`
  chain ending in `=> {}`. In **each** file the fix is one line added to that
  chain — `grep -n '| Response::PaneList { .. }'` finds the anchor, unique in
  both (`ask.rs:216`, `stream.rs:751`):

  ```rust
              | Response::PaneList { .. }
              | Response::ContainerStatus { .. }
  ```

- The building blocks already exist in `src/daemon/executor/container.rs` and
  **none of them changes**: `probe_runtime` (`:138`), `read_lock` (`:236`),
  `check_image_matches` (`:254`) with its `ImageCheck` enum (`:272`),
  `probe_live_image_id` (`:395`, private — same module, so callable),
  `describe_unavailable` (`:429`), `sweep_container_list_args` (`:591`, which
  is exactly `ps -aq --filter label=de.sandbox=1` and is **reused as the
  status listing**), and `lock_path` (`:230`).
- `Command`, `Duration` and `bounded_output_with` are imported at
  `container.rs:1-4`. `serde_json` is a crate dependency, used across the
  daemon; `container.rs` does not import it today — use the full path
  `serde_json::from_str`, adding no `use`.
- The server dispatch is a flat `match` in `src/daemon/server/mod.rs:205-207`;
  handlers live in `src/daemon/server/handlers.rs` and are `pub(super) async
  fn` with a `W: tokio::io::AsyncWriteExt + Unpin` bound. `handlers::*` is
  glob-imported at `mod.rs:12`, so a new handler needs **no** import edit.
  The existing status arm:

  ```rust
          Request::Status => {
              handle_status(&mut tx, &sessions, &schedule_store, &config).await?;
          }
  ```

- `src/cli/status.rs` builds its output with a `Section` helper (`:76-100`):
  `Section::new("TITLE")`, then `.kv(key, value)` / `.cont(text)` rows, then
  `.render(tw)`. Colour helpers are `c_ok` / `c_err` / `c_warn` / `c_key` /
  `c_val` / `c_num` / `c_dim` (`:19-61`). The last section rendered is
  `COST TODAY` (`:574-599`), inside the `Ok(Response::DaemonStatus { .. })`
  arm; `tx` and `rx` are still in scope there, so a **second round-trip** on
  the same connection is the natural place for this.
- `src/ipc_tests.rs` holds the wire round-trips: `roundtrip_req` /
  `roundtrip_resp` (`:3-11`) serialize and deserialize through `serde_json`.
  `Request::Status`'s round-trip is at `:659`.
- `PaneInfo` (`src/ipc.rs:6`) is the precedent for a named payload struct in
  `ipc.rs` consumed by the daemon: *"A named struct rather than a widening
  tuple"*.

### Live measurements (architect, rootless Docker on the daemon host)

Throwaway containers with `de.sandbox=1`, some `de.ghost=1`, various
`de.session` values. All removed afterwards.

1. **`docker ps`'s own label field cannot be parsed back.** `{{.Labels}}`
   joins pairs with `,`, so a comma inside a value is indistinguishable from
   the separator:

   ```
   $ docker run -d --label de.sandbox=1 --label de.ghost=1 \
       --label 'de.session=ghost-disk,full=x-abc' alpine:3.22 sleep 60
   $ docker ps -a --format '{{json .}}' | jq -r .Labels
   de.ghost=1,de.sandbox=1,de.session=ghost-disk,full=x-abc
   ```

   A splitter would report the session as `ghost-disk` and invent a label
   `full=x-abc`. **Do not parse `{{.Labels}}`.**
2. **`docker inspect --format '… {{json .Config.Labels}}'` is unambiguous** —
   the labels come back as a JSON object, so `,`, `=` and whitespace inside a
   value are the decoder's problem, not ours:

   ```
   $ docker inspect --format '{{.Id}} {{.State.Status}} {{.Config.Image}} {{json .Config.Labels}}' $IDS
   39c2a88ad413…0f running alpine:3.22 {"de.sandbox":"1","de.session":"sess-plain"}
   a1997c9929c4…5d running alpine:3.22 {"de.ghost":"1","de.sandbox":"1","de.session":"ghost-a\nb,c=d"}
   ```

   A **newline** in the value comes back escaped as `\n` *inside* the JSON
   string, so the record stays on one line — verified with `cat -A`: the only
   line terminators are at the true record ends. A tab-separated format does
   **not** survive this: the same container printed with `\t` separators split
   across two output lines.
3. **`docker inspect` with an empty id list is a usage error, not an empty
   result** — and the empty list is the common case (no sandboxed containers
   running):

   ```
   $ docker inspect --format '{{.Id}}'
   docker: 'docker inspect' requires at least 1 argument
   exit=1
   ```

   `status_inspect_args` therefore returns an empty vector for an empty id
   list and the caller skips the spawn — the same shape
   `sweep_container_rm_args` (`container.rs:603`) already uses.
4. `docker ps -aq --filter …` matching nothing prints nothing and **exits 0**.
5. `{{.Id}}` from `inspect` is the **64-character** id; `docker ps -q` shows
   the 12-character prefix. The record is truncated to 12 for display.

## Gotchas

1. **The two exhaustive matches break the build, not the tests** (§ Current
   state). Fix both `| Response::PaneList { .. }` sites before assuming
   anything else is wrong.

2. **Never parse `{{.Labels}}`** (§ Live measurement 1). The format constant
   uses `{{json .Config.Labels}}` and the parser JSON-decodes it. A test pins
   a session id containing a space, a comma and an `=`.

3. **`splitn(4, ' ')`, not `split(' ')`.** The fourth field is the whole
   remaining text — the JSON object, which contains spaces whenever a label
   value does. Mutation M2 pins this.

4. **The collector must never fail.** It runs on an operator command; an
   unreachable runtime is a *reported* state, not an error. Every error arm
   logs and degrades to an empty list. There is no `Result` in its signature.

5. **Do not put this on `Request::Status`.** `DaemonStatus` is already flagged
   `#[allow(clippy::large_enum_variant)]` (`ipc.rs:372`), and collecting this
   spawns the runtime twice under a 30 s bound — `daemoneye status` must not
   block on docker to print its uptime. A second round-trip on the same
   connection is the design of record and what the prototype does.

6. **Spawn the collector off the async runtime.** `tokio::task::spawn_blocking`
   in the handler, as `run.rs` and `ghost.rs` already do for container work. A
   `JoinError` degrades to a reported failure, never a propagated one.

7. **`probe_live_image_id` is private but in the same module** — call it
   directly from `collect_container_status`. Do not make it `pub`.

## Spec

### Task 1 — Wire types in `src/ipc.rs`

Insert both structs immediately **above** the `/// A snapshot of a single tmux
pane (M12 D7).` doc comment of `PaneInfo` (`ipc.rs:3`):

```rust
/// One sandboxed container this daemon owns (M19 phase-05).
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct ContainerInfo {
    /// Short (12-char) container id, as docker displays it.
    pub id: String,
    /// `running`, `exited`, `created`, …
    pub state: String,
    pub image: String,
    /// Owning session, from the `de.session` label. `None` for a container
    /// started before the label existed.
    pub session: Option<String>,
    /// Carries `de.ghost=1`.
    pub is_ghost: bool,
}

/// Sandbox health plus the containers it owns, returned by
/// `Request::ContainerStatus`.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct ContainerStatusReport {
    /// `[sandbox] enabled`. When false the rest is reported, not enforced.
    pub enabled: bool,
    pub runtime_ok: bool,
    /// Version string when reachable; the operator-facing reason when not.
    pub runtime_detail: String,
    /// Live image id vs the lockfile, already rendered for display.
    pub image_detail: String,
    pub containers: Vec<ContainerInfo>,
}
```

Add the request variant directly after `Status,` (`ipc.rs:295`):

```rust
    /// Query sandbox runtime health and the containers this daemon owns.
    /// Separate from `Status` because collecting it spawns the container
    /// runtime, which `Status` must not wait on.
    ContainerStatus,
```

Add the response variant directly after the `PaneList` variant:

```rust
    /// Sandbox status snapshot (response to `Request::ContainerStatus`).
    ContainerStatus { report: ContainerStatusReport },
```

and its name arm directly after the `DaemonStatus` one (`ipc.rs:729`):

```rust
            Response::ContainerStatus { .. } => "ContainerStatus",
```

### Task 2 — Fix the two exhaustive matches

In **both** `src/cli/commands/ask.rs` and `src/cli/commands/stream.rs`, add one
line to the ignore-arm chain, directly after its unique
`| Response::PaneList { .. }` line:

```rust
            | Response::ContainerStatus { .. }
```

Nothing else in either file changes.

### Task 3 — The collector, in `src/daemon/executor/container.rs`

Insert directly **before** the `/// Remove orphaned sandbox containers and
staging volumes. Best-effort:` doc comment of `sweep_sandbox_leftovers`:

```rust
/// The `--format` template `status_inspect_args` uses. Each container becomes
/// one line: `<id> <state> <image> <labels-as-json>`. The labels are JSON, not
/// a comma-joined string, because a `de.session` value carries a
/// webhook-supplied alert name — `docker ps`'s own `{{.Labels}}` joins pairs
/// with `,` and is irrecoverably ambiguous once a value contains one.
pub const STATUS_INSPECT_FORMAT: &str =
    "{{.Id}} {{.State.Status}} {{.Config.Image}} {{json .Config.Labels}}";

/// argv inspecting the given containers. **Empty when `ids` is empty** —
/// `docker inspect` with no arguments is a usage error (exit 1), and the empty
/// case is the common one.
pub fn status_inspect_args(cfg: &SandboxConfig, ids: &[String]) -> Vec<String> {
    if ids.is_empty() {
        return Vec::new();
    }
    let mut args = vec![
        "--host".to_string(),
        cfg.docker_host.clone(),
        "inspect".to_string(),
        "--format".to_string(),
        STATUS_INSPECT_FORMAT.to_string(),
    ];
    args.extend(ids.iter().cloned());
    args
}

/// Parse `status_inspect_args`' output into one record per container.
///
/// Pure. Splits each line into exactly four fields — the last is the whole
/// remaining text, so a label value containing a space, comma or `=` cannot
/// shift the parse. A line whose JSON does not decode is skipped rather than
/// guessed at.
pub fn parse_container_records(text: &str) -> Vec<crate::ipc::ContainerInfo> {
    text.lines()
        .filter_map(|line| {
            let mut parts = line.trim_end().splitn(4, ' ');
            let id = parts.next()?;
            let state = parts.next()?;
            let image = parts.next()?;
            let labels: std::collections::HashMap<String, String> =
                serde_json::from_str(parts.next()?).ok()?;
            Some(crate::ipc::ContainerInfo {
                id: id.chars().take(12).collect(),
                state: state.to_string(),
                image: image.to_string(),
                session: labels.get("de.session").cloned(),
                is_ghost: labels.contains_key("de.ghost"),
            })
        })
        .collect()
}

/// Runtime and image health plus every sandbox container this daemon owns.
/// Blocking — call it off the async runtime. Never fails: an unreachable
/// runtime is reported, not raised.
pub fn collect_container_status(cfg: &SandboxConfig) -> crate::ipc::ContainerStatusReport {
    let (runtime_ok, runtime_detail) = match probe_runtime(cfg) {
        Ok(version) => (true, version),
        Err(reason) => (
            false,
            describe_unavailable(&SandboxUnavailable::Runtime(reason)),
        ),
    };
    let image_detail = match read_lock() {
        None => format!("no lockfile at {}", lock_path().display()),
        Some(lock) => match check_image_matches(&lock, &probe_live_image_id(cfg)) {
            ImageCheck::Match => format!("{} ({})", cfg.image, lock.image_id),
            other => format!("{other:?}"),
        },
    };
    if !runtime_ok {
        return crate::ipc::ContainerStatusReport {
            enabled: cfg.enabled,
            runtime_ok,
            runtime_detail,
            image_detail,
            containers: Vec::new(),
        };
    }
    let mut cmd = Command::new(&cfg.runtime);
    cmd.args(sweep_container_list_args(cfg));
    let ids: Vec<String> = match bounded_output_with(&mut cmd, Duration::from_secs(30)) {
        Ok(out) => String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(str::to_string)
            .collect(),
        Err(e) => {
            log::warn!("container status list failed: {e}");
            Vec::new()
        }
    };
    let inspect = status_inspect_args(cfg, &ids);
    let containers = if inspect.is_empty() {
        Vec::new()
    } else {
        let mut cmd = Command::new(&cfg.runtime);
        cmd.args(inspect);
        match bounded_output_with(&mut cmd, Duration::from_secs(30)) {
            Ok(out) => parse_container_records(&String::from_utf8_lossy(&out.stdout)),
            Err(e) => {
                log::warn!("container status inspect failed: {e}");
                Vec::new()
            }
        }
    };
    crate::ipc::ContainerStatusReport {
        enabled: cfg.enabled,
        runtime_ok,
        runtime_detail,
        image_detail,
        containers,
    }
}
```

### Task 4 — The handler, in `src/daemon/server/handlers.rs`

Insert directly **before** `pub(super) async fn handle_status<W>(` (`:252`):

```rust
/// Sandbox runtime health and the containers this daemon owns. Collected off
/// the async runtime because it spawns the container runtime.
pub(super) async fn handle_container_status<W>(tx: &mut W, config: &Config) -> Result<()>
where
    W: tokio::io::AsyncWriteExt + Unpin,
{
    let sandbox = config.sandbox.clone();
    let report = tokio::task::spawn_blocking(move || {
        crate::daemon::executor::container::collect_container_status(&sandbox)
    })
    .await
    .unwrap_or_else(|e| {
        log::warn!("container status task failed: {e}");
        crate::ipc::ContainerStatusReport {
            enabled: config.sandbox.enabled,
            runtime_ok: false,
            runtime_detail: format!("status collection failed: {e}"),
            image_detail: String::new(),
            containers: Vec::new(),
        }
    });
    send_response_split(tx, Response::ContainerStatus { report }).await?;
    Ok(())
}
```

and the dispatch arm in `src/daemon/server/mod.rs`, directly after the
`Request::Status` arm:

```rust
        Request::ContainerStatus => {
            handle_container_status(&mut tx, &config).await?;
        }
```

`handlers::*` is glob-imported (`mod.rs:12`) — add no `use`.

### Task 5 — The SANDBOX section, in `src/cli/status.rs`

Inside the `Ok(Response::DaemonStatus { .. })` arm, directly after the
`COST TODAY` block's closing `}` and before the arm's own closing brace:

```rust
                    // ── SANDBOX ───────────────────────────────────────────────
                    send_request(&mut tx, Request::ContainerStatus).await?;
                    if let Ok(Response::ContainerStatus { report }) = recv(&mut rx).await {
                        let mut sbx = Section::new("SANDBOX");
                        sbx.kv(
                            "enabled",
                            if report.enabled {
                                c_ok("yes")
                            } else {
                                c_dim("no")
                            },
                        );
                        sbx.kv(
                            "runtime",
                            if report.runtime_ok {
                                c_ok(&report.runtime_detail)
                            } else {
                                c_err(&report.runtime_detail)
                            },
                        );
                        sbx.kv("image", c_val(&report.image_detail));
                        sbx.kv("containers", c_num(&report.containers.len().to_string()));
                        for ci in &report.containers {
                            sbx.cont(format!(
                                "{} {} {} {}",
                                c_key(&ci.id),
                                c_val(&ci.state),
                                if ci.is_ghost { c_warn("ghost") } else { c_dim("session") },
                                c_dim(ci.session.as_deref().unwrap_or("-")),
                            ));
                        }
                        sbx.render(tw);
                    }
```

The `if let` is deliberate: a daemon too old to know the variant, or any other
response, leaves the section off rather than failing the whole command.

### Task 6 — Tests

**Six in `container.rs`'s existing `mod tests`,** appended at the end of the
module. Every name begins `container_status_`:

```rust
#[test]
fn container_status_inspect_args_are_empty_without_ids() {
    let cfg = SandboxConfig::default();
    assert!(
        status_inspect_args(&cfg, &[]).is_empty(),
        "docker inspect with no arguments is a usage error, not an empty result"
    );
}

#[test]
fn container_status_inspect_args_carry_the_json_label_format() {
    let cfg = SandboxConfig::default();
    let args = status_inspect_args(&cfg, &["abc".to_string(), "def".to_string()]);
    assert_eq!(args.first().map(String::as_str), Some("--host"), "{args:?}");
    assert!(args.iter().any(|a| a == "inspect"), "{args:?}");
    assert!(args.iter().any(|a| a == STATUS_INSPECT_FORMAT), "{args:?}");
    assert!(
        args.iter().any(|a| a.contains("json .Config.Labels")),
        "labels must come back as JSON, not docker's comma-joined string: {args:?}"
    );
    assert_eq!(
        &args[args.len() - 2..],
        &["abc".to_string(), "def".to_string()]
    );
}

#[test]
fn container_status_parses_a_ghost_and_an_interactive_record() {
    let text = concat!(
        "39c2a88ad4137144 running alpine:3.22 {\"de.sandbox\":\"1\",\"de.session\":\"sess-plain\"}\n",
        "a1997c9929c48003 exited alpine:3.22 {\"de.ghost\":\"1\",\"de.sandbox\":\"1\",\"de.session\":\"ghost-x\"}\n"
    );
    let got = parse_container_records(text);
    assert_eq!(got.len(), 2, "{got:?}");
    assert_eq!(got[0].id, "39c2a88ad413", "id is truncated for display");
    assert_eq!(got[0].state, "running");
    assert_eq!(got[0].session.as_deref(), Some("sess-plain"));
    assert!(!got[0].is_ghost);
    assert!(got[1].is_ghost);
    assert_eq!(got[1].session.as_deref(), Some("ghost-x"));
}

#[test]
fn container_status_survives_a_session_id_with_spaces_and_commas() {
    // A ghost id is `ghost-<alert>-<uuid>` and the alert name comes from a
    // webhook, so it can hold spaces, commas and `=`. Measured: docker's
    // own `{{.Labels}}` joins pairs with `,` and cannot be split back.
    let text = "abcdef0123456789 running img {\"de.ghost\":\"1\",\"de.session\":\"ghost-disk full,x=1-uuid\"}\n";
    let got = parse_container_records(text);
    assert_eq!(got.len(), 1, "{got:?}");
    assert_eq!(
        got[0].session.as_deref(),
        Some("ghost-disk full,x=1-uuid"),
        "the label value must survive the split intact"
    );
    assert_eq!(got[0].image, "img");
}

#[test]
fn container_status_skips_a_line_it_cannot_decode() {
    let text = concat!(
        "abcdef0123456789 running img not-json\n",
        "0123456789abcdef running img {\"de.session\":\"ok\"}\n",
        "too few fields\n"
    );
    let got = parse_container_records(text);
    assert_eq!(got.len(), 1, "only the decodable line survives: {got:?}");
    assert_eq!(got[0].session.as_deref(), Some("ok"));
}

#[test]
fn container_status_reports_an_unlabelled_container_without_a_session() {
    let text = "abcdef0123456789 created img {\"de.sandbox\":\"1\"}\n";
    let got = parse_container_records(text);
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].session, None);
    assert!(!got[0].is_ghost);
}
```

**Two wire round-trips in `src/ipc_tests.rs`.** Extend the existing
`Request::Status` assertion's test (`:659`) by adding, directly after it:

```rust
    assert!(matches!(
        roundtrip_req(&Request::ContainerStatus),
        Request::ContainerStatus
    ));
```

and append a new test at the end of the file:

```rust
#[test]
fn response_container_status_roundtrip() {
    let report = crate::ipc::ContainerStatusReport {
        enabled: true,
        runtime_ok: true,
        runtime_detail: "29.7.2".to_string(),
        image_detail: "daemoneye-agent-base (sha256:abc)".to_string(),
        containers: vec![crate::ipc::ContainerInfo {
            id: "39c2a88ad413".to_string(),
            state: "running".to_string(),
            image: "daemoneye-agent-base".to_string(),
            session: Some("ghost-disk full,x=1".to_string()),
            is_ghost: true,
        }],
    };
    match roundtrip_resp(&Response::ContainerStatus {
        report: report.clone(),
    }) {
        Response::ContainerStatus { report: got } => assert_eq!(got, report),
        other => panic!("wrong variant: {other:?}"),
    }
}
```

The `PartialEq` derives in Task 1 are what let this compare the whole payload
rather than one field.

### Task 7 — Mutation pair M1: the JSON label format is real

Mutation edits go through your `patch` tool — **`sed -i`, `perl -i` and `>`
redirects into a source file are banned by your contract and `bash` will
refuse them.** Append each marker and run to `/tmp/e2e-05.txt`. Run the gates
(§ End-to-end verification) only **after** all three pairs are restored.

1. **Apply.** `patch` `src/daemon/executor/container.rs` — `old_str` (measured
   unique, one occurrence):

   ```
       "{{.Id}} {{.State.Status}} {{.Config.Image}} {{json .Config.Labels}}";
   ```

   `new_str`:

   ```
       "{{.Id}} {{.State.Status}} {{.Config.Image}} {{.Labels}}";
   ```

   Then:
   ```sh
   echo "== M1 APPLIED ==" >> /tmp/e2e-05.txt
   cargo test --lib container_status 2>&1 | grep -E "FAILED|^test result:" >> /tmp/e2e-05.txt
   grep -c '{{json .Config.Labels}}' src/daemon/executor/container.rs >> /tmp/e2e-05.txt
   ```
   Measured on the prototype: **exactly 1 failed**, naming
   `container_status_inspect_args_carry_the_json_label_format`, and the
   `grep -c` prints `0`. This is the phase's central measured decision — with
   `{{.Labels}}` the runtime returns a comma-joined string that silently
   mis-attributes any session whose id contains a comma (§ Live measurement 1).
   A green suite here means nothing pins it — record a blocker.

2. **Restore.** The inverse `patch`, then:
   ```sh
   echo "== M1 RESTORED ==" >> /tmp/e2e-05.txt
   cargo test --lib container_status 2>&1 | grep -E "FAILED|^test result:" >> /tmp/e2e-05.txt
   grep -c '{{json .Config.Labels}}' src/daemon/executor/container.rs >> /tmp/e2e-05.txt
   ```
   The tests pass and the `grep -c` prints `1`.

### Task 8 — Mutation pair M2: the four-field split is real

Only after M1 is restored.

1. **Apply.** `patch` `src/daemon/executor/container.rs`:
   - `old_str`: `            let mut parts = line.trim_end().splitn(4, ' ');`
   - `new_str`: `            let mut parts = line.trim_end().splitn(5, ' ');`

   Then:
   ```sh
   echo "== M2 APPLIED ==" >> /tmp/e2e-05.txt
   cargo test --lib container_status 2>&1 | grep -E "FAILED|^test result:" >> /tmp/e2e-05.txt
   grep -c "splitn(5, ' ')" src/daemon/executor/container.rs >> /tmp/e2e-05.txt
   ```
   Measured: **exactly 1 failed**, naming
   `container_status_survives_a_session_id_with_spaces_and_commas` — the
   record whose label value contains a space — and the `grep -c` prints `1`.

2. **Restore.** The inverse `patch`, then:
   ```sh
   echo "== M2 RESTORED ==" >> /tmp/e2e-05.txt
   cargo test --lib container_status 2>&1 | grep -E "FAILED|^test result:" >> /tmp/e2e-05.txt
   grep -c "splitn(4, ' ')" src/daemon/executor/container.rs >> /tmp/e2e-05.txt
   ```
   The `grep -c` prints `1`.

### Task 9 — Mutation pair M3: the empty-id guard is real

Only after M2 is restored. **`    if ids.is_empty() {` alone is NOT unique —
measured, it occurs three times in this file** (`sweep_container_rm_args`,
`teardown_ghost_containers`, `status_inspect_args`), and even the three-line
`return Vec::new();` form occurs twice. The `old_str` below carries four more
lines to reach `"inspect".to_string(),`, which makes it unique (measured: one
occurrence).

1. **Apply.** `patch` `src/daemon/executor/container.rs` — `old_str`:

   ```
       if ids.is_empty() {
           return Vec::new();
       }
       let mut args = vec![
           "--host".to_string(),
           cfg.docker_host.clone(),
           "inspect".to_string(),
   ```

   `new_str`:

   ```
       let mut args = vec![
           "--host".to_string(),
           cfg.docker_host.clone(),
           "inspect".to_string(),
   ```

   Then:
   ```sh
   echo "== M3 APPLIED ==" >> /tmp/e2e-05.txt
   cargo test --lib container_status 2>&1 | grep -E "FAILED|^test result:" >> /tmp/e2e-05.txt
   grep -c 'if ids.is_empty()' src/daemon/executor/container.rs >> /tmp/e2e-05.txt
   ```
   Measured: **exactly 1 failed**, naming
   `container_status_inspect_args_are_empty_without_ids`, and the `grep -c`
   prints `2` (the other two guards remain). Without it `daemoneye status`
   shells out to a docker usage error on every run with no containers — the
   common case (§ Live measurement 3).

2. **Restore.** The inverse `patch`, then:
   ```sh
   echo "== M3 RESTORED ==" >> /tmp/e2e-05.txt
   cargo test --lib container_status 2>&1 | grep -E "FAILED|^test result:" >> /tmp/e2e-05.txt
   grep -c 'if ids.is_empty()' src/daemon/executor/container.rs >> /tmp/e2e-05.txt
   ```
   The `grep -c` prints `3`.

The `grep -c` after **each** direction is not optional: a `patch` whose
`old_str` matches the wrong line fails silently, and a mutation that never
applied certifies a vacuous guard. **All three failure counts above were
measured, not estimated.** If a mutation fails a different number of tests
than stated, do not adjust a test to match — record a blocker naming the
criterion.

### Task 10 — Capture the end-to-end evidence

**The § End-to-end block appends (`>> /tmp/e2e-05.txt`). If you need to run it
a second time — for any reason — `rm -f /tmp/e2e-05.txt` first and run the
whole sequence again from Task 7.** Two executions otherwise leave two copies
in the file, the paste holds one, and the self-check prints `PASTE MISMATCH`.
**Never edit `/tmp/e2e-05.txt` or the pasted block to reconcile them** — the
`PASTE MATCH` check is worth nothing if either side can be adjusted until they
agree, and what an edit removes is usually the failing line that mattered. Run
`cargo fmt --all` **before** the block so `fmt_exit` is a real `0`, not one
produced by a later fix.

Run the block in § End-to-end verification **verbatim and unmodified**, then
paste the resulting `/tmp/e2e-05.txt` into a new Update Log entry headed
`### Update — <date> (end-to-end verification)`. The server-authored
`(complete)` entry does not satisfy this. **The entry ends with the
self-check's verdict line, `PASTE MATCH`, bare on its own line after the
fenced block** — a tick in your final summary is not that line.

## Acceptance criteria

**Every count below was read off the architect's prototype of this exact
change, not derived from the spec text.**

- [ ] `grep -c 'pub const STATUS_INSPECT_FORMAT' src/daemon/executor/container.rs`
      prints `1`, and
      `grep -cE '^pub fn (status_inspect_args|parse_container_records|collect_container_status)\(' src/daemon/executor/container.rs`
      prints `3` (**before: 0, 0**).
- [ ] `grep -c 'pub struct ContainerInfo' src/ipc.rs` and
      `grep -c 'pub struct ContainerStatusReport' src/ipc.rs` each print `1`;
      `grep -c '^    ContainerStatus,$' src/ipc.rs` prints `1` (the request
      variant); `grep -c 'ContainerStatus { report: ContainerStatusReport }' src/ipc.rs`
      prints `1` (the response variant); and
      `grep -c 'Response::ContainerStatus { .. } => "ContainerStatus"' src/ipc.rs`
      prints `1` (the name arm). **All four before: 0.**
- [ ] `grep -c 'Response::ContainerStatus' src/cli/commands/ask.rs` and the
      same on `src/cli/commands/stream.rs` each print `1` (**before: 0, 0**) —
      § Gotchas 1.
- [ ] `grep -c 'handle_container_status' src/daemon/server/handlers.rs` prints
      `1` and the same grep on `src/daemon/server/mod.rs` prints `1`
      (**before: 0, 0**).
- [ ] `grep -c 'Request::ContainerStatus' src/cli/status.rs` prints `1`,
      `grep -c 'Response::ContainerStatus { report }' src/cli/status.rs` prints
      `1`, and `grep -c 'Section::new("SANDBOX")' src/cli/status.rs` prints `1`
      (**all before: 0**).
- [ ] `grep -c 'ContainerStatus' src/ipc_tests.rs` prints `5` (**before: 0**) —
      the request round-trip plus the response test.
- [ ] `cargo test --lib container_status 2>&1 | grep -c "^test .* ok$"` prints
      `7` — the six `container_status_*` tests in `container.rs` **plus**
      `response_container_status_roundtrip`, whose name the same filter
      matches. A count, not an exit status.
- [ ] `cargo test --lib` reports **1485** passing and `0 failed`
      (**before: 1478**), with `4 ignored` unchanged.
- [ ] `grep -c '{{json .Config.Labels}}' src/daemon/executor/container.rs`
      prints `1` (**before: 0**) — the format constant asks for JSON labels.
      (`grep -c '{{.Labels}}'` reads **2** on a correct tree, both inside
      comments explaining why that form is not used, so it is not the pin.)
- [ ] `grep -c 'if ids.is_empty()' src/daemon/executor/container.rs` prints `3`
      (**before: 2**) — `sweep_container_rm_args`, `teardown_ghost_containers`
      and the new `status_inspect_args`.
- [ ] `grep -rc "allow(dead_code)" src/ | awk -F: '{s+=$2} END {print s}'`
      prints `6` (**unchanged**).
- [ ] `sed -n '1,/^#\[cfg(test)\]/p' src/daemon/executor/container.rs | grep -c '\.unwrap()\|\.expect('`
      prints `0` (**before: 0**) — no new panicking idiom in production code.
- [ ] The § End-to-end entry shows `== M1 APPLIED ==`, `== M2 APPLIED ==` and
      `== M3 APPLIED ==` each failing **exactly one** named test — the three
      names in Tasks 7, 8 and 9 — all three `RESTORED` runs passing, with a
      `grep -c` line after each direction reading the value that task states.
- [ ] No new `#[allow(...)]` anywhere, no `unsafe`, no `TODO`.
- [ ] All four gates green: `cargo fmt --all`, `cargo build`,
      `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`.
- [ ] The § End-to-end entry contains the literal line `PASTE MATCH` (bare,
      with no surrounding backticks):
      `grep -c '^PASTE MATCH$' docs/dev/milestones/M19-sandbox-completion/phase-05-container-status-ipc.md`
      prints `1`.

## Test plan

Six unit tests in `container.rs`'s `mod tests` and two wire round-trips in
`src/ipc_tests.rs`, all given in full in Task 6. No new test file.

**The negative cases are the phase.**
`container_status_survives_a_session_id_with_spaces_and_commas` is the whole
argument for the JSON format — it holds a session id with a space, a comma and
an `=`, exactly the shape a webhook alert name produces, and M2 proves the
four-field split is what protects it.
`container_status_inspect_args_are_empty_without_ids` pins § Live measurement
3, where the failure is a *usage error on the common path*; M1 proves it.
`container_status_skips_a_line_it_cannot_decode` pins that a malformed record
is dropped rather than guessed at, and
`container_status_reports_an_unlabelled_container_without_a_session` pins that
a pre-phase-04 container reports `None` instead of being dropped.

`collect_container_status` spawns `docker` twice and is **not** unit-tested,
matching how `sweep_sandbox_leftovers` and `stage_script`'s success arm are
treated. Its two decisions that can be pure — the argv and the parse — are
tested and mutation-proved. The rendered `SANDBOX` section and the live
round-trip are verified by the architect at milestone close, through a real
`daemoneye status` against a running daemon.

Behaviour is unchanged with the sandbox disabled: the section still renders,
reporting `enabled no`. **If an existing test requires a change to pass, stop
and record a blocker.**

## End-to-end verification

Run this block verbatim from the repo root, **after** Tasks 7, 8 and 9 have
appended their mutation markers to `/tmp/e2e-05.txt` and all three pairs are
restored.

```sh
{
echo "== A. named tests (expect 7 ok) =="
cargo test --lib container_status 2>&1 | grep -E "^test |^test result:"; echo "cargo_exit=${PIPESTATUS[0]}"
echo "== B. full lib suite =="
cargo test --lib 2>&1 | grep -E "^test result:"; echo "cargo_exit=${PIPESTATUS[0]}"
echo "== C. gates =="
cargo fmt --all -- --check > /dev/null 2>&1; echo "fmt_exit=$?"
cargo clippy --all-targets --all-features -- -D warnings > /dev/null 2>&1; echo "clippy_exit=$?"
echo "== D. structural greps =="
echo -n "STATUS_INSPECT_FORMAT (1):   "; grep -c 'pub const STATUS_INSPECT_FORMAT' src/daemon/executor/container.rs
echo -n "three new fns (3):           "; grep -cE '^pub fn (status_inspect_args|parse_container_records|collect_container_status)\(' src/daemon/executor/container.rs
echo -n "ContainerInfo struct (1):    "; grep -c 'pub struct ContainerInfo' src/ipc.rs
echo -n "ContainerStatusReport (1):   "; grep -c 'pub struct ContainerStatusReport' src/ipc.rs
echo -n "request variant (1):         "; grep -c '^    ContainerStatus,$' src/ipc.rs
echo -n "response variant (1):        "; grep -c 'ContainerStatus { report: ContainerStatusReport }' src/ipc.rs
echo -n "response name arm (1):       "; grep -c 'Response::ContainerStatus { .. } => "ContainerStatus"' src/ipc.rs
echo -n "ask.rs arm (1):              "; grep -c 'Response::ContainerStatus' src/cli/commands/ask.rs
echo -n "stream.rs arm (1):           "; grep -c 'Response::ContainerStatus' src/cli/commands/stream.rs
echo -n "handler (1):                 "; grep -c 'handle_container_status' src/daemon/server/handlers.rs
echo -n "dispatch (1):                "; grep -c 'handle_container_status' src/daemon/server/mod.rs
echo -n "status.rs request (1):       "; grep -c 'Request::ContainerStatus' src/cli/status.rs
echo -n "status.rs response (1):      "; grep -c 'Response::ContainerStatus { report }' src/cli/status.rs
echo -n "SANDBOX section (1):         "; grep -c 'Section::new("SANDBOX")' src/cli/status.rs
echo -n "ipc_tests refs (5):          "; grep -c 'ContainerStatus' src/ipc_tests.rs
echo -n "json label format (1):       "; grep -c '{{json .Config.Labels}}' src/daemon/executor/container.rs
echo -n "ids.is_empty guards (3):     "; grep -c 'if ids.is_empty()' src/daemon/executor/container.rs
echo -n "allow total (6):             "; grep -rc "allow(dead_code)" src/ | awk -F: '{s+=$2} END {print s}'
echo -n "prod unwrap/expect (0):      "; sed -n '1,/^#\[cfg(test)\]/p' src/daemon/executor/container.rs | grep -c '\.unwrap()\|\.expect('
} >> /tmp/e2e-05.txt 2>&1
cat /tmp/e2e-05.txt
```

Paste the whole of `/tmp/e2e-05.txt` — mutation markers included — into your
Update Log entry as a fenced block, then run the self-check and paste its
verdict line into the same entry **bare, on its own line, with no backticks**:

```sh
D=docs/dev/milestones/M19-sandbox-completion/phase-05-container-status-ipc.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-05.txt
diff /tmp/pasted-05.txt /tmp/e2e-05.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

**Run the block exactly as written.** If a label in it has gone stale against
the criteria, that is a spec defect — record a blocker naming it rather than
editing the block.

## Authorizations

- Edit `src/ipc.rs`, `src/ipc_tests.rs`, `src/daemon/executor/container.rs`,
  `src/daemon/server/handlers.rs`, `src/daemon/server/mod.rs`,
  `src/cli/status.rs`, `src/cli/commands/ask.rs` and
  `src/cli/commands/stream.rs` only.
- Run `cargo fmt --all`, `cargo build`,
  `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`.
- No `#[allow(...)]` may be added or removed, and no `#[ignore]` may be added
  or removed.
- **Do not change any existing test's assertions.** Task 6's addition to the
  `Request::Status` round-trip test is the only edit an existing test receives.
- **Do not run `docker`, `podman`, or any container command**, and do not
  start, stop or query a system service. Every runtime behaviour this phase
  depends on was measured by the architect (§ Live measurements) and is
  re-verified at milestone close.
- Mutation edits go through `patch`. **Never `git checkout` a file to restore
  it** — it would discard this round's own uncommitted work.
- **Do not edit any other source file, and do not edit any doc other than this
  phase doc's Update Log.**
- **Append to the Update Log; never edit or delete an existing entry.** When
  flipping this doc's `Status:` line, change **only** that line — the line
  above it is `**Milestone:** M19 — Sandbox Completion` and must survive (a
  mis-anchored status patch ate it in phase-03; see `bugs/bug-phase-03-1.md`).
  After the flip, `grep -c '^\*\*Status:\*\*' <this doc>` must print `1` and
  `grep -c '^\*\*Milestone:\*\*' <this doc>` must print `1`.
- **Never edit `/tmp/e2e-05.txt` or the pasted evidence block after capture,
  for any reason** (Task 10). On a `PASTE MISMATCH`, delete the artifact and
  re-run the sequence; if a mismatch survives a clean re-run, record a
  blocker. This is `bugs/bug-phase-04-1.md`.
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

- **The `/panes`-style container inspector** the design mentions alongside
  `daemoneye status`. This phase ships the IPC surface and one status section;
  a dedicated inspector command has no phase.
- **Per-session filtering over IPC.** `Request::ContainerStatus` takes no
  arguments and reports everything this daemon owns; the client filters if it
  ever needs to.
- **Volumes.** `de-stage-*` volumes carry no labels and are reclaimed per job
  (phases 02–03); the report covers containers only.
- **`ghost_defaults.mount_scripts`** — still parsed and read by nothing; still
  has no phase. Recorded in the milestone README.
- **`profile.network` / `proxy_allow`** — phases 06–08. The report has no
  network column yet; adding one belongs to the phase that gives networks
  meaning.
- **Refusing to run on an image mismatch.** `evaluate_preflight` already does
  that; this phase only *reports* the comparison.
- `CLAUDE.md`, `README.md`, the design doc — the phase-10 doc sweep. The
  design's `Response::ContainerStatus` shape gains `enabled` and
  `image_detail` here and should be amended there.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-08-30 15:10 (started)

Executor started phase-05. Implementing the wire types, collector, handler,
SANDBOX section and tests per the spec, then running the mutation pairs and
capturing end-to-end evidence.


### Update — 2026-08-30 15:20 (end-to-end verification)

All three mutation pairs applied, run and restored; the full artifact and the
self-check verdict follow.

```text
== M1 APPLIED ==
test daemon::executor::container::tests::container_status_inspect_args_carry_the_json_label_format ... FAILED
test result: FAILED. 6 passed; 1 failed; 0 ignored; 0 measured; 1482 filtered out; finished in 0.00s
0
== M1 RESTORED ==
test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 1482 filtered out; finished in 0.00s
1
== M2 APPLIED ==
test daemon::executor::container::tests::container_status_survives_a_session_id_with_spaces_and_commas ... FAILED
test result: FAILED. 6 passed; 1 failed; 0 ignored; 0 measured; 1482 filtered out; finished in 0.00s
1
== M2 RESTORED ==
test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 1482 filtered out; finished in 0.00s
1
== M3 APPLIED ==
test daemon::executor::container::tests::container_status_inspect_args_are_empty_without_ids ... FAILED
test result: FAILED. 6 passed; 1 failed; 0 ignored; 0 measured; 1482 filtered out; finished in 0.00s
2
== M3 RESTORED ==
test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 1482 filtered out; finished in 0.00s
3
== A. named tests (expect 7 ok) ==
test daemon::executor::container::tests::container_status_inspect_args_are_empty_without_ids ... ok
test daemon::executor::container::tests::container_status_inspect_args_carry_the_json_label_format ... ok
test daemon::executor::container::tests::container_status_skips_a_line_it_cannot_decode ... ok
test daemon::executor::container::tests::container_status_reports_an_unlabelled_container_without_a_session ... ok
test daemon::executor::container::tests::container_status_survives_a_session_id_with_spaces_and_commas ... ok
test daemon::executor::container::tests::container_status_parses_a_ghost_and_an_interactive_record ... ok
test ipc::tests::response_container_status_roundtrip ... ok
test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 1482 filtered out; finished in 0.00s
cargo_exit=0
== B. full lib suite ==
test result: ok. 1485 passed; 0 failed; 4 ignored; 0 measured; 0 filtered out; finished in 3.74s
cargo_exit=0
== C. gates ==
fmt_exit=0
clippy_exit=0
== D. structural greps ==
STATUS_INSPECT_FORMAT (1):   1
three new fns (3):           3
ContainerInfo struct (1):    1
ContainerStatusReport (1):   1
request variant (1):         1
response variant (1):        1
response name arm (1):       1
ask.rs arm (1):              1
stream.rs arm (1):           1
handler (1):                 1
dispatch (1):                1
status.rs request (1):       1
status.rs response (1):      1
SANDBOX section (1):         1
ipc_tests refs (5):          5
json label format (1):       1
ids.is_empty guards (3):     3
allow total (6):             6
prod unwrap/expect (0):      0
```

PASTE MATCH

### Update — ts=1788102950848 (complete, server-authored)

**Summary:** ## Summary + Notes for review

Implemented phase-05 end-to-end: `Request::ContainerStatus` / `Response::ContainerStatus` wire types in `src/ipc.rs` (with `ContainerInfo` and `ContainerStatusReport` payload structs and the name arm), the two exhaustive-match fixes in `ask.rs`/`stream.rs`, the collector (`STATUS_INSPECT_FORMAT`, `status_inspect_args`, `parse_container_records`, `collect_container_status`) in `src/daemon/executor/container.rs`, the `handle_container_status` handler + `Request::ContainerStatus` dispatch arm, the SANDBOX section in `src/cli/status.rs`, and the six `container_status_*` unit tests plus two wire round-trips.

All three mutation pairs behaved exactly as the spec predicted: each APPLIED direction failed exactly the one named test, each RESTORED direction passed, and every `grep -c` matched the stated value (including `ids.is_empty` = 2/3 and `json label format` = 0/1). No deviations were required — the spec's measured anchors all held. The complete `/tmp/e2e-05.txt` artifact (mutation markers, 7 named tests, 1485 lib tests, gates) was pasted into its own Update Log entry and the self-check printed `PASTE MATCH`; the `PASTE MATCH` line is bare on its own line in the doc. `cargo fmt --all`, `cargo build`, `clippy -D warnings` and `cargo test` all pass (1485 passed / 0 failed / 4 ignored, unchanged ignores), total `allow(dead_code)` stays at 6, no new `#[allow]`, no `unsafe`, no TODO, no panicking idioms in production code, and the working tree is clean after the single `feat:` commit. Status remains `in-progress` and the server-authored completion entry is pending as usual.

Notable for review: `collect_container_status` is intentionally untested as a whole (spawns docker twice), matching the spec's treatment of `sweep_sandbox_leftovers`; the `/panes`-style inspector is out of scope per the phase doc.

**Executor:** deepseek-v4-flash-0731

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.07s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.07s


TEST
nored; 0 measured; 0 filtered out; finished in 3.91s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 6 tests
test header_status_reads_bare_word ... ok
test header_status_strips_trailing_prose ... ok
test header_status_uses_first_occurrence_only ... ok
test open_bug_on_in_progress_phase_is_clean ... ok
test open_bug_on_done_phase_is_a_finding ... ok
test repository_bug_tracker_is_consistent ... ok

test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 10 tests
test approval_gated_tools_all_exist ... ok
test claude_md_tools_table_counts_are_accurate ... ok
test claude_md_tools_table_matches_the_code ... ok
test readme_tools_counts_are_accurate ... ok
test readme_approval_markers_match_the_gated_tools ... ok
test readme_tools_tables_match_the_code ... ok
test docs_document_the_reindex_command ... ok
test docs_do_not_carry_retired_index_claims ... ok
test seeded_config_template_has_no_phantom_keys ... ok
test seeded_config_template_documents_every_config_field ... ok

test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 33 tests
test daemon_ping_status_loop ... ignored
test cancel_request_roundtrip ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g4_briefing_injection_block_format ... ok
test g5_depth_limit_enforced ... ok
test g5_child_inherits_depth_and_parent ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ipc_ask_round_trip ... ok
test ipc_session_info_round_trip ... ok
test ipc_tool_call_response_round_trip ... ok
test ghost_config_parsing ... ok
test minimal_config_parsing ... ok
test window_switch_does_not_corrupt_chat ... ignored
test schedule_store_persistence ... ok
test config_pricing_round_trip ... ok
test g4_briefing_masking_applied ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test event_log_entry_format ... ok
test event_log_append_read ... ok
test g4_briefing_injects_on_next_run ... ok
test g4_briefing_read_and_clear ... ok
test g6_agent_namespace_field_persisted ... ok
test g6_agent_config_roundtrip ... ok
test session_index_persistence ... ok
test session_jsonl_round_trip ... ok
test webhook_alert_below_threshold_discarded ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_no_severity_passes_gate ... ok
test webhook_alert_to_event_log ... ok
test webhook_alert_unrankable_severity_passes_gate ... ok

test result: ok. 31 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.04s


running 10 tests
test webhook_ghost_e2e_http ... ignored
test held_port_cannot_be_rebound ... ok
test webhook_ports_differ_between_environments ... ok
test stub_returns_canned_response_via_make_client ... ok
test webhook_ghost_e2e_deterministic ... ok
test config_contains_webhook_and_stub_url ... ok
test daemon_boots_in_throwaway_root ... ok
test hooks_land_on_private_server ... ok
test default_server_unchanged ... ok
test daemon_webhook_returns_200 ... ok

test result: ok. 9 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.15s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.07s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-b60224cb24515ede)
     Running unittests src/main.rs (target/debug/deps/daemoneye-e700f2084319867a)
     Running tests/bug_tracker.rs (target/debug/deps/bug_tracker-9b22636ef5c08466)
     Running tests/doc_truth.rs (target/debug/deps/doc_truth-c00c74ef4ffe9c11)
     Running tests/integration.rs (target/debug/deps/integration-6230826c10f36795)
     Running tests/isolation.rs (target/debug/deps/isolation-66949bca409172a9)
   Doc-tests daemoneye

```

**Files changed:**

- `docs/dev/milestones/M19-sandbox-completion/README.md` — +1 -1
- `docs/dev/milestones/M19-sandbox-completion/phase-05-container-status-ipc.md` — +75 -1
- `src/cli/commands/ask.rs` — +1 -0
- `src/cli/commands/stream.rs` — +1 -0
- `src/cli/status.rs` — +38 -0
- `src/daemon/executor/container.rs` — +195 -0
- `src/daemon/server/handlers.rs` — +25 -0
- `src/daemon/server/mod.rs` — +3 -0
- `src/ipc.rs` — +36 -0
- `src/ipc_tests.rs` — +27 -0

**Commit:** 342f823264074251bd1bdd06047b9393feb8aeef

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

### Review verdict — 2026-08-30

- **Verdict:** approved_first_try
- **Bounces:** none
- **Executor:** deepseek-v4-flash-0731 (127 turns, commit `342f823`)
- **Scope deviations:** none. The production diff is **byte-identical** to
  the architect's prototype (`diff` of the two `src/` diffs is empty), the
  status flip touched only the `**Status:**` line (one `**Status:**`, one
  `**Milestone:**` after it), and no existing test's assertions changed.
- **Calibration:** none new — and one confirmation. This is the first phase
  dispatched with every claim in the summary pinned to a command in the doc,
  three mutation pairs whose failure counts were measured before dispatch,
  and the repeat-run rule in Task 10. The summary made no claim the artifact
  contradicts, the artifact holds exactly one execution, and the executor
  filed no blocker because none was needed. Three phases of fold, one clean
  run: hold, do not add.

Independent re-run at review (four separate invocations): `cargo fmt --all`
→ 0; `cargo build` → 0; `cargo clippy --all-targets --all-features -- -D
warnings` → 0; `cargo test` → **1485 passed; 0 failed; 4 ignored** in the lib
suite, every other target green.

Every acceptance criterion re-measured on the tree under review: the format
constant and three new fns, both wire structs, the request variant, response
variant and name arm, the two exhaustive-match arms in `ask.rs`/`stream.rs`,
handler and dispatch, the three `status.rs` sites, five `ipc_tests.rs`
references, `{{json .Config.Labels}}` at 1, `if ids.is_empty()` at 3,
`allow(dead_code)` total 6, no production `unwrap`/`expect`, 7 named tests
green, the pinned `run_args` vector still passing. The reviewer's own run of
the § End-to-end self-check against the last entry prints `PASTE MATCH`; the
pasted block holds one `== A. named tests` and one `== C. gates ==` line, and
all three `APPLIED` markers show exactly the named failure.

**Tests spot-checked as real with two mutations the phase doc does not
name.** Deriving `is_ghost` from nothing (`false`) and dropping the 12-char id
truncation each fail exactly
`container_status_parses_a_ghost_and_an_interactive_record` and nothing else.
Together with the phase's own M1–M3 that is five independent breakages, five
distinct single-test failures: the parser's fields are pinned separately, not
jointly by one assertion.
