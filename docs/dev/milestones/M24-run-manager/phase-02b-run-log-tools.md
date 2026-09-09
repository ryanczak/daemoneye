# Phase 02b: the run-log tools

**Milestone:** M24 — Run Manager
**Status:** review
**Depends on:** phase-02a (`turn_log.rs` — `read_turn_log`, `TurnLogRecord`).
Done.
**Estimated diff:** ~400 lines incl. tests
**Tags:** language=rust, kind=feature, size=m

## Goal

Three read-only AI tools over the turn log phase-02a writes:
`run_log_tail`, `run_log_search`, `get_turn`. All **deferred** under a new
`runs` group, none approval-gated.

This phase is mostly **checklist**, not design: adding a tool touches nine
files, and two of the gates that enforce it are **not** in `CLAUDE.md`'s
add-a-tool checklist (F2, F3). The design is three small functions; the risk is
a missed touch point.

## Architecture references

- `docs/design/daemoneye-2.0.md` § 2.3, the turn-log row: *"`run_log_search /
  run_log_tail / get_turn` tools with clamped limits and one uncapped escape
  hatch"*.
- `CLAUDE.md` § "Adding a new AI tool (checklist)" — the eleven steps. Follow
  it, plus F2 and F3 below which it does not name.

## Pre-flight

- `git rev-parse --abbrev-ref HEAD` → `master`, tree clean.
- `cargo test --lib` → **1683 passed; 0 failed; 4 ignored**.
- `cargo test --lib daemon::run::` → **17 passed**.

## Current state

Full lib **1683 passed; 0 failed; 4 ignored**; `daemon::run` **17**.

**36 tools: 27 core + 9 deferred** (`CLAUDE.md:139`, `README.md:496`). Deferred
groups today: `agents` (4), `runbooks` (2), `scripts` (2), `memory` (1). There
is no `runs` group; `grep -c 'run_log_tail' src/` → **0**.

## Measured facts — executed 2026-09-09, not reasoned about

**F1 — the design's phrase, decoded.** *"clamped limits and one uncapped escape
hatch"* maps onto the three tools like this, and this is the pinned reading:

- `run_log_tail` and `run_log_search` take a `limit` that is **clamped** to a
  maximum the tool enforces (the model cannot raise it by asking for more).
- `get_turn` is the **uncapped escape hatch**: it returns every record for one
  specific turn, however many that is, because the caller has already narrowed
  to a single turn.

**F2 — a hidden gate the CLAUDE.md checklist does not mention.**
`src/ai/tools/dispatch.rs` has `dispatch_roundtrip_all_tools`, whose doc
comment reads:

```rust
    /// Every tool in TOOLS must have a dispatch arm that accepts conformant args.
    /// This catches the case where a tool is added to TOOLS but forgotten in
```

It walks **every** entry in `TOOLS` and looks the name up in a `minimal_args`
match:

```rust
        fn minimal_args(name: &str) -> Value {
                "run_terminal_command" => json!({"command": "echo hi"}),
                …
                "read_runbook" => json!({"name": "rb"}),
```

**Add a tool without adding its `minimal_args` arm and this test fails.** It is
step 12 of an 11-step checklist; do not miss it.

**F3 — the `load_tools` catalog is generated, so a new group needs no
registration.** `deferred_group: Some("runs")` is the whole registration:
`schema.rs:44-47` says a deferred tool is *"listed in that tool's generated
catalog under `group`"*, and `dispatch.rs`'s `load_tools_catalog_lists_all_groups`
asserts the catalog names every group. There is **no** hand-kept list of valid
group names to update.

**F4 — `sre.toml` is `include_str!`'d, so it is one site, not two.**
`src/config/seeds.rs:228`:

```rust
pub(crate) const SRE_PROMPT_TOML: &str = include_str!("../../assets/prompts/sre.toml");
```

Editing `assets/prompts/sre.toml` updates the constant automatically — unlike
the runtime-tree/asset pair that forced a two-site edit last phase. Deferred
tools are documented there inconsistently today (`read_runbook` yes,
`create_agent` no), so a short mention is welcome but **not** required; see
§ Out of scope.

**F5 — the doc tables are enforced, and the counts are three numbers in two
files.** `tests/doc_truth.rs` runs `claude_md_tools_table_counts_are_accurate`,
`claude_md_tools_table_matches_the_code`, `readme_tools_counts_are_accurate`
and `readme_tools_tables_match_the_code`. The lines to change:

- `CLAUDE.md:139` — `**36 tools: 27 core + 9 deferred.**` → **39 / 27 / 12**,
  plus three rows in the tools table (Loaded column = **`runs`** in bold, the
  way `read_script` shows **scripts**).
- `README.md:496` — *"The model has **36 tools**. The 27 **core** … the 9
  **deferred**"* → the same three numbers.
- `README.md:534-539` — the Group table gains a row:
  `| `runs` | `run_log_tail`, `run_log_search`, `get_turn` |`.

**F6 — none of these is approval-gated, and that is load-bearing.** They are
read-only and prompt for nothing, so they do **not** go in
`APPROVAL_GATED_TOOLS` and carry **no** `**⚠**` in the README. `CLAUDE.md`
warns why: that list is also what exempts a tool from the per-turn/session/tool
budget caps, so *"add a tool that does not really prompt and you hand it an
uncapped call budget."*

**F7 — the worked example for a deferred read tool, end to end.**
`read_runbook` is the closest analogue; it touches exactly the files this phase
touches:

```rust
// src/ai/tools/defs.rs
    ToolDef {
        name: "read_runbook",
        deferred_group: Some("runbooks"),
        description: "Read the full content of a named runbook from ~/.daemoneye/runbooks/.",
        params: &[ParamDef { name: "name", ty: ParamTy::Str, required: true,
                             description: "Name of the runbook to read (no extension)." }],
    },
// src/ai/tools/args.rs
#[derive(Deserialize)]
pub(super) struct ReadRunbookArgs { name: String }
// src/ai/tools/dispatch.rs
        "read_runbook" => dispatch::<ReadRunbookArgs>(id, args, ts),
// src/ai/types/pending.rs
    ReadRunbook { id: String, thought_signature: Option<String>, name: String },
            PendingCall::ReadRunbook { name, .. } => name.clone(),   // summary()
            PendingCall::ReadRunbook { .. } => "read_runbook",        // tool_name()
// src/daemon/executor/mod.rs
        PendingCall::ReadRunbook { name, .. } => {
            Ok(ToolCallOutcome::Result(knowledge::read_runbook(name)))
        }
```

**F8 — silent tools opt into `ToolStarted`/`ToolFinished`.**
`should_emit_tool_feedback` (`pending.rs:792`) lists the read tools —
`ReadScript`, `ReadFile`, `ReadRunbook`, … — because they have no approval
prompt of their own. **All three new tools belong in that list**, or the user
sees nothing happen.

**F9 — `#[serde(default)]` and `Option<u64>` are the arg idiom for an optional
limit** (`ReadFileArgs`, `args.rs:84-91`):

```rust
pub(super) struct ReadFileArgs {
    path: String,
    offset: Option<u64>,
    limit: Option<u64>,
```

## Spec

### Task 1 — `src/daemon/run/`: the three read functions

Pure-ish readers over `read_turn_log` from phase-02a. No new file I/O beyond
what that already does.

- **tail**: last `limit` records of a run, oldest-first in the returned slice.
- **search**: records whose serialised text contains a query substring
  (case-insensitive), capped at `limit`.
- **get turn**: **all** records whose event is `TurnStarted { turn }` or that
  follow it up to the next `TurnStarted` — i.e. one turn's records. Uncapped
  (F1).

**Clamp the limit inside these functions, not at the tool boundary**, so the
cap cannot be bypassed by a future caller. Pin: a `MAX_RUN_LOG_LIMIT` constant
of **200**, `None` → a smaller default of **50**, and any larger request
silently clamped to 200. Name the constant so a test can assert against it.

Return a rendered `String` for the tool result — the same shape
`knowledge::read_runbook` returns — with a one-line header naming the run and
how many records are shown, then the records. **Say when you clamped**: if the
request was reduced, the header must say so, otherwise the model cannot tell a
truncated answer from a complete one.

### Task 2 — the tool plumbing (the checklist)

Three tools, all `deferred_group: Some("runs")`, all read-only:

| Tool | Params |
|---|---|
| `run_log_tail` | `run_id` (str, required), `limit` (int, optional) |
| `run_log_search` | `run_id` (str, required), `query` (str, required), `limit` (int, optional) |
| `get_turn` | `run_id` (str, required), `turn` (int, required) |

Follow `CLAUDE.md` § "Adding a new AI tool", **plus F2's twelfth step**. The
touch points, from F7: `defs.rs`, `args.rs`, `dispatch.rs` (arm **and**
`minimal_args`), `pending.rs` (variant + `to_tool_call` + `id()` +
`tool_name()` + `summary()` + `should_emit_tool_feedback` per F8),
`events.rs`, `stream.rs`, `executor/mod.rs`, and the two doc files per F5.

An unknown or malformed `run_id` is a **tool result**, not an `Err` — the model
should see "no such run" and adapt. `RunId::parse` already rejects a bad id
(phase-01); surface its message rather than propagating an error.

## Acceptance criteria

Each was run against the current tree and returns the "now" value shown.

- [ ] All three tools exist in `TOOLS` under the `runs` group:
      `grep -c 'deferred_group: Some("runs")' src/ai/tools/defs.rs` → **3**
      (now `0`).
- [ ] **F2's hidden gate is satisfied** — each tool has a `minimal_args` arm:
      `grep -cE '"(run_log_tail|run_log_search|get_turn)" =>' src/ai/tools/dispatch.rs`
      → **6 or more** (now `0`) — three dispatch arms + three `minimal_args`
      arms.
- [ ] **They are silent tools (F8):**
      `grep -cE 'PendingCall::(RunLogTail|RunLogSearch|GetTurn)' src/ai/types/pending.rs`
      → **10 or more** (now `0`) — variant + `to_tool_call` + `id()` +
      `tool_name()` + `summary()` + `should_emit_tool_feedback` across three
      tools. State the number you got.
- [ ] **They are NOT approval-gated (F6):**
      `grep -cE 'run_log_tail|run_log_search|get_turn' src/ai/tools/defs.rs`
      → count includes the `TOOLS` entries but
      `grep -A20 'APPROVAL_GATED_TOOLS' src/ai/tools/defs.rs | grep -cE 'run_log|get_turn'`
      → **0** (now `0`, preservation), and `grep -c 'get_turn' README.md`
      lines carry no `**⚠**`.
- [ ] **The clamp is a named constant and is enforced in the reader:**
      `cat src/daemon/run/*.rs | grep -c 'MAX_RUN_LOG_LIMIT'` → **2 or more**
      (now `0`) — the definition and at least one use. *(Note the `cat |` form:
      `grep -c` over a file glob prefixes each count with `filename:`, which is
      not a bare number.)*
- [ ] **The doc tables and all three counts are updated (F5):**
      `grep -c '39 tools: 27 core + 12 deferred' CLAUDE.md` → **1** (now `0`),
      `grep -c '39 tools' README.md` → **1 or more** (now `0`), and
      `grep -c '| `runs` |' README.md` → **1** (now `0`).
- [ ] **The doc-truth gates pass** — this is F5's whole point:
      `cargo test --test doc_truth` reports **10 passed; 0 failed** (now `10`).
      *(Preservation criterion: the count does not change; the gates must
      simply keep passing with the new tools and counts.)*
- [ ] No `unsafe`/`unwrap`/`expect`/`panic!` outside test code in the run module:
      `for f in src/daemon/run/*.rs; do awk '/^#\[cfg\(test\)\]/{exit} {print}' $f; done | grep -cE '\.(unwrap|expect)\(|panic!\(|unsafe'`
      → **0** (now `0`, preservation).
- [ ] `cargo test --lib daemon::run::` reports **23 or more** passing,
      `0 failed` (now `17`). *Arithmetic: 17 existing + the 6 named tests in
      the Test plan = 23.*
- [ ] `cargo test --lib ai::tools::` reports **17** passing, `0 failed`
      (now `17`). *(Preservation criterion: adding tools adds no test
      **functions** here — `dispatch_roundtrip_all_tools` is one test that
      walks all of `TOOLS` — so the count must not move. If it does, say why.)*
- [ ] The full lib suite reports `0 failed` on **three consecutive runs**.
- [ ] All four gates pass.

## Test plan

Six tests, names pinned; placement is yours. Tests that write a turn log take
`crate::test_home_guard()` bound to a named variable with `HOME` pointed at a
`tempfile::tempdir()` — the same shape phase-02a's tests use.

- `run_log_tail_returns_the_last_n_records` — write 5 records, tail with
  `limit = 2`, assert the **last two** come back in order.
- `run_log_tail_clamps_an_oversized_limit` — request `limit = 10_000`, assert
  at most `MAX_RUN_LOG_LIMIT` records come back **and** the rendered header
  says the request was clamped. Both halves: a clamp the model cannot see is a
  truncation it will mistake for the whole log.
- `run_log_search_matches_case_insensitively` — a `Note { text: "Disk Full" }`
  is found by a query of `disk full`.
- `run_log_search_returns_nothing_for_no_match` — the **negative**: a query
  matching no record returns a result saying so, **not** an error and not the
  whole log.
- `get_turn_returns_only_that_turns_records` — a log with turns 1, 2 and 3;
  `get_turn(2)` returns turn 2's records and **neither** turn 1's nor turn 3's.
- `run_log_tools_reject_an_unknown_run_as_a_result_not_an_error` — a `run_id`
  that parses but has no log, and one that does not parse at all: both come
  back as a `String` result the model can read, never an `Err`.

**Mutation-check two of these before reporting**, pasting red and green
`test result:` lines for each:

1. Remove the clamp (use the requested limit directly) →
   `run_log_tail_clamps_an_oversized_limit` must go red.
2. Make `get_turn` return the whole log instead of one turn's slice →
   `get_turn_returns_only_that_turns_records` must go red.

Both are verified independently at review.

## End-to-end verification

Run this block verbatim from the repo root. It writes `/tmp/e2e-m24-02b.txt`.

```sh
{
echo "== A. build =="; cargo build 2>&1 | tail -2; echo "cargo_exit=${PIPESTATUS[0]}"
echo "== B. named tests (each line is one pinned test) =="
cargo test --lib 2>&1 | grep -E "^test daemon::run::.*(run_log_|get_turn_).* ok$" | sed 's/^test //' | sort
echo "cargo_exit=${PIPESTATUS[0]}"
echo "== C. run module total (>=23) =="
cargo test --lib daemon::run:: 2>&1 | grep -E "^test result:"; echo "cargo_exit=${PIPESTATUS[0]}"
echo "== D. the tool gates =="
printf '%-18s ' "ai::tools"; cargo test --lib ai::tools:: 2>&1 | grep -E "^test result:"; echo "cargo_exit=${PIPESTATUS[0]}"
printf '%-18s ' "doc_truth"; cargo test --test doc_truth 2>&1 | grep -E "^test result:"; echo "cargo_exit=${PIPESTATUS[0]}"
echo "== E. lib suite, three consecutive runs =="
for r in 1 2 3; do cargo test --lib 2>&1 | grep -E "^test result:" | head -1; done
echo "== F. structural greps (each must print the stated number) =="
echo -n "runs group in TOOLS        (3): "; grep -c 'deferred_group: Some("runs")' src/ai/tools/defs.rs
echo -n "dispatch + minimal_args  (>=6): "; grep -cE '"(run_log_tail|run_log_search|get_turn)" =>' src/ai/tools/dispatch.rs
echo -n "pending.rs arms         (>=10): "; grep -cE 'PendingCall::(RunLogTail|RunLogSearch|GetTurn)' src/ai/types/pending.rs
echo -n "not approval-gated         (0): "; grep -A20 'APPROVAL_GATED_TOOLS' src/ai/tools/defs.rs | grep -cE 'run_log|get_turn'
echo -n "clamp constant           (>=2): "; cat src/daemon/run/*.rs | grep -c 'MAX_RUN_LOG_LIMIT'
echo -n "CLAUDE.md counts           (1): "; grep -c '39 tools: 27 core + 12 deferred' CLAUDE.md
echo -n "README counts            (>=1): "; grep -c '39 tools' README.md
echo -n "README runs group row      (1): "; grep -c '| `runs` |' README.md
echo -n "no unwrap/panic pre-test   (0): "
for f in src/daemon/run/*.rs; do awk '/^#\[cfg\(test\)\]/{exit} {print}' $f; done | grep -cE '\.(unwrap|expect)\(|panic!\(|unsafe'
echo "== G. nothing leaked into the real HOME =="
echo -n "runs dir in the developer HOME (0): "; ls -d "$HOME/.daemoneye/var/log/runs" 2>/dev/null | wc -l
} > /tmp/e2e-m24-02b.txt 2>&1
cat /tmp/e2e-m24-02b.txt
```

Paste the contents into your Update Log entry as a fenced block. **The entry's
header must be exactly this line**, or the self-check cannot find it:

```
### Update — YYYY-MM-DD HH:MM (end-to-end verification)
```

Then run the self-check and paste its verdict into the same entry:

```sh
D=docs/dev/milestones/M24-run-manager/phase-02b-run-log-tools.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-m24-02b.txt
diff /tmp/pasted-m24-02b.txt /tmp/e2e-m24-02b.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

**Paste the block exactly as it came out, including any section that fails.**
Commentary belongs in prose **outside** the fenced block.

**Your prose about section F must state each grep's value as it printed.**

**Section D is the gate section.** `ai::tools` carries
`dispatch_roundtrip_all_tools` (F2) and `doc_truth` carries the four table
gates (F5). If either is red, the tool is half-added — say so rather than
letting the block pass.

## Authorizations

You may edit, and nothing else:

- `src/daemon/run/` — the three read functions and their tests (Task 1)
- `src/ai/tools/defs.rs` — the three `ToolDef` entries. **Do not** touch
  `APPROVAL_GATED_TOOLS` (F6).
- `src/ai/tools/args.rs` — the three arg structs
- `src/ai/tools/dispatch.rs` — three dispatch arms **and** three `minimal_args`
  arms (F2)
- `src/ai/types/pending.rs` — the three variants and their six arms each (F7, F8)
- `src/ai/types/events.rs` — the three `AiEvent` variants
- `src/daemon/stream.rs` — the three `AiEvent` match arms
- `src/daemon/executor/mod.rs` — the three `PendingCall` arms
- `CLAUDE.md` and `README.md` — the counts and table rows (F5)
- this phase doc's Update Log

**Do not** change the turn-log format, writer or reader from phase-02a — this
phase only reads. **Do not** add a config key. **Do not** add a dependency.

**Do not run `pkill`, `killall`, `tmux kill-server`, or any signal to a process
group.** Use `find <dir> -depth -delete` for cleanup, never `rm -rf`.

## Out of scope

- **`sre.toml` prose.** Optional for deferred tools (F4 — `create_agent` has
  none), and the generated `load_tools` catalog already names the group. Skip
  it; a later phase can add prose when the run tools have a story to tell.
- **Anything that writes a turn log.** Phases 05–08 wire runs; these tools read
  what is there.
- **Cross-run search** (searching every run at once). `run_log_search` takes a
  `run_id`; a global search wants the knowledge index and no phase owns it.
- **Retention of `var/log/runs`** — still `Pending`, still phase-10's.
- **`/runs` and `/run` CLI commands** — phase-10. These are AI tools only.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-09-09 13:53 (started)

Status flipped `todo` → `in-progress`; milestone README phase-table row updated to
match. Executor dispatched. Pre-flight verified: branch `master`, tree clean, full
lib `1683 passed; 0 failed; 4 ignored`, `cargo test --lib daemon::run::` → `17
passed; 0 failed`.

### Update — 2026-09-09 14:17 (end-to-end verification)

Every acceptance criterion checked against the real artifacts. Numbers from
section F, as they printed: `runs group in TOOLS` **3**, `dispatch +
minimal_args` **6**, `pending.rs arms` **15** (variant + `to_tool_call` +
`id()` + `tool_name()` + `summary()` + `should_emit_tool_feedback`, 5 arms ×
3 tools), `not approval-gated` **0**, `clamp constant` **18** (definition plus
uses across `log_read.rs` and the milestone doc — the `cat |` false-shared-form
note in the spec was read; this count is over `src/daemon/run/*.rs` only),
`CLAUDE.md counts` **1**, `README counts` **1**, `README runs group row` **1**,
`no unwrap/panic pre-test` **0**, real-HOME runs dir **0**.

Section B lists the six pinned tests plus three extras at `... ok`. Section D:
`ai::tools` **17 passed; 0 failed** (F2's `dispatch_roundtrip_all_tools` walks
the 39-entry `TOOLS`), `doc_truth` **10 passed; 0 failed** (F5's four table
gates keep passing). Section E: three consecutive full-lib runs, `0 failed`
each.

Mutation checks (pasted red → green `test result:` lines below, ran before
restoring): 1) `Some(n) => n.min(MAX)` → `n.max(MAX)` turned
`run_log_tail_clamps_an_oversized_limit` red (`FAILED. 1690 passed; 4
failed`) — the tail, search, and default-limit tests caught it too; 2)
`get_turn`'s slice end forced to `0` turned
`get_turn_returns_only_that_turns_records` red (`FAILED. 1693 passed; 1
failed`). Both were green again after restore — the consecutive runs in
section E are the restored tree.

```text
== A. build ==
   Compiling daemoneye v1.0.0 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1.95s
cargo_exit=0
== B. named tests (each line is one pinned test) ==
daemon::run::log_read::tests::get_turn_reports_an_unknown_turn ... ok
daemon::run::log_read::tests::get_turn_returns_only_that_turns_records ... ok
daemon::run::log_read::tests::run_log_search_matches_case_insensitively ... ok
daemon::run::log_read::tests::run_log_search_respects_the_clamp ... ok
daemon::run::log_read::tests::run_log_search_returns_nothing_for_no_match ... ok
daemon::run::log_read::tests::run_log_tail_clamps_an_oversized_limit ... ok
daemon::run::log_read::tests::run_log_tail_returns_the_last_n_records ... ok
daemon::run::log_read::tests::run_log_tail_uses_the_default_when_limit_is_omitted ... ok
daemon::run::log_read::tests::run_log_tools_reject_an_unknown_run_as_a_result_not_an_error ... ok
cargo_exit=0
== C. run module total (>=23) ==
test result: ok. 28 passed; 0 failed; 0 ignored; 0 measured; 1670 filtered out; finished in 0.01s
cargo_exit=0
== D. the tool gates ==
ai::tools          test result: ok. 17 passed; 0 failed; 0 ignored; 0 measured; 1681 filtered out; finished in 0.00s
cargo_exit=0
doc_truth          test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
cargo_exit=0
== E. lib suite, three consecutive runs ==
test result: ok. 1694 passed; 0 failed; 4 ignored; 0 measured; 0 filtered out; finished in 7.03s
test result: ok. 1694 passed; 0 failed; 4 ignored; 0 measured; 0 filtered out; finished in 7.26s
test result: ok. 1694 passed; 0 failed; 4 ignored; 0 measured; 0 filtered out; finished in 6.95s
== F. structural greps (each must print the stated number) ==
runs group in TOOLS        (3): 3
dispatch + minimal_args  (>=6): 6
pending.rs arms         (>=10): 15
not approval-gated         (0): 0
clamp constant           (>=2): 18
CLAUDE.md counts           (1): 1
README counts            (>=1): 1
README runs group row      (1): 1
no unwrap/panic pre-test   (0): 0
== G. nothing leaked into the real HOME ==
runs dir in the developer HOME (0): 0
```

### Update — ts=1788988822739 (complete, server-authored)

**Summary:** **Summary**

Phase 02b is implemented and committed (`f863940`, working tree clean). `src/daemon/run/log_read.rs` adds the three read-only projections over phase-02a's turn log — `run_log_tail`, `run_log_search`, `get_turn` — with the limit clamped **inside** the readers (`MAX_RUN_LOG_LIMIT = 200`, default 50, `0`/`None` → default) and a header that names the run, the totals seen vs. shown, and says explicitly when a request was clamped; `get_turn` is the uncapped escape hatch. The tools are plumbed through all nine touch points of the CLAUDE.md checklist plus the two hidden gates: `dispatch.rs` carries three dispatch arms **and** three `minimal_args` arms (F2's `dispatch_roundtrip_all_tools` passes), and `["runs"]` needed no catalog registration (F3). All three are silent tools in `should_emit_tool_feedback` (F8) and none appears in `APPROVAL_GATED_TOOLS` (F6); doc counts now read 39 / 27 + 12 in both `CLAUDE.md` and `README.md`, including the new `` | `runs` | `` group row.

**Notes for review**

- **Deviations worth knowing:** (1) the clamped readers' headers are a small superset of the spec's sketch — search names its query and, when the cap cut matches off, prints "N further match(es) not shown — raise the limit up to 200", which is the same clamp-honesty the spec asked for and falls out of rendering search under a header; (2) `run_log_input_rejected` is a tiny helper the executor arms route `RunId::parse` failures through, so a malformed id surfaces as a readable result string, never an `Err`; (3) `clamp_note` folds `Some(0)` into the default (documented on `clamp_run_log_limit`) since "show me the last 0" is nonsense.
- **Grep values as printed** (section F): rows in `TOOLS` **3**, dispatch + `minimal_args` **6**, `pending.rs` arms **15**, not-approval-gated **0**, clamp-constant occurrences **18** (over `src/daemon/run/*.rs` only — the spec noted the `cat |` count-prefix hazard), `CLAUDE.md` counts **1**, `README` counts **1**, README group row **1**, no-unwrap/panic pre-test **0**, real-HOME runs dir **0**.
- **Gates:** `cargo build` clean, `clippy -D warnings` clean, `fmt` clean; `ai::tools` `17 passed; 0 failed` (unchanged — the roundtrip test is one function that walks all 39 `TOOLS`), `doc_truth` `10 passed; 0 failed`, full lib `1694 passed; 0 failed` on three consecutive runs, `daemon::run` `28 passed; 0 failed`. Mutation checks: removing `.min` (→ `.max`) turned `run_log_tail_clamps_an_oversized_limit` red (plus 3 neighbours), and forcing `get_turn`'s slice end to `0` turned `get_turn_returns_only_that_turns_records` red; both restored and the pasted E2E block is from the restored tree, and the self-check reports `PASTE MATCH`. The end-to-end transcript lives in its own Update Log entry (`### Update — 2026-09-09 14:17 (end-to-end verification)`), built live and diff-verified against the pasted block.
- **Out of scope, left alone as specified:** `sre.toml` prose (F4 — optional; the generated `load_tools` catalog already names the group), anything that writes a turn log, cross-run search, retention, and the `/runs` CLI commands.

**Executor:** local-inference-lab/GLM-5.3-Flash-NVFP4-Spark

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.07s


LINT
    Checking daemoneye v1.0.0 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 3.88s


TEST
nning 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 6 tests
test header_status_reads_bare_word ... ok
test open_bug_on_done_phase_is_a_finding ... ok
test open_bug_on_in_progress_phase_is_clean ... ok
test header_status_strips_trailing_prose ... ok
test header_status_uses_first_occurrence_only ... ok
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
test g3_tool_policy_deny_merged_and_enforced ... ok
test g5_child_inherits_depth_and_parent ... ok
test g5_depth_limit_enforced ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ipc_session_info_round_trip ... ok
test config_pricing_round_trip ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test window_switch_does_not_corrupt_chat ... ignored
test g4_briefing_injection_block_format ... ok
test ipc_tool_call_response_round_trip ... ok
test ipc_ask_round_trip ... ok
test minimal_config_parsing ... ok
test ghost_config_parsing ... ok
test schedule_store_persistence ... ok
test g4_briefing_masking_applied ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test event_log_entry_format ... ok
test event_log_append_read ... ok
test g4_briefing_injects_on_next_run ... ok
test g4_briefing_read_and_clear ... ok
test g6_agent_config_roundtrip ... ok
test session_jsonl_round_trip ... ok
test session_index_persistence ... ok
test g6_agent_namespace_field_persisted ... ok
test webhook_alert_to_event_log ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_no_severity_passes_gate ... ok
test webhook_alert_unrankable_severity_passes_gate ... ok
test webhook_alert_below_threshold_discarded ... ok

test result: ok. 31 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.04s


running 10 tests
test webhook_ghost_e2e_http ... ignored
test held_port_cannot_be_rebound ... ok
test webhook_ports_differ_between_environments ... ok
test stub_returns_canned_response_via_make_client ... ok
test webhook_ghost_e2e_deterministic ... ok
test hooks_land_on_private_server ... ok
test config_contains_webhook_and_stub_url ... ok
test daemon_boots_in_throwaway_root ... ok
test default_server_unchanged ... ok
test daemon_webhook_returns_200 ... ok

test result: ok. 9 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.16s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

   Compiling daemoneye v1.0.0 (/home/matt/src/daemoneye)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.95s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-9810e08c3de1e001)
     Running unittests src/main.rs (target/debug/deps/daemoneye-54ad57cf3704bc40)
     Running tests/bug_tracker.rs (target/debug/deps/bug_tracker-d709c9b862b5691d)
     Running tests/doc_truth.rs (target/debug/deps/doc_truth-34cb94d3012a3d69)
     Running tests/integration.rs (target/debug/deps/integration-deb70358b520d0d5)
     Running tests/isolation.rs (target/debug/deps/isolation-45facff22bac60f5)
   Doc-tests daemoneye

```

**Files changed:**

- `CLAUDE.md` — +4 -1
- `README.md` — +6 -1
- `docs/dev/milestones/M24-run-manager/README.md` — +1 -1
- `docs/dev/milestones/M24-run-manager/phase-02b-run-log-tools.md` — +77 -1
- `src/ai/tools/args.rs` — +64 -0
- `src/ai/tools/defs.rs` — +78 -0
- `src/ai/tools/dispatch.rs` — +11 -2
- `src/ai/types/events.rs` — +22 -0
- `src/ai/types/pending.rs` — +77 -0
- `src/daemon/executor/mod.rs` — +29 -0
- `src/daemon/run/log_read.rs` — +492 -0
- `src/daemon/run/mod.rs` — +5 -0
- `src/daemon/stream.rs` — +41 -0

**Commit:** f8639407f790605471fc373461b73dfdbb6b90a8

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).
