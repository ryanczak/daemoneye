# Phase 01: the `Run` core types

**Milestone:** M24 — Run Manager
**Status:** in-progress
**Depends on:** none (this is the milestone's first phase; pure types, no wiring)
**Estimated diff:** ~450 lines incl. tests
**Tags:** language=rust, kind=feature, size=m

## Goal

Create `src/daemon/run/` and the pure data types the rest of M24 is built from:
`RunId`, `RunKind`, `RunState`, `Budget`, `Briefing`. **No wiring, no I/O, no
callers.** Nothing in the daemon constructs a `Run` yet — later phases do that.

Everything here is pure and unit-testable: an id grammar, four kind variants, a
state machine's terminal predicate, budget arithmetic, and a briefing's bounds.

## Architecture references

- `docs/design/daemoneye-2.0.md` § 2.3 "Run manager" — the `Run` struct this
  phase's types come from, and the rexyMCP anchors later phases port from.
- `docs/dev/milestones/M24-run-manager/README.md` § Exit criteria — phase 05
  (background) and 06–08 (chat/ghost/scheduled wiring) all consume these types,
  so getting the shape right here avoids a breaking change three phases later.

## Pre-flight

- `git rev-parse --abbrev-ref HEAD` → `master`, tree clean.
- `cargo test --lib` → **1666 passed; 0 failed; 4 ignored** before you start.
- `cargo test --lib daemon::run::` → **0 passed** (the module does not exist).

## Current state

`src/daemon/run/` **does not exist**; `ls -d src/daemon/run` fails and
`cargo test --lib daemon::run::` matches **0** tests. Full lib suite: **1666
passed; 0 failed; 4 ignored**.

`src/daemon/mod.rs` declares its submodules as a `pub mod <name>;` list
(`mod.rs:27-46`); this phase adds one line to it.

## Measured facts — executed 2026-09-09, not reasoned about

**F1 — the id convention this project already uses for a log filename.** A run's
turn log will be `var/log/runs/<run-id>.jsonl` — the same shape as the session
log `var/log/sessions/<id>.jsonl`, whose id comes from
`src/cli/commands/ipc_client.rs:13`:

```rust
pub(super) fn new_session_id() -> String {
    let mut bytes = [0u8; 8];
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        use std::io::Read;
        if f.read_exact(&mut bytes).is_ok() {
            return bytes.iter().map(|b| format!("{:02x}", b)).collect();
        }
```

Eight random bytes → **16 lowercase hex characters** (a real one observed live:
`2190b361c94e7a45.jsonl`). `RunId` uses the **same grammar**, so a run log and a
session log look alike on disk and a user can type either.

Two other id conventions exist and are **not** the model here — cite neither:
`src/daemon/ghost.rs:185` uses `format!("ghost-{}-{}", alert, Uuid::new_v4().simple())`
(32 hex, prefixed) and `src/shell/pty.rs:30` uses `Uuid::new_v4().simple()` for
the marker nonce. `uuid` **is** already a dependency (`Cargo.toml:42`), so
generating from it is fine — the pinned thing is the **grammar**, not the source.

**F2 — the newtype + validated-grammar idiom to copy.** `ShellId`
(`src/shell/registry.rs:16-40`) is the closest analogue in the tree, built last
milestone:

```rust
#[derive(Clone, Debug)]
pub struct ShellId(String);

impl ShellId {
    pub fn parse(s: &str) -> anyhow::Result<ShellId> {
        super::launch::validate_shell_id(s)?;
        Ok(ShellId(s.to_string()))
    }
    pub fn as_str(&self) -> &str { &self.0 }
}
```

and its grammar is a **hard validator that bails**, not a lenient parse
(`src/shell/launch.rs`):

```rust
pub fn validate_shell_id(id: &str) -> Result<()> {
    let Some(digits) = id.strip_prefix('s') else {
        bail!("Invalid shell id: '{id}'");
    };
    if digits.is_empty() || digits.len() > 9 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        bail!("Invalid shell id: '{id}'");
    }
    Ok(())
}
```

Do the same shape for `RunId`. **This is a security boundary, not a
convenience**: the id becomes a path segment, so `parse` must reject anything
containing `/`, `\`, or `..`, exactly as the shell-id validator rejects
everything outside its grammar.

**F3 — the serde-enum idiom.** `ShellState` (`src/shell/proto.rs`) spells each
rename explicitly so the wire form can never drift with a rename:

```rust
pub enum ShellState {
    #[serde(rename = "Idle")]    Idle,
    #[serde(rename = "Running")] Running,
    #[serde(rename = "Paused")]  Paused,
    #[serde(rename = "Exited")]  Exited,
}
```

`RunKind` and `RunState` are serialised into the turn log (phase 02), so use
this shape.

**F4 — `0 = unlimited` is this project's existing budget idiom, and `Budget`
must not duplicate `LimitsConfig`.** `LimitsConfig`
(`src/config/types.rs:732+`) already holds the *configured* caps —
`per_tool_batch`, `total_tool_calls_per_turn`, `tool_result_chars`, `max_turns`,
`max_tool_calls_per_session`, `per_tool`, `turn_timeout_secs` — several
documented "Set to 0 for no cap." `Budget` is **runtime consumption tracked
against limits**, not a second config struct: it holds limits *and* what has
been spent, and answers "am I out?". Keep the `0 = unlimited` convention.

## Spec

Create `src/daemon/run/` and add `pub mod run;` to `src/daemon/mod.rs`'s
submodule list (`mod.rs:27-46`, alphabetical).

**Module layout is yours** — one file or several under `src/daemon/run/`. But
**`mod.rs` must re-export the public names** (`pub use …`) rather than making
submodules `pub mod`: bug-07c-1 in the last milestone bounced a phase for making
ten modules public when the re-export block was the right tool. Callers must
reach these as `crate::daemon::run::RunId`, not `crate::daemon::run::id::RunId`.

### Task 1 — `RunId`

A newtype over `String`, same shape as `ShellId` (F2):

- `RunId::new()` — generate a fresh id.
- `RunId::parse(s: &str) -> anyhow::Result<RunId>` — validate against the
  grammar and `bail!` otherwise.
- `RunId::as_str(&self) -> &str`.

**Grammar: exactly 16 lowercase hex characters** (F1). Nothing else is a valid
`RunId`. `parse` must reject — these are the pinned negative cases:

| Input | Why it must fail |
|---|---|
| `../../etc/passwd` | path traversal — the id becomes a filename |
| `a/b` | path separator |
| `2190B361C94E7A45` | uppercase (grammar is lowercase) |
| `2190b361c94e7a4` (15) / `…a456` (17) | wrong length |
| `` (empty) | empty |
| `2190b361c94e7a4z` | non-hex character |

### Task 2 — `RunKind`

```rust
pub enum RunKind {
    Chat { session: String },
    Ghost { runbook: String, trigger: String },
    Scheduled { job: String },
    Delegated { agent: String, parent: RunId },
}
```

Serde-renamed per F3, plus `fn label(&self) -> &'static str` returning
`"chat" | "ghost" | "scheduled" | "delegated"` for logs and the future
`daemoneye runs` listing.

### Task 3 — `RunState`

```rust
pub enum RunState {
    Running,
    Complete,
    HardFail(Briefing),
    BudgetExceeded(Briefing),
    Cancelled { stage: String, turns: u32 },
}
```

Plus `fn is_terminal(&self) -> bool` — `false` only for `Running`. Serde-renamed
per F3.

`Briefing` is defined here (Task 5) so this enum is **final from the start**;
phase 03 builds briefings from live runs but does not change this shape.

### Task 4 — `Budget`

Runtime consumption against limits, in five dimensions from design § 2.3:
**turns, tool calls, tokens, wall clock, USD**. Shape is yours, but it must:

- hold both the **limits** and the **spent** amounts;
- treat **`0` as unlimited** in every dimension (F4);
- expose `fn exhausted(&self) -> Option<BudgetDimension>` naming the **first**
  dimension that is out (so the caller can say *which* budget ended the run),
  returning `None` when none is;
- expose a way to record spend and to read what remains, with `remaining`
  **saturating at zero** rather than underflowing.

`BudgetDimension` is a small enum (`Turns | ToolCalls | Tokens | WallClock |
Usd`) so the exhausted dimension is a value, not a string.

**Do not** read config here and **do not** touch `LimitsConfig` (F4) — a later
phase constructs a `Budget` from config.

### Task 5 — `Briefing`

A bounded, plain-data struct (design § 2.3): goal, criteria, working files,
attempts, blocker, budget left. The **bounds are the behavior**:

- **at most 5 working files** — extras dropped;
- **each attempt truncated to 200 characters**;

both enforced at construction, so an over-long briefing cannot be built. Keep
the other fields plain `String`/`Option<String>`.

**No I/O anywhere in this phase.** No file reads, no config, no clock beyond
whatever a `Budget` wall-clock dimension needs as a passed-in value —
`STANDARDS.md` § 3.3 requires deterministic tests, so **any elapsed time is a
parameter, never `Instant::now()` inside the type**.

## Acceptance criteria

Each was run against the current tree and returns the "now" value shown.

- [ ] `test -d src/daemon/run && echo yes` → **yes** (now: absent).
- [ ] `grep -c '^pub mod run;' src/daemon/mod.rs` → **1** (now `0`).
- [ ] **Re-exported, not `pub mod`** (bug-07c-1's lesson):
      `grep -c '^pub mod ' src/daemon/run/mod.rs` → **0**, and
      `grep -c 'pub use ' src/daemon/run/mod.rs` → **1 or more**. *(If you put
      everything in `mod.rs` itself with no submodules, both still hold: 0 and
      — if there is nothing to re-export — state that in the Update Log and the
      `pub use` criterion is waived. Say which shape you chose.)*
- [ ] `grep -c 'pub struct RunId' src/daemon/run/*.rs` → **1** (now: no such
      path), and the same for `pub enum RunKind`, `pub enum RunState`,
      `pub struct Briefing`, `pub enum BudgetDimension` → **1** each.
- [ ] **The id grammar is a hard validator:**
      `grep -c 'bail!' src/daemon/run/*.rs` → **1 or more** (now: no such path).
- [ ] **No I/O and no clock in these types:**
      `grep -cE 'Instant::now|SystemTime::now|std::fs::|File::open' src/daemon/run/*.rs`
      → **0**.
- [ ] No `unsafe`/`unwrap`/`expect`/`panic!` outside test code:
      `for f in src/daemon/run/*.rs; do awk '/^#\[cfg\(test\)\]/{exit} {print}' $f; done | grep -cE '\.(unwrap|expect)\(|panic!\(|unsafe'`
      → **0**.
- [ ] `cargo test --lib daemon::run::` reports **10 or more** passing,
      `0 failed` (now `0`). *Arithmetic: 0 existing + the 10 named tests in the
      Test plan = 10.*
- [ ] **Nothing else changed** — this phase has no callers. Against the
      **pre-phase baseline commit `3077587`** (not `HEAD`, which moves as you
      commit and would make this vacuous):
      `git diff --name-only 3077587 -- src/ | grep -vE '^src/daemon/(run/|mod\.rs$)' | wc -l`
      → **0**. *(Preservation criterion.)*
- [ ] Unchanged: `shell::registry::` **27**, `daemon::stats::` **7**, and the
      full lib suite `0 failed` on **three consecutive runs**.
- [ ] All four gates pass.

## Test plan

Ten tests, names pinned; placement is yours. All are pure — **no `tempfile`, no
`test_home_guard`, no sleeps**; nothing here touches the filesystem or a clock.

- `run_id_generates_a_valid_16_hex_id` — `RunId::new()` round-trips through
  `parse` and is 16 lowercase hex chars.
- `run_id_parse_rejects_path_traversal_and_separators` — **the security
  negative**: `../../etc/passwd`, `a/b`, and a backslash form all `Err`. This is
  why the grammar exists; the id becomes a path segment.
- `run_id_parse_rejects_wrong_length_case_and_non_hex` — the rest of the F-table:
  15 chars, 17 chars, uppercase, empty, and a `z`.
- `run_kind_labels_all_four_kinds` — `label()` for Chat/Ghost/Scheduled/
  Delegated returns the four expected strings.
- `run_state_terminal_predicate_separates_running_from_finished` — `Running` is
  not terminal; `Complete`, `HardFail`, `BudgetExceeded`, `Cancelled` all are.
- `budget_reports_the_first_exhausted_dimension` — with two dimensions over
  their limit, `exhausted()` names the **first** in the documented order (assert
  the specific dimension, not just `is_some()`).
- `budget_zero_limit_means_unlimited` — a dimension whose limit is `0` never
  reports exhausted no matter how much is spent (F4's idiom).
- `budget_remaining_saturates_at_zero` — overspending a dimension leaves
  `remaining` at `0`, not an underflow/panic.
- `briefing_truncates_working_files_to_five` — six files in, five out.
- `briefing_truncates_each_attempt_to_200_chars` — a 500-char attempt is stored
  at exactly 200.

**Mutation-check the two bounds before reporting** (they are the phase's real
behavior): remove the 5-file cap → `briefing_truncates_working_files_to_five`
must go red; remove the `0 = unlimited` branch →
`budget_zero_limit_means_unlimited` must go red. Restore and confirm green.
Paste both red and green `test result:` lines.

## End-to-end verification

Run this block verbatim from the repo root. It writes `/tmp/e2e-m24-01.txt`.

```sh
{
echo "== A. build =="; cargo build 2>&1 | tail -2; echo "cargo_exit=${PIPESTATUS[0]}"
echo "== B. named tests (each line is one pinned test) =="
cargo test --lib 2>&1 | grep -E "^test .*(run_id_|run_kind_|run_state_|budget_|briefing_).* ok$" | sed 's/^test //' | sort
echo "cargo_exit=${PIPESTATUS[0]}"
echo "== C. run module total (>=10) =="
cargo test --lib daemon::run:: 2>&1 | grep -E "^test result:"; echo "cargo_exit=${PIPESTATUS[0]}"
echo "== D. untouched (registry 27, stats 7) =="
for m in shell::registry daemon::stats; do
  printf '%-20s ' "$m"; cargo test --lib $m:: 2>&1 | grep -E "^test result:"; echo "cargo_exit=${PIPESTATUS[0]}"
done
echo "== E. lib suite, three consecutive runs =="
for r in 1 2 3; do cargo test --lib 2>&1 | grep -E "^test result:" | head -1; done
echo "== F. structural greps (each must print the stated number) =="
echo -n "run dir exists             (1): "; test -d src/daemon/run && echo 1 || echo 0
echo -n "declared in daemon/mod.rs  (1): "; grep -c '^pub mod run;' src/daemon/mod.rs
echo -n "no pub mod in run/mod.rs   (0): "; grep -c '^pub mod ' src/daemon/run/mod.rs
echo -n "RunId declared             (1): "; cat src/daemon/run/*.rs | grep -c 'pub struct RunId'
echo -n "RunKind declared           (1): "; cat src/daemon/run/*.rs | grep -c 'pub enum RunKind'
echo -n "RunState declared          (1): "; cat src/daemon/run/*.rs | grep -c 'pub enum RunState'
echo -n "Briefing declared          (1): "; cat src/daemon/run/*.rs | grep -c 'pub struct Briefing'
echo -n "BudgetDimension declared   (1): "; cat src/daemon/run/*.rs | grep -c 'pub enum BudgetDimension'
echo -n "grammar bails            (>=1): "; cat src/daemon/run/*.rs | grep -c 'bail!'
echo -n "no io/clock in types       (0): "; cat src/daemon/run/*.rs | grep -cE 'Instant::now|SystemTime::now|std::fs::|File::open'
echo -n "no unwrap/panic pre-test   (0): "
for f in src/daemon/run/*.rs; do awk '/^#\[cfg\(test\)\]/{exit} {print}' $f; done | grep -cE '\.(unwrap|expect)\(|panic!\(|unsafe'
echo "== G. no caller was added (this phase wires nothing) =="
echo -n "files touched outside daemon/run + daemon/mod.rs (0): "
git diff --name-only 3077587 -- src/ | grep -vE '^src/daemon/(run/|mod\.rs$)' | wc -l
} > /tmp/e2e-m24-01.txt 2>&1
cat /tmp/e2e-m24-01.txt
```

Paste the contents of `/tmp/e2e-m24-01.txt` into your Update Log entry as a
fenced block. **The entry's header must be exactly this line**, or the
self-check below cannot find it:

```
### Update — YYYY-MM-DD HH:MM (end-to-end verification)
```

Then run the self-check and paste its verdict line into the same entry:

```sh
D=docs/dev/milestones/M24-run-manager/phase-01-run-core.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-m24-01.txt
diff /tmp/pasted-m24-01.txt /tmp/e2e-m24-01.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

**Paste the block exactly as it came out, including any section that fails.**
`PASTE MATCH` is not optional and has no "by design" exemption: commentary
belongs in prose **outside** the fenced block.

**Your prose about section F must state each grep's value as it printed.** A
transcript that honestly prints a number under prose claiming a different one is
what bug-07b-1 was filed for in the last milestone.

**A `0` in section F can be an error, not a measurement.** Before the module
exists these greps print `No such file or directory`; after it exists every line
must carry a number on the *same* line as its label.

**Section G is the "no wiring" criterion.** This phase adds types nobody calls
yet. If it prints anything but `0`, you wired something — say so rather than
letting it pass.

## Authorizations

You may edit, and nothing else:

- **create** `src/daemon/run/` and its files
- `src/daemon/mod.rs` — **only** the one `pub mod run;` line in the submodule
  list at `mod.rs:27-46`
- this phase doc's Update Log

**Do not** touch `LimitsConfig` or any config file, `src/shell/`, the executor,
or any existing caller. Nothing in the daemon constructs a `Run` in this phase —
that is phases 05–08.

**Do not add a dependency.** `uuid` and `serde` are already available
(`Cargo.toml:42`); nothing else is needed.

**Do not run `pkill`, `killall`, `tmux kill-server`, or any signal to a process
group.** Use `find <dir> -depth -delete` for cleanup, never `rm -rf`.

## Out of scope

- **The `Run` struct itself** (the aggregate holding `shells`, `governor`,
  `cancel`, `turn_log`). This phase ships the *field types*; assembling them is
  phase 02+ once the turn log exists to hold.
- **Building a `Briefing` from a live run**, and `RunResult` — phase 03. This
  phase defines the briefing's *type and bounds*; 03 populates it and ports the
  rexyMCP construction logic.
- **The turn log** (`var/log/runs/<id>.jsonl`) — phase 02.
- **Constructing a `Budget` from `LimitsConfig`** — a later wiring phase.
- **The governor** — M25 entirely, and it needs `Category` on `TOOLS` from M22.
- **Any wiring**: chat, ghost, scheduled, delegated, and `background=true` all
  come later (phases 05–08). Section G enforces that here.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-09-09 19:10 (started)

**Executor:** headless run, phase-01 (M24 Run Manager).

Pre-flight confirmed: branch `master`, tree clean, `src/daemon/run/` absent,
full lib suite **1666 passed; 0 failed; 4 ignored**. Phase status flipped to
`in-progress` here and in the milestone README. Implementation begins: `RunId`,
`RunKind`, `RunState`, `Budget`/`BudgetDimension`, `Briefing` under
`src/daemon/run/`, re-exported from `mod.rs`, no wiring and no I/O (per spec §
Authorizations).

### Update — 2026-09-09 19:24 (implementation + mutation check)

**Module shape chosen:** everything in a single `src/daemon/run/mod.rs` — the
five types total ~300 lines and share the idiom family (F1–F4), so a second
file would only add a hop. Consequence for the acceptance criteria: `grep -c
'^pub mod ' src/daemon/run/mod.rs` is **0** and there is nothing to re-export,
so the `pub use` criterion is **waived** under the spec's parenthetical ("if
you put everything in mod.rs itself … say which shape you chose"). This entry
is that statement.

**What the types are:** `RunId` is a newtype over `String` validated by a hard
grammar — exactly 16 lowercase hex characters (same span a session id carries),
built from the low 64 bits of a `Uuid::new_v4()`, so `var/log/runs/<id>.jsonl`
and `var/log/sessions/<id>.jsonl` look alike on disk and a user can type
either. `parse` rejects the whole F-table pinned in the spec; it is a security
boundary because the id becomes a path segment. `RunKind` carries the four
design-2.3 variants with `label()` for the future `daemoneye runs` listing;
`RunState` is the design's five variants with `is_terminal()` false only for
`Running`; `Briefing` enforces its own bounds (≤5 working files, each attempt
≤200 chars) at construction via `new`, and the bounds are named constants;
`Budget` holds five limit/spent pairs with `0 = unlimited`, `record_*` for
spend, `remaining()` saturating at zero and `None`-when-unlimited, and
`exhausted()` naming the first spent-out dimension in the documented order
(`BUDGET_DIMENSION_ORDER`). Wall-clock is a parameter, never a clock read.

**Mutation checks (both bounds):** with the 5-file cap removed
(`take(MAX_BRIEFING_FILES + 1)`) `briefing_truncates_working_files_to_five`
went red: `test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured;
1679 filtered out; finished in 0.00s` — restored, green again. With the
`limit == 0` unlimited branch removed from `remaining()` immediately
`budget_zero_limit_means_unlimited` went red: `test result: FAILED. 0 passed;
1 failed; 0 ignored; 0 measured; 1679 filtered out; finished in 0.00s`
(assertion message `limit 0 must be unlimited for Turns, left: Some(0),
right: None`) — restored, green again. Full run-module result after
restoration: `test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured;
1670 filtered out; finished in 0.00s`.

**Gates:** `cargo fmt --all` clean (run before staging); `cargo build` — 0
warnings; `cargo clippy --all-targets --all-features -- -D warnings` — exit 0;
`cargo test` — lib `1676 passed; 0 failed; 4 ignored` (1666 pre-phase + the
10 new), plus the integration/bin/doc suites all green.

### Update — 2026-09-09 19:24 (end-to-end verification)

Ran the phase's E2E block verbatim from the repo root; `/tmp/e2e-m24-01.txt`
contents follow, then the self-check verdict.

```text
== A. build ==
   Compiling daemoneye v1.0.0 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 2.26s
cargo_exit=0
== B. named tests (each line is one pinned test) ==
ai::tests::select_timeout_uses_first_token_budget_before_first_token ... ok
cli::commands::chat::tests::turn_budget_label_new_session ... ok
cli::commands::chat::tests::turn_budget_label_reports_usage ... ok
cli::commands::stream::tests::silence_budget_phase1_is_90s ... ok
cli::commands::stream::tests::silence_budget_phase2_is_120s ... ok
daemon::briefing::tests::clear_briefing_is_noop_when_absent ... ok
daemon::briefing::tests::clear_briefing_removes_file ... ok
daemon::briefing::tests::read_briefing_returns_content_when_present ... ok
daemon::briefing::tests::read_briefing_returns_none_when_absent ... ok
daemon::digest::tests::budget_cut_keeps_min_tail ... ok
daemon::digest::tests::budget_cut_respects_target ... ok
daemon::run::tests::briefing_truncates_each_attempt_to_200_chars ... ok
daemon::run::tests::briefing_truncates_working_files_to_five ... ok
daemon::run::tests::budget_remaining_saturates_at_zero ... ok
daemon::run::tests::budget_reports_the_first_exhausted_dimension ... ok
daemon::run::tests::budget_zero_limit_means_unlimited ... ok
daemon::run::tests::run_id_generates_a_valid_16_hex_id ... ok
daemon::run::tests::run_id_parse_rejects_path_traversal_and_separators ... ok
daemon::run::tests::run_id_parse_rejects_wrong_length_case_and_non_hex ... ok
daemon::run::tests::run_kind_labels_all_four_kinds ... ok
daemon::run::tests::run_state_terminal_predicate_separates_running_from_finished ... ok
cargo_exit=0
== C. run module total (>=10) ==
test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 1670 filtered out; finished in 0.00s
cargo_exit=0
== D. untouched (registry 27, stats 7) ==
shell::registry      test result: ok. 27 passed; 0 failed; 0 ignored; 0 measured; 1653 filtered out; finished in 0.00s
cargo_exit=0
daemon::stats        test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 1673 filtered out; finished in 0.00s
cargo_exit=0
== E. lib suite, three consecutive runs ==
test result: ok. 1676 passed; 0 failed; 4 ignored; 0 measured; 0 filtered out; finished in 7.04s
test result: ok. 1676 passed; 0 failed; 4 ignored; 0 measured; 0 filtered out; finished in 6.97s
test result: ok. 1676 passed; 0 failed; 4 ignored; 0 measured; 0 filtered out; finished in 7.00s
== F. structural greps (each must print the stated number) ==
run dir exists             (1): 1
declared in daemon/mod.rs  (1): 1
no pub mod in run/mod.rs   (0): 0
RunId declared             (1): 1
RunKind declared           (1): 1
RunState declared          (1): 1
Briefing declared          (1): 1
BudgetDimension declared   (1): 1
grammar bails            (>=1): 1
no io/clock in types       (0): 0
no unwrap/panic pre-test   (0): 0
== G. no caller was added (this phase wires nothing) ==
files touched outside daemon/run + daemon/mod.rs (0): 0
```

Section F, value by value as printed: run dir exists **1** (expected 1);
declared in daemon/mod.rs **1** (1); no pub mod in run/mod.rs **0** (0 — all
types live in `mod.rs` itself, no submodules, so nothing to re-export and the
`pub use` line is waived per the spec's parenthetical); RunId **1**, RunKind
**1**, RunState **1**, Briefing **1**, BudgetDimension **1** (all 1);
`bail!` **1** (>=1 — the hard grammar validator in `RunId::parse`);
no-IO/clock **0** (0); pre-test unwrap/panic **0** (0). Every line carried a
number on the same line as its label (the module exists, so none of the
zeroes is a `No such file or directory` artifact).

Section G: **0** — no caller was added, per the phase's "no wiring" criterion.

The run-module total in C is 10, matching the test plan's arithmetic; the ten
named tests in B are each present with `... ok`.

Self-check verdict below (pasted from its own output).

```text
PASTE MATCH
```
