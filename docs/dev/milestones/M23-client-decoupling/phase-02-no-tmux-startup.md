# Phase 02: no-tmux-startup — with `tmux_session = ""`, a daemon launched outside tmux never starts, adopts or talks to a tmux server

**Milestone:** M23 — Client decoupling & shell UX
**Status:** todo
**Depends on:** none
**Estimated diff:** ~40 lines of production code in 1 file; 2 docs; 2 new tests
**Tags:** language=rust, kind=feature, size=s

## Goal

Launched **outside** tmux, the daemon always creates or adopts a managed tmux
session, even when `[daemon] tmux_session = ""`. With an empty name it runs
`tmux new-session -d -s ''`, which **starts a tmux server**. It then registers
four global tmux hooks and runs a background-window GC every 60 s, both of
which assume tmux. After this phase, an empty session name, from
`--session ""` or `tmux_session = ""`, means **no tmux at all**:
- no session is created or adopted;
- no global hook is registered;
- the background-window GC does not run;
- one info line in `daemon.log` says so.

The PE decided at M23 sign-off that the default stays `"daemoneye"`, so only
`""` turns tmux off, and **nothing changes inside tmux** or for a non-empty
name. The client side already works with `""`. That was measured at scoping:
`chat` skips its managed-session attach when the name is empty.

## Architecture references

- `docs/dev/milestones/M23-client-decoupling/README.md` — row 02, the
  measured no-tmux facts, and the PE decisions at sign-off.
- `docs/design/daemoneye-2.0.md` § 2.6 — chat runs in any terminal.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read this entire phase doc before touching any code.
3. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

Measured on **2026-10-01** at commit `c2646f3f`.

Baseline: `cargo test --lib` → **2098 passed / 0 failed / 14 ignored**;
`cargo test --test isolation` → **10 passed / 1 ignored**.

### 1. The startup decision (`src/daemon/mod.rs:605-647`)

```rust
    let (initial_session, managed_session): (Option<String>, Option<String>) = if let Some(name) =
        inside_session
    {
        log::info!("Launched inside tmux — adopting session '{}'.", name);
        (Some(name), None)
    } else {
        let name = session_override.unwrap_or_else(|| startup_config.daemon.tmux_session.clone());
        let n = name.clone();
        let exists = …session_exists(&n)…;
        if exists {
            …adopt…
        } else {
            …`tmux new-session -d -s <name>`, and bail if it fails…
        }
    };
```

The comment above it says "A tmux session is always required — there is no
degraded 'no session' mode". After this phase that is false for `""`, so
**update that comment**.

### 2. What else assumes tmux at startup

- **Four global hooks** (`src/daemon/mod.rs:661-732`). `let hook_exe_path = …`
  (`:661-663`) is followed by four `tmux set-hook -g …` calls (`pane-died`,
  `after-new-session`, `client-attached`, `client-detached`), each ending in
  `log_hook_install_result("<hook>", res);`. `hook_exe_path` is **also used
  after** that block (`:737`, inside `if let Some(ref sn) = initial_session`,
  which is already gated), so keep its definition outside any new gate.
- **The background-window GC** (`src/daemon/mod.rs:1010-1027`): a supervised
  task that loops `sleep(60s)` then `gc_bg_windows(&s)`.

### 3. Measured with a throwaway daemon that could reach no tmux server

A probe binary carried the planned empty-name branch, with its own `HOME`,
`tmux_session = ""` and `TMUX_TMPDIR` pointed at an empty directory:

- The daemon started and answered `ping`. **No tmux server was created.**
- A full tool-calling chat turn against it (`echo … $(tty)` in a PTY shell,
  exit 0) worked from a terminal with no tmux reachable. **Still no server
  afterwards.**
- `daemon.log` then held **four `ERROR`s**, one per global hook
  (`Failed to register global tmux pane-died hook (exit status: 1): error connecting to …`),
  and **one `WARN`** (`gc_bg_windows: failed to list panes`). The GC `WARN`
  would recur every 60 s. Nothing else repeated: 0 new log lines over 20 s.

Then the **whole** change below (predicate + branch + both gates) was applied,
with an `isolation.rs` probe test calling `start_daemon("")`. Results: `ping`
succeeded, **no `tmux-*` directory** appeared under the private `TMUX_TMPDIR`,
**0** hook errors and **0** GC warnings were logged, and all existing
isolation tests still passed. `--session ""` reaches the daemon as an empty
string; clap accepts it.

### 4. The isolation harness (`tests/harness/mod.rs`)

`IsolatedEnv::new()` gives a throwaway root that is both `HOME` and
`TMUX_TMPDIR`. `env.start_daemon(session)` runs `daemoneye setup`, writes a
test config, then `daemoneye daemon --session <session>`. `env.daemoneye(&[…])`
runs a command in the environment. `env.root()` is the root path, and
`env.daemon_log()` returns `daemon.log`'s text. A tmux server, if one
starts, creates a `tmux-<uid>/` directory directly under the root. The
existing `daemon_boots_in_throwaway_root` (`tests/isolation.rs:51`) is the
shape to follow.

## Spec

### Task 1 — the predicate and the branch (`src/daemon/mod.rs`)

1. Add, above the `#[cfg(test)] mod tests`:

   ```rust
   /// Whether the daemon should use tmux when launched outside it: an empty
   /// (or all-whitespace) session name, from `--session` or `[daemon]
   /// tmux_session`, means no tmux at all.
   fn tmux_wanted(session_override: Option<&str>, configured: &str) -> bool {
       !session_override.unwrap_or(configured).trim().is_empty()
   }
   ```

2. In the startup decision, between the `inside_session` branch and the
   existing `else`, add:

   ```rust
       } else if !tmux_wanted(session_override.as_deref(), &startup_config.daemon.tmux_session) {
           log::info!(
               "tmux_session is empty: no managed tmux session, tmux hooks or background-window GC."
           );
           (None, None)
   ```

   The existing `else` block stays byte-identical. Update the comment above
   the decision. Replace "A tmux session is always required — there is no
   degraded "no session" mode." with: "An empty session name means no tmux at
   all: no session, no global hooks, no background-window GC."

### Task 2 — gate what assumes tmux (`src/daemon/mod.rs`)

1. Keep `let hook_exe_path = …;` where it is. Wrap **only** the four global
   hook installs, from `let global_notify_cmd = …` through
   `log_hook_install_result("client-detached", res);` inclusive, in
   `if initial_session.is_some() { … }`.
2. Wrap the background-window GC's `tokio::spawn(supervise("bg-window-gc", …));`
   in `if initial_session.is_some() { … }`. With no tmux, no `de-bg-*`
   window can exist.

Inside tmux, or with a non-empty name, `initial_session` is `Some`, so both
run exactly as today.

### Task 3 — docs

1. `assets/etc/config.toml:303`: append to the `tmux_session` comment so it
   reads
   `# tmux_session = "daemoneye"   # session the daemon creates/adopts when launched outside tmux; "" = no tmux at all`.
2. `README.md`, the `[daemon]` table row for `tmux_session` (`:1027`): append
   the sentence
   `Set it to `""` for no tmux at all: launched outside tmux, the daemon then creates no session and registers no tmux hooks, and `daemoneye chat` does not attach.`

### Task 4 — tests

1. In `src/daemon/mod.rs` tests: `an_empty_session_name_means_no_tmux`.
   - `tmux_wanted(None, "")` → `false`; `tmux_wanted(None, "   ")` → `false`;
     `tmux_wanted(Some(""), "daemoneye")` → `false`, because an empty
     override wins.
   - `tmux_wanted(None, "daemoneye")` → `true`;
     `tmux_wanted(Some("work"), "")` → `true`.
2. In `tests/isolation.rs`: `an_empty_session_starts_no_tmux_server`.
   `let mut env = IsolatedEnv::new(); env.start_daemon("");` then:
   - `env.daemoneye(&["ping"])` succeeds;
   - **no** entry of `std::fs::read_dir(env.root())` has a name starting with
     `"tmux-"` (no tmux server was started);
   - `env.daemon_log()` contains `"tmux_session is empty"`, and contains
     **neither** `"Failed to register global tmux"` **nor** `"gc_bg_windows"`.

   It needs **no** `tmux_available()` guard: it must pass with or without
   tmux installed.

**Test count:** lib **2099 passed / 14 ignored**; isolation **11 passed / 1
ignored**.

### Task 5 — capture the end-to-end evidence

Run the block in § End-to-end verification verbatim. Paste its output **into
this phase doc** as the first fenced block of a new Update Log entry titled
exactly `### Update — <date> (end-to-end verification)`. Then run the paste
check, and **paste the line it prints into the entry on a line of its own**.

## Acceptance criteria

Parenthesised values are what each line produces **against the current
tree**. Every non-preservation line fails today; each was run at drafting.

- [ ] `cargo test --lib` → **2099 passed / 0 failed / 14 ignored** on three
      consecutive runs (current: 2098/0/14). Both counts are exact.
- [ ] `cargo test --test isolation` → **11 passed / 0 failed / 1 ignored**
      (current: 10/0/1).
- [ ] `cargo fmt --all` clean; `cargo build` clean;
      `cargo clippy --all-targets --all-features -- -D warnings` exits 0.
- [ ] `cargo test` → other targets unmoved: bug_tracker 6, doc_truth 14,
      integration 32 (*preservation*).
- [ ] The two new tests pass:
      ``cargo test --lib an_empty_session_name_means_no_tmux`` → **1 passed**
      and ``cargo test --test isolation an_empty_session_starts_no_tmux_server``
      → **1 passed** (current: 0 and 0, filtered out).
- [ ] The existing boot test still passes (*preservation*):
      ``cargo test --test isolation daemon_boots_in_throwaway_root`` → **1
      passed**.
- [ ] Docs: ``grep -c 'no tmux at all' assets/etc/config.toml`` → **1** and
      ``grep -c 'no tmux at all' README.md`` → **1** (current: 0 and 0).
- [ ] Mutations A and B were applied, run, and their observed results recorded
      **in this doc**.
- [ ] The § End-to-end verification block was run verbatim and pasted as the
      **first fenced block** of the `(end-to-end verification)` entry:
      ``grep -c '^### Update — .* (end-to-end verification)$' <this doc>`` →
      **1** (current: 0).
- [ ] The paste-check line is in the doc on its own line:
      ``grep -cx 'PASTE MATCH' <this doc>`` → **1** (current: 0).

## Mutations

Apply each with the `patch` tool (the executor contract forbids `sed -i`,
`perl -i`, `> file` and `rm -rf`), run once, restore, and **record the observed
result in this doc's end-to-end entry**.

- **Mutation A:** make `tmux_wanted` return `true` unconditionally. Run
  isolation test 2. Expected **red**: the daemon runs
  `tmux new-session -d -s ''` and a `tmux-*` directory appears.
- **Mutation B:** remove the `if initial_session.is_some()` gate around the
  four global hook installs (keep the branch from Task 1). Run isolation test
  2. Expected **red**: four `Failed to register global tmux` errors in
  `daemon.log`.

After each restore, confirm `git diff --stat` matches its pre-mutation state.

## Test plan

Two tests. A pure test pins what counts as "no tmux". An end-to-end test
boots a real daemon with `--session ""` in an isolated root and proves no
tmux server, no hook errors and no GC noise.

## End-to-end verification

```sh
cd /home/matt/src/daemoneye
{
  for i in 1 2 3; do
    out=$(cargo test --lib 2>&1)
    printf 'lib%d=%s %s\n' "$i" "$(printf '%s\n' "$out" | grep -E '^test result' | tail -1 | grep -oE 'ok|FAILED')" "$(printf '%s\n' "$out" | grep -E '^test .* FAILED$' | sed -E 's/^test (.*) \.\.\. FAILED$/\1/' | tr '\n' ' ')"
  done
  printf 'lib_counts=%s\n' "$(cargo test --lib 2>&1 | grep -E '^test result' | tail -1 | sed -E 's/.*ok\. ([0-9]+) passed; ([0-9]+) failed; ([0-9]+) ignored.*/\1p-\2f-\3i/')"
  printf 'isolation_counts=%s\n' "$(cargo test --test isolation 2>&1 | grep -E '^test result' | tail -1 | sed -E 's/.*ok\. ([0-9]+) passed; ([0-9]+) failed; ([0-9]+) ignored.*/\1p-\2f-\3i/')"
  printf 'predicate_test=%s\n' "$(cargo test --lib an_empty_session_name_means_no_tmux 2>&1 | grep -E '^test result' | tail -1 | grep -oE '[0-9]+ passed')"
  printf 'no_tmux_test=%s\n' "$(cargo test --test isolation an_empty_session_starts_no_tmux_server 2>&1 | grep -E '^test result' | tail -1 | grep -oE '[0-9]+ passed')"
  printf 'boot_test=%s\n' "$(cargo test --test isolation daemon_boots_in_throwaway_root 2>&1 | grep -E '^test result' | tail -1 | grep -oE '[0-9]+ passed')"
  printf 'docs=%s/%s\n' "$(grep -c 'no tmux at all' assets/etc/config.toml)" "$(grep -c 'no tmux at all' README.md)"
} > /tmp/e2e-m23-02.txt
cat /tmp/e2e-m23-02.txt
```

Expected: `lib1..3=ok` with **nothing after `ok`**, `lib_counts=2099p-0f-14i`,
`isolation_counts=11p-0f-1i`, `predicate_test=1 passed`,
`no_tmux_test=1 passed`, `boot_test=1 passed`, `docs=1/1`.

**Re-run budget: three** (WORKFLOW.md § End-to-end verification). The known
pre-existing flake is `shell::backend::tests::input_after_a_prompt_logs_a_redaction_marker`.
If it fails, name it and re-run the whole block. After three re-runs, stop,
paste the last capture as it is, and name every failing test. Any other
failing test is a finding: report it by name.

**Run this block with nothing else running `cargo test`.** Paste the file into
this phase doc, then run:

```sh
D=docs/dev/milestones/M23-client-decoupling/phase-02-no-tmux-startup.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-m23-02.txt
diff /tmp/pasted-m23-02.txt /tmp/e2e-m23-02.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

Paste the line that command prints into the entry, **on its own line**. Then
record both mutations' observed results in the same entry.

## Authorizations

You may edit these files, and nothing else:

- `src/daemon/mod.rs`: Tasks 1, 2 and test 1.
- `tests/isolation.rs`: test 2.
- `assets/etc/config.toml` (the one comment) and `README.md` (the one table
  row): Task 3.
- This phase doc's Update Log.

## Out of scope

- **Changing the `"daemoneye"` default.** That is a PE decision, already made.
- **Anything inside tmux** or with a non-empty name: unchanged by design.
- **The client side.** `chat` already skips its attach for `""` (measured at
  scoping); this phase does not touch `src/cli/`.
- **Running without tmux installed at all** with a non-empty name. That
  still fails at `new-session`, as today. M26 removes tmux.

## Update Log

<!-- entries appended below this line -->
