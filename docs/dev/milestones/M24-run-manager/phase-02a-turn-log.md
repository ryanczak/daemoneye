# Phase 02a: the per-run turn log

**Milestone:** M24 — Run Manager
**Status:** done
**Depends on:** phase-01 (`RunId`, `RunKind`, `RunState` — the types a record
carries). Done.
**Estimated diff:** ~330 lines incl. tests
**Tags:** language=rust, kind=feature, size=m

## Goal

One append-only JSONL per run at `var/log/runs/<run-id>.jsonl`: an
`event_type`-tagged record envelope, a writer that flushes per record, and a
reader that tolerates a malformed line. Plus the two things a **new artifact
directory** requires in this repo — a path constructor and a lifecycle-policy
row — without which the existing gates fail (F1).

**No wiring.** Nothing in the daemon writes a turn log yet; phases 05–08 do.
The `run_log_*` / `get_turn` **tools are phase 02b**, not this phase.

## Architecture references

- `docs/design/daemoneye-2.0.md` § 2.3, the turn-log row: *"one JSONL per run,
  `event_type`-tagged, per-record flushed, domain types serialised verbatim,
  `#[serde(default)]` on every later field"*.
- `docs/dev/milestones/M24-run-manager/README.md` § Exit criteria — "one turn-log
  format at `var/log/runs/<id>.jsonl`".

## Pre-flight

- `git rev-parse --abbrev-ref HEAD` → `master`, tree clean.
- `cargo test --lib` → **1676 passed; 0 failed; 4 ignored**.
- `cargo test --lib daemon::run::` → **10 passed** (phase-01's types).

## Current state

Counts today: full lib **1676 passed; 0 failed; 4 ignored**; `daemon::run`
**10**, `config::lifecycle` **5**, `daemon::session` **21**.

`grep -c 'pub fn runs_dir' src/config/load.rs` → **0**; there is no
`var/log/runs` anywhere in `src/`.

## Measured facts — executed 2026-09-09, not reasoned about

**F1 — a new artifact directory is gated in BOTH directions; miss either and a
test fails.** `src/config/lifecycle.rs`'s own test module enforces:

- `every_existing_directory_has_a_policy_entry` — a directory that exists at
  runtime with no row in the policy table **fails**.
- `every_eager_policy_entry_is_created_by_ensure_dirs` — a row that is not
  `lazy` and is **not** created by `ensure_dirs()` **fails**.
- `every_policy_entry_corresponds_to_a_real_path` — a row naming a path nothing
  creates **fails**.

Plus **two more the original draft of this phase missed**, both in
`src/config/runtime_tree.rs` and both found by the executor at run time
(corrected here at review 2026-09-09):

- `every_policy_path_appears_in_tree` — a `POLICY_TABLE` path with no leaf in
  `RUNTIME_TREE` **fails**.
- `render_matches_shipped_asset` — `RUNTIME_TREE` must still render to the
  shipped `assets/memory/knowledge/agent-runtime-layout.md`, so that asset
  needs the same leaf.

So `var/log/runs` needs **all five**: the `runs_dir()` constructor, a
`create_dir_all(runs_dir())` in `ensure_dirs()`, a `LifecycleEntry`, a
`RUNTIME_TREE` leaf, and the matching line in the shipped runtime-layout asset.
Verified at review by deleting the leaf: both tree tests go red together.
There is also a `mutation_check_uncovered_directory_fails_gate` test proving
the first gate bites. **This is the single most likely way to bounce this
phase.**

**F2 — `ensure_dirs()` is the eager-creation site** (`src/config/seeds.rs:19+`),
a flat list to add one line to:

```rust
        std::fs::create_dir_all(shells_dir())?;
        std::fs::create_dir_all(shell_logs_dir())?;
        std::fs::create_dir_all(var_index_dir())?;
        …
        std::fs::create_dir_all(sessions_dir())?;
```

and `sessions_dir()` (`src/config/load.rs:107`) is the constructor to copy —
`var/log/sessions` is this phase's closest analogue in every respect.

**F3 — a `Pending` lifecycle row is legitimate and is the right shape here.**
Retention for run logs needs a config key, and a new config key drags in
`assets/etc/config.toml` plus two `doc_truth` gates
(`seeded_config_template_documents_every_config_field`,
`seeded_config_template_has_no_phantom_keys`) — out of scope for this phase.
`var/log/shells` carried `ImplementationStatus::Pending { owned_by: … }` across
a whole milestone for exactly this reason, and `config_key: None` is an
established value (`var/run/shells`, `var/log/pipe`). Use `Pending`.

**F4 — the append-and-flush idiom already in the tree**
(`src/daemon/session.rs:354-365`), which is what "per-record flushed" means
here — `writeln!` on an append handle, and a **warn, never a panic**, when the
write fails:

```rust
    use std::fs::OpenOptions;
    use std::io::Write;
    let path = session_file(id);
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&path) {
        if let Ok(line) = serde_json::to_string(msg)
            && let Err(e) = writeln!(f, "{}", line)
        {
            log::warn!("Failed to append to session file {}: {}", path.display(), e);
        }
    } else {
        log::warn!("Failed to open session file {} for append", path.display());
    }
```

**F5 — a malformed line is skipped, not fatal.** The project's existing JSONL
reader states the rule (`src/shell/log.rs:196-198`): *"a malformed line inside
the range is skipped rather than fatal."* The turn-log reader does the same — a
truncated final record (a crash mid-write) must not make the whole log
unreadable.

**F6 — `#[serde(default)]` is the forward-compat idiom** already used for
exactly this purpose in `src/session_store.rs:21-35`. Every field added to a
record after its first version carries it, so an old log still deserialises.

**F7 — tests that write under `HOME` take the guard.** The project's rule, and
the RAII shape to copy verbatim (`src/daemon/session.rs:852-857`):

```rust
        let _lock = crate::test_home_guard();
        let tmp = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("HOME", tmp.path()) };

        let sessions_dir = crate::config::sessions_dir();
        std::fs::create_dir_all(&sessions_dir).unwrap();
```

Bind the guard to a **named** variable (`_lock`, not bare `_`, which drops it
immediately). Edition 2024 requires the `unsafe` around `set_var`.

## Spec

### Task 1 — `src/config/load.rs`: the path constructor

Add `pub fn runs_dir() -> PathBuf` returning `var_log_dir().join("runs")`,
mirroring `sessions_dir()` (`load.rs:107`). Re-export it the way the other
constructors are re-exported from `src/config/mod.rs`.

### Task 2 — `src/config/seeds.rs`: create it eagerly

One `std::fs::create_dir_all(runs_dir())?;` line in `ensure_dirs()` beside
`sessions_dir()` (F2). Required by F1's second gate.

### Task 3 — `src/config/lifecycle.rs`: the policy row

```rust
    LifecycleEntry {
        path: "var/log/runs",
        intent: LifecycleIntent::Sweep { default_retention_days: 0 },
        config_key: None,
        implemented: ImplementationStatus::Pending { owned_by: "M24 phase-10" },
        note: "per-run turn logs (var/log/runs/<run-id>.jsonl); retention needs a \
               [runs] config key, which lands with the run CLI in phase-10",
        lazy: false,
    },
```

Adjust wording as you like; the **shape** is what matters — `Pending`,
`config_key: None`, not `lazy` (F3). **Do not** add a config key and **do not**
touch `assets/etc/config.toml`.

### Task 4 — `src/daemon/run/`: the record, writer and reader

The record envelope carries, at minimum: a timestamp, the `RunId`, and an
`event_type`-tagged payload. Serde-rename each variant explicitly, the way
phase-01's `RunKind`/`RunState` do, so a wire name can never drift with a
rename.

**Four starter event variants** — the run lifecycle, and nothing speculative:

- `RunStarted { kind: RunKind }`
- `TurnStarted { turn: u32 }`
- `Note { text: String }` — a free-form breadcrumb
- `RunFinished { state: RunState }`

Later phases add variants as they need them (tool calls, model responses);
**every field added after this phase carries `#[serde(default)]`** (F6).

Then:

- a **writer** that appends one record per line to `runs_dir().join(format!("{id}.jsonl"))`,
  flushing per record and warning rather than panicking on failure (F4);
- a **reader** that returns the records for a run id in order, **skipping a
  malformed line rather than failing the whole read** (F5), and returning an
  empty result for a run with no log.

**No caller.** Nothing in the daemon writes a turn log in this phase.

## Acceptance criteria

Each was run against the current tree and returns the "now" value shown.

- [ ] `grep -c 'pub fn runs_dir' src/config/load.rs` → **1** (now `0`).
- [ ] `grep -c 'runs_dir()' src/config/seeds.rs` → **1 or more** (now `0`).
- [ ] `grep -c '"var/log/runs"' src/config/lifecycle.rs` → **1** (now `0`), and
      that row is `Pending`:
      `grep -c 'M24 phase-10' src/config/lifecycle.rs` → **1** (now `0`).
- [ ] **The lifecycle gates still pass** — this is F1's whole point:
      `cargo test --lib config::lifecycle::` reports **5** passing, `0 failed`
      (now `5`). *(Preservation criterion: the count must not change; the
      existing gates must simply keep passing with the new row and directory.)*
- [ ] **Forward-compat and wire names are explicit:**
      `cat src/daemon/run/*.rs | grep -c 'serde(rename'` → **18 or more**.
      *Arithmetic, counted from the tree not estimated: **14** today
      (`RunKind` 4 + `RunState` 5 + `BudgetDimension` 5) + one per new event
      variant × 4 = **18**. State the number you got.*
- [ ] **The reader tolerates a malformed line** — asserted by a test, not just
      claimed (see Test plan).
- [ ] No `unsafe`/`unwrap`/`expect`/`panic!` outside test code in the run module:
      `for f in src/daemon/run/*.rs; do awk '/^#\[cfg\(test\)\]/{exit} {print}' $f; done | grep -cE '\.(unwrap|expect)\(|panic!\(|unsafe'`
      → **0** (now `0`, preservation).
- [ ] `cargo test --lib daemon::run::` reports **17 or more** passing,
      `0 failed` (now `10`). *Arithmetic: 10 existing + the 7 named tests in the
      Test plan = 17.*
- [ ] Unchanged: `daemon::session::` **21**, and the full lib suite `0 failed`
      on **three consecutive runs**.
- [ ] **No caller was added.** Against the pre-phase baseline commit
      `736a9a3` (not `HEAD`, which moves as you commit):
      `git diff --name-only 736a9a3 -- src/ | grep -vE '^src/(daemon/run/|config/(load|seeds|lifecycle|runtime_tree|mod)\.rs$)' | wc -l`
      → **0**. *(Corrected at review 2026-09-09 to include `runtime_tree.rs`,
      which F1's fourth gate forces. As originally written this printed `1`;
      the executor pasted that honestly and explained it in prose, which is the
      correct handling.)*
- [ ] All four gates pass.

## Test plan

Seven tests, names pinned; placement is yours. **Every test that writes a log
takes `crate::test_home_guard()` bound to a named variable and points `HOME` at
a `tempfile::tempdir()`** (F7) — the turn log is under `HOME`, so an unguarded
test would write into the developer's real `~/.daemoneye/`.

- `turn_log_writes_one_json_line_per_record` — write three records, read the
  file back raw, assert exactly three lines and that each parses as JSON.
- `turn_log_round_trips_all_four_event_variants` — write `RunStarted`,
  `TurnStarted`, `Note`, `RunFinished`; read them back and assert each comes
  back as the same variant with the same payload (a `RunKind`/`RunState`
  survives the round trip).
- `turn_log_reader_skips_a_malformed_line` — **the F5 negative.** Write two
  good records, append a line of garbage (`not json at all`), append a third
  good record; assert the reader returns **3** records and does not error. A
  reader that returns an `Err`, or 2, or 4, fails this.
- `turn_log_reader_returns_empty_for_an_unknown_run` — no file → empty, not an
  error.
- `turn_log_appends_rather_than_truncating` — write, then write again with a
  fresh writer for the same run id; assert both records are present. Guards the
  `.append(true)` in F4 (a `create(true).write(true)` would truncate and this
  test would see 1 record).
- `runs_dir_is_under_var_log` — `runs_dir()` ends with `var/log/runs` and sits
  beneath `var_log_dir()`.
- `ensure_dirs_creates_the_runs_dir` — under a temp `HOME`, `ensure_dirs()`
  leaves `runs_dir()` existing. This is the same property F1's lifecycle gate
  enforces, asserted directly so a failure names the cause.

**Mutation-check two of these before reporting**, and paste the red and green
`test result:` lines for each:

1. Change the writer's `.append(true)` to `.truncate(true)` →
   `turn_log_appends_rather_than_truncating` must go red.
2. Make the reader propagate a parse error instead of skipping →
   `turn_log_reader_skips_a_malformed_line` must go red.

Both are verified independently at review; a claimed mutation check is not one.

## End-to-end verification

Run this block verbatim from the repo root. It writes `/tmp/e2e-m24-02a.txt`.

```sh
{
echo "== A. build =="; cargo build 2>&1 | tail -2; echo "cargo_exit=${PIPESTATUS[0]}"
echo "== B. named tests (each line is one pinned test) =="
cargo test --lib 2>&1 | grep -E "^test daemon::run::.*(turn_log_|runs_dir_|ensure_dirs_).* ok$" | sed 's/^test //' | sort
echo "cargo_exit=${PIPESTATUS[0]}"
echo "== C. run module total (>=17) =="
cargo test --lib daemon::run:: 2>&1 | grep -E "^test result:"; echo "cargo_exit=${PIPESTATUS[0]}"
echo "== D. the lifecycle gates still pass (5) + session unchanged (21) =="
for m in config::lifecycle daemon::session; do
  printf '%-22s ' "$m"; cargo test --lib $m:: 2>&1 | grep -E "^test result:"; echo "cargo_exit=${PIPESTATUS[0]}"
done
echo "== E. lib suite, three consecutive runs =="
for r in 1 2 3; do cargo test --lib 2>&1 | grep -E "^test result:" | head -1; done
echo "== F. structural greps (each must print the stated number) =="
echo -n "runs_dir constructor       (1): "; grep -c 'pub fn runs_dir' src/config/load.rs
echo -n "created by ensure_dirs   (>=1): "; grep -c 'runs_dir()' src/config/seeds.rs
echo -n "lifecycle row              (1): "; grep -c '"var/log/runs"' src/config/lifecycle.rs
echo -n "row is Pending             (1): "; grep -c 'M24 phase-10' src/config/lifecycle.rs
echo -n "serde renames           (>=18): "; cat src/daemon/run/*.rs | grep -c 'serde(rename'
echo -n "no unwrap/panic pre-test   (0): "
for f in src/daemon/run/*.rs; do awk '/^#\[cfg\(test\)\]/{exit} {print}' $f; done | grep -cE '\.(unwrap|expect)\(|panic!\(|unsafe'
echo "== G. no caller was added (this phase wires nothing) =="
echo -n "files touched outside run/ + the three config files (0): "
git diff --name-only 736a9a3 -- src/ | grep -vE '^src/(daemon/run/|config/(load|seeds|lifecycle|runtime_tree|mod)\.rs$)' | wc -l
echo "== H. nothing leaked into the real HOME =="
echo -n "runs dir in the developer HOME (0 or pre-existing): "
ls -d "$HOME/.daemoneye/var/log/runs" 2>/dev/null | wc -l
} > /tmp/e2e-m24-02a.txt 2>&1
cat /tmp/e2e-m24-02a.txt
```

Paste the contents of `/tmp/e2e-m24-02a.txt` into your Update Log entry as a
fenced block. **The entry's header must be exactly this line**, or the
self-check below cannot find it:

```
### Update — YYYY-MM-DD HH:MM (end-to-end verification)
```

Then run the self-check and paste its verdict line into the same entry:

```sh
D=docs/dev/milestones/M24-run-manager/phase-02a-turn-log.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-m24-02a.txt
diff /tmp/pasted-m24-02a.txt /tmp/e2e-m24-02a.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

**Paste the block exactly as it came out, including any section that fails.**
`PASTE MATCH` is not optional; commentary belongs in prose **outside** the
fenced block.

**Your prose about section F must state each grep's value as it printed.**

**Section H is a leak check, and it matters here.** The turn log lives under
`HOME`; a test that forgets `test_home_guard()` writes into the developer's real
`~/.daemoneye/var/log/runs/`. If that directory did not exist before your run
and exists after, say so — a `1` is only acceptable if it was already there.

## Authorizations

You may edit, and nothing else:

- `src/config/load.rs` — the `runs_dir()` constructor (Task 1)
- `src/config/mod.rs` — **only** to re-export `runs_dir` alongside the other
  path constructors
- `src/config/seeds.rs` — **only** the one `create_dir_all(runs_dir())?;` line
  in `ensure_dirs()` (Task 2)
- `src/config/lifecycle.rs` — **only** the one new `LifecycleEntry` (Task 3).
  Do not change any existing row.
- `src/config/runtime_tree.rs` and
  `assets/memory/knowledge/agent-runtime-layout.md` — **only** the one `runs/`
  leaf each, which the policy row forces (F1's fourth and fifth gates).
  *(Added at review 2026-09-09: the original Authorizations omitted these, and
  the executor was correct to make the edit and flag it.)*
- `src/daemon/run/` — the record, writer and reader (Task 4)
- this phase doc's Update Log

**Do not** add a config key, touch `assets/etc/config.toml`, or change
`LimitsConfig` — retention is phase-10's (F3). **Do not** add an AI tool: the
`run_log_*` / `get_turn` tools are **phase 02b**, and adding one here would drag
in the 11-step tool checklist and the `doc_truth` table gates.

**Do not add a dependency.** `serde`, `serde_json`, `chrono` and `tempfile` are
already available.

**Do not run `pkill`, `killall`, `tmux kill-server`, or any signal to a process
group.** Use `find <dir> -depth -delete` for cleanup, never `rm -rf`.

## Out of scope

- **The `run_log_search` / `run_log_tail` / `get_turn` tools** — phase 02b.
  They read what this phase writes.
- **Retention/sweeping of `var/log/runs`** — the lifecycle row is deliberately
  `Pending` (F3); the config key and sweep land in phase-10.
- **Any wiring.** Nothing constructs a run or writes a log yet; phases 05–08 do.
  Section G enforces it.
- **Tool-call / model-response event variants** — later phases add them as they
  need them, under the `#[serde(default)]` rule (F6).
- **Indexing turn logs into the knowledge index** — design § 2.3 mentions it;
  no phase owns it yet.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-09-09 20:01 (started)

Executor: gemini-cli-3. Baseline confirmed: tree clean on `master` at `736a9a3`
family (`bfb83cc`); `cargo test --lib` → 1676 passed; 0 failed; 4 ignored;
`daemon::run::` → 10 passed. Statuses flipped to `in-progress` here and in the
milestone README. Proceeding with Tasks 1–4 in spec order; no wiring this phase.

### Update — 2026-09-09 20:25 (blocker, resolved by the cheapest reading)

**Blocked on (and self-resolved):** adding the `var/log/runs` lifecycle row
(Task 3) turned `config::runtime_tree::tests::every_policy_path_appears_in_tree`
red — the tree in `src/config/runtime_tree.rs` and its shipped twin
`assets/memory/knowledge/agent-runtime-layout.md` had no `runs/` leaf, and that
gate asserts *every* policy path appears in the tree. F1's list of three
directions (policy → constructor → eager creation) is complete but not
exhaustive: the runtime-tree gate is a fourth direction that
`POLICY_TABLE` feeds.

**What I tried:** first confirming no other test moves (the lifecycle gates
still report 5 passed). Then re-reading the phase doc: the Authorizations list
does not name `runtime_tree.rs`/`agent-runtime-layout.md`, but Section G's
"no caller was added" check pins the diff to `src/(daemon/run/|config/(load|seeds|lifecycle)\.rs|config/mod\.rs)`
— and my edit also had to keep the **acceptance criterion** "the full lib
suite `0 failed` on three consecutive runs" true, which the un-updated tree
breaks on the very next run.

**What I did (decision, for the reviewer to ratify):** I added one leaf
`runs/` to `RUNTIME_TREE` in `src/config/runtime_tree.rs` and the matching
line to the shipped `agent-runtime-layout.md` asset — the same one-line,
two-sites shape every other directory has, no behavior change, no API change.
This is a minimal, mechanical consequence of policy-driven tree generation,
not a design decision. Flagging it here explicitly rather than burying it: if
you consider the asset out of bounds, revert the two-site leaf and instead the
tree gate must special-case the pending row — which is worse.

### Update — 2026-09-09 20:33 (end-to-end verification)

Ran the phase doc's E2E block verbatim from the repo root; full transcript as
it printed (redirected to /tmp/e2e-m24-02a.txt and pasted back unmodified):

````
== A. build ==
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.06s
cargo_exit=0
== B. named tests (each line is one pinned test) ==
daemon::run::turn_log::tests::ensure_dirs_creates_the_runs_dir ... ok
daemon::run::turn_log::tests::runs_dir_is_under_var_log ... ok
daemon::run::turn_log::tests::turn_log_appends_rather_than_truncating ... ok
daemon::run::turn_log::tests::turn_log_reader_returns_empty_for_an_unknown_run ... ok
daemon::run::turn_log::tests::turn_log_reader_skips_a_malformed_line ... ok
daemon::run::turn_log::tests::turn_log_round_trips_all_four_event_variants ... ok
daemon::run::turn_log::tests::turn_log_writes_one_json_line_per_record ... ok
cargo_exit=0
== C. run module total (>=17) ==
test result: ok. 17 passed; 0 failed; 0 ignored; 0 measured; 1670 filtered out; finished in 0.00s
cargo_exit=0
== D. the lifecycle gates still pass (5) + session unchanged (21) ==
config::lifecycle      test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 1682 filtered out; finished in 0.00s
cargo_exit=0
daemon::session        test result: ok. 21 passed; 0 failed; 0 ignored; 0 measured; 1666 filtered out; finished in 0.00s
cargo_exit=0
== E. lib suite, three consecutive runs ==
test result: ok. 1683 passed; 0 failed; 4 ignored; 0 measured; 0 filtered out; finished in 7.01s
test result: ok. 1683 passed; 0 failed; 4 ignored; 0 measured; 0 filtered out; finished in 7.01s
test result: ok. 1683 passed; 0 failed; 4 ignored; 0 measured; 0 filtered out; finished in 7.09s
== F. structural greps (each must print the stated number) ==
runs_dir constructor       (1): 1
created by ensure_dirs   (>=1): 1
lifecycle row              (1): 1
row is Pending             (1): 1
serde renames           (>=18): 18
no unwrap/panic pre-test   (0): 0
== G. no caller was added (this phase wires nothing) ==
files touched outside run/ + the three config files (0): 1
== H. nothing leaked into the real HOME ==
runs dir in the developer HOME (0 or pre-existing): 0
````

Prose about section F, each value as it printed — `pub fn runs_dir` in
load.rs: **1**; `runs_dir()` in seeds.rs: **1**; `"var/log/runs"` in
lifecycle.rs: **1**; `M24 phase-10` in lifecycle.rs: **1**; `serde(rename`
across run modules: **18**; unwrap/panic/unsafe pre-test: **0**.

Section B printed `cargo_exit=101` on the first E2E run (the tree gate was
red then); it printed **0** on this final block. Section G prints **1**, not
**0**: the one extra file is `src/config/runtime_tree.rs` (plus its shipped
`assets/memory/knowledge/agent-runtime-layout.md` twin), carrying the one
`runs/` leaf the tree gate requires once the policy row exists — see the
earlier update in this log for why that edit is necessary and what exactly
changed.

Section H: **0** — `~/.daemoneye/var/log/runs` did not exist before this run
and still does not exist; every test wrote under a guarded tempdir `HOME`.

E2E block altered: the transcript above is from the corrected E2E run. The
literal E2E harness section G printed **1** where the spec's arrow says 0;
per the phase's own instruction I pasted the block without modification and
explain the one-extra-line here, in prose outside the fence.

### Update — 2026-09-09 20:40 (mutation checks)

Both required mutations were applied to the code, run, and reverted (each
verified red, then restored and verified green on the same command).

**Mutation 1 — writer `.append(true)` → `.truncate(true)`:**
`turn_log_appends_rather_than_truncating` went red:

```text
test daemon::run::turn_log::tests::turn_log_appends_rather_than_truncating ... FAILED
thread '…' panicked at src/daemon/run/turn_log.rs:237:65:
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 1686 filtered out; finished in 0.00s
```

Green again after restore: `test result: ok. 17 passed; 0 failed` on
`daemon::run::`.

**Mutation 2 — reader propagates a parse error (`.map(from_str(l).unwrap())`)
instead of skipping:**
`turn_log_reader_skips_a_malformed_line` went red:

```text
test daemon::run::turn_log::tests::turn_log_reader_skips_a_malformed_line ... FAILED
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 1686 filtered out; finished in 0.00s
```

Green again after restore: `test result: ok. 17 passed; 0 failed; 0 ignored`
on `daemon::run::`.

### Update — ts=1788985449114 (complete, server-authored)

**Summary:** </think>All four tasks are complete. Let me do a final sanity pass against the completion checklist:

- [x] Phase doc Status: in-progress (not flipped to review by me)
- [x] README phase-table row: in-progress
- [x] No hand-written (complete) entry — only started, blocker-explainer, E2E verification, and mutation-check entries
- [x] cargo fmt --all was actually run before git add
- [x] All four gates pass: build clean, clippy clean, fmt check clean, 3× full lib suite 1683 passed / 0 failed
- [x] Grep for spec-pinned literals: all structural greps printed expected values (runs_dir=1, seeds=1, lifecycle row=1, M24 phase-10=1, serde renames=18, no-unwrap=0)
- [x] Mutation checks both verified red then green
- [x] E2E transcript pasted verbatim, PASTE MATCH confirmed
- [x] git status clean, two commits, all files included
- [x] No caller added outside the authorized files except runtime_tree.rs which was forced by the tree gate and is explicitly documented in the Update Log
- [x] Section H leak check: 0

Now write the final Summary + Notes for review.</think>
**Summary**

Implemented M24 phase-02a: the per-run turn log at `var/log/runs/<run-id>.jsonl`. `src/daemon/run/turn_log.rs` (~300 lines) carries the `TurnLogRecord` envelope (timestamp, `RunId`, `TurnLogEvent` payload tagged on `event_type` with an explicit `serde(rename)` per variant — 18 renames counted across the run module), a writer that appends one JSON line per record and flushes per record (warns, never panics, on any I/O failure), and a reader that skips a malformed line rather than failing the read and returns empty for a run with no log. Four starter variants: `RunStarted`/`TurnStarted`/`Note`/`RunFinished`; no caller was added. F1's wiring landed in config: `runs_dir()` in `load.rs`, one `create_dir_all(runs_dir())` in `seeds.rs::ensure_dirs()`, and a `Pending { owned_by: "M24 phase-10" }`, `config_key: None`, non-lazy `LifecycleEntry` in `lifecycle.rs`. All four gates pass: build and clippy `-D warnings` clean, fmt clean, and the full lib suite is `1683 passed; 0 failed` on three consecutive runs (was 1676 + 4 ignored, +7 pinned tests). Mutation checks were executed and are evidenced in the Update Log: `.append→.truncate` turned `turn_log_appends_rather_than_truncating` red, and error-propagating parsing turned `turn_log_reader_skips_a_malformed_line` red; both were restored to green. The E2E transcript is pasted verbatim (`PASTE MATCH` confirmed by the doc's self-check), and section H is **0** — the developer HOME never gained a `var/log/runs`.

**Notes for review**

- **One authorized-file list deviation, flagged in the Update Log before commit:** adding the policy row turned `config::runtime_tree::every_policy_path_appears_in_tree` red — the runtime-tree gate validates every `POLICY_TABLE` path against `RUNTIME_TREE`, which the doc's F1 three-direction list doesn't name. I added the single `runs/` leaf to `RUNTIME_TREE` and the matching line to the shipped `assets/memory/knowledge/agent-runtime-layout.md` twin (no behavior change; the minimal mechanical consequence of policy-driven tree generation). Phase doc section G consequently prints **1**, not 0, and that is explained in prose outside the pasted fence — everything else in the block matches the spec's stated values.
- Every event field this phase carries is already in the envelope's baseline, so the doc's forward-compat rule ("fields added *after* this phase carry `#[serde(default)]`") is a contract for later phases, and the module doc comment states it; nothing to `default` yet.
- Timestamps in records are caller-supplied (no clock inside the type) so tests stay deterministic; the writer/reader API is sync like its `session.rs` analogue, which is what phases 05–08 will call from the blocking side.
- Two governance notes for the run loop: the E2E self-check found its entry at line 402, and the phase doc's own template slot (`### Update — YYYY-MM-DD HH:MM (end-to-end verification)`) is left as the placeholder, with the real dated entry a few lines below — that keeps the template text intact for future phases.

**Executor:** local-inference-lab/GLM-5.3-Flash-NVFP4-Spark

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.08s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.07s


TEST
nning 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 6 tests
test header_status_strips_trailing_prose ... ok
test header_status_reads_bare_word ... ok
test header_status_uses_first_occurrence_only ... ok
test open_bug_on_in_progress_phase_is_clean ... ok
test open_bug_on_done_phase_is_a_finding ... ok
test repository_bug_tracker_is_consistent ... ok

test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 10 tests
test approval_gated_tools_all_exist ... ok
test claude_md_tools_table_counts_are_accurate ... ok
test readme_tools_counts_are_accurate ... ok
test claude_md_tools_table_matches_the_code ... ok
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
test g3_tool_policy_allow_merged_and_enforced ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g4_briefing_injection_block_format ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g5_depth_limit_enforced ... ok
test g5_child_inherits_depth_and_parent ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ipc_tool_call_response_round_trip ... ok
test ipc_ask_round_trip ... ok
test ipc_session_info_round_trip ... ok
test ghost_config_parsing ... ok
test window_switch_does_not_corrupt_chat ... ignored
test minimal_config_parsing ... ok
test schedule_store_persistence ... ok
test config_pricing_round_trip ... ok
test g4_briefing_masking_applied ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test event_log_entry_format ... ok
test event_log_append_read ... ok
test g4_briefing_injects_on_next_run ... ok
test g4_briefing_read_and_clear ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test session_jsonl_round_trip ... ok
test session_index_persistence ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_below_threshold_discarded ... ok
test webhook_alert_to_event_log ... ok
test webhook_alert_unrankable_severity_passes_gate ... ok
test webhook_alert_no_severity_passes_gate ... ok

test result: ok. 31 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.04s


running 10 tests
test webhook_ghost_e2e_http ... ignored
test held_port_cannot_be_rebound ... ok
test webhook_ports_differ_between_environments ... ok
test stub_returns_canned_response_via_make_client ... ok
test webhook_ghost_e2e_deterministic ... ok
test config_contains_webhook_and_stub_url ... ok
test hooks_land_on_private_server ... ok
test daemon_boots_in_throwaway_root ... ok
test default_server_unchanged ... ok
test daemon_webhook_returns_200 ... ok

test result: ok. 9 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.16s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

   Compiling daemoneye v1.0.0 (/home/matt/src/daemoneye)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 6.94s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-9810e08c3de1e001)
     Running unittests src/main.rs (target/debug/deps/daemoneye-54ad57cf3704bc40)
     Running tests/bug_tracker.rs (target/debug/deps/bug_tracker-d709c9b862b5691d)
     Running tests/doc_truth.rs (target/debug/deps/doc_truth-34cb94d3012a3d69)
     Running tests/integration.rs (target/debug/deps/integration-deb70358b520d0d5)
     Running tests/isolation.rs (target/debug/deps/isolation-45facff22bac60f5)
   Doc-tests daemoneye

```

**Files changed:**

- `docs/dev/milestones/M24-run-manager/README.md` — +1 -1
- `docs/dev/milestones/M24-run-manager/phase-02a-turn-log.md` — +129 -1
- `src/config/lifecycle.rs` — +13 -0
- `src/config/load.rs` — +6 -0
- `src/config/seeds.rs` — +1 -0
- `src/daemon/run/turn_log.rs` — +298 -0

**Commit:** b7f9124293f6158f8dbe5722d6c8b3f97bab6ef0

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

### Review verdict — 2026-09-09

- **Verdict:** approved_first_try
- **Bounces:** none
- **Executor:** local-inference-lab/GLM-5.3-Flash-NVFP4-Spark
- **Scope deviations:** two files outside the Authorizations list —
  `src/config/runtime_tree.rs` and its shipped twin
  `assets/memory/knowledge/agent-runtime-layout.md`, one `runs/` leaf each.
  **Both were forced, and the deviation is mine, not the executor's.**
  Verified at review by deleting the leaf: `every_policy_path_appears_in_tree`
  **and** `render_matches_shipped_asset` go red together (3 passed / 2 failed),
  green when restored. The executor hit the gate, wrote a blocker entry naming
  what it tried and why the alternative was worse, made the minimal two-site
  edit, pasted the E2E block **unmodified** showing section G at `1` rather
  than the pinned `0`, and explained it in prose outside the fence. That is
  exactly the handling the phase doc asks for.
- **Calibration — my F1 was incomplete.** It named **three** gates on a new
  artifact directory; there are **five**. The two I missed both live in
  `src/config/runtime_tree.rs`: a `POLICY_TABLE` path needs a `RUNTIME_TREE`
  leaf, and `RUNTIME_TREE` must still render to the shipped runtime-layout
  asset. F1, the Authorizations and the section-G pattern are corrected above.
  **This matters beyond this phase**: any future phase adding a directory under
  `~/.daemoneye/` faces all five, and M24 phase-10 will when it adds the
  retention config key. Recorded in the milestone README so it is not
  rediscovered a third time.

**Verified at review, independently re-run:** all four gates green; full suite
**1683 passed / 0 failed / 4 ignored on three consecutive runs**;
`daemon::run::` **17**; `config::lifecycle::` **5** and `daemon::session::`
**21** unchanged. Structural, each as it printed: `runs_dir` **1**,
`ensure_dirs` **1**, lifecycle row **1**, `M24 phase-10` **1**,
`serde(rename` **18** (exactly the re-pinned target), pre-test
unwrap/expect/panic/unsafe **0**, no TODO/dbg/allow. **Section H is `0`** — the
turn log never touched the developer's real `~/.daemoneye/var/log/runs`, which
is the check that matters most for a phase whose artifact lives under `HOME`.

**Both mutations re-run by the reviewer, not read from the log:**

1. Writer `.append(true)` → `.truncate(true)`:
   `turn_log_appends_rather_than_truncating` goes red (with three collateral
   failures, since truncation breaks every multi-record test — expected).
2. Reader propagating the parse error instead of skipping: **only**
   `turn_log_reader_skips_a_malformed_line` goes red (6 passed / 1 failed) —
   precisely targeted, which is what makes it a real guard for the
   crash-truncated-final-record case.

Both green on restore (17 passed).

**One observation on the `PhaseResult` payload, not on the work.** Its
`files_changed` list omitted `src/daemon/run/mod.rs`, `src/config/runtime_tree.rs`
and the asset — three files that are genuinely in the commits. The tree itself
is coherent (`git diff --name-only` against the baseline shows all ten), so this
is a reporting gap in the harness rather than anything wrong with the phase.
Worth knowing when reading a `PhaseResult`: **verify the diff against the
baseline rather than trusting `files_changed`.**
