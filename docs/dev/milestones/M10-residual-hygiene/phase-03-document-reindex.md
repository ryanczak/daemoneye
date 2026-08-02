# Phase 03: Document `daemoneye reindex`, and gate it

**Milestone:** M10 — Residual Hygiene
**Status:** done
**Depends on:** phase-02 (`done`)
**Estimated diff:** ~70 lines — two doc sentences plus a new tripwire test.

## Goal

`daemoneye reindex` shipped in M9 and neither `CLAUDE.md` nor
`docs/architecture.md` describes it. Document it in both, and add a gate so it
cannot silently vanish again.

This is M10's last item.

## Read this first — why a plain grep is not enough

`docs/architecture.md` **already contains the string `daemoneye reindex` twice**,
at lines 406 and 411. Both are inside `### Active milestone — M10 Residual
Hygiene`, which sits under `## 5. Milestone roadmap`.

**That section is rewritten at every milestone close.** The text describing
`reindex` today is the same text that will be replaced when M11 is scoped. A
criterion of the form `grep -c 'reindex' docs/architecture.md >= 1` is therefore
**already satisfied right now, before any work**, and would keep passing after
the durable documentation was deleted.

Measured:

| Scope | `reindex` mentions today |
|---|---|
| `CLAUDE.md`, whole file | **0** |
| `docs/architecture.md`, whole file | **2** — both transient |
| `docs/architecture.md`, everything **before** `## 5. Milestone roadmap` | **0** |

So the gate — and the acceptance criteria — must look only at the **durable**
part of `architecture.md`.

## Current state

Measured against the tree on 2026-08-02. Every claim was executed.

Baselines: `cargo test --lib` **1038**; `cargo test --test doc_truth` **1**.

**The two places to edit.** Both already discuss `reconcile_index()` and are
incomplete without the command — this is filling a gap, not bolting on a section.

`docs/architecture.md` § 2.3 Knowledge flow (line 184), the relevant sentence:

```
Memory is
indexed in a SQLite FTS5 database at `var/index/memory.db`, maintained
best-effort on every add/update/delete and rebuilt by `reconcile_index()`
whenever the index is found empty.
```

`CLAUDE.md`, the `src/memory/index.rs` row of the key-files table (line 72),
the relevant clause:

```
`reconcile_index()` rebuilds from the files on disk and runs automatically when
the index is empty, which is what indexes the memories a fresh install seeds.
```

Both stop exactly where the operator command belongs: they say the rebuild fires
when the index is *empty*, and never say what to do about an index that is
populated but wrong.

**The facts to document** (all verified against the shipped binary in M9):

- `daemoneye reindex` rebuilds the index from the memory files on disk and
  reports the row count before and after.
- It needs **no running daemon**.
- It is **safe to run while the daemon is up**: the rebuild is a single
  transaction (`src/memory/index.rs:254`–`:311`), so a concurrent search sees the
  old index or the new one, never a half-empty one.
- It is idempotent, and tolerates a bare `$HOME`.
- Reconcile-on-empty only fires at **zero rows**, so a *stale* index — rows
  present but wrong — is reachable **only** through this command.

## Spec

### Task 1 — `docs/architecture.md` § 2.3

Extend the Knowledge-flow sentence so it covers the stale case. Keep it to one
or two sentences in the existing paragraph's voice; do **not** add a new heading.
It must contain the literal string `daemoneye reindex` and say that the rebuild
is a single transaction and therefore safe with the daemon running.

Do **not** edit anything under `## 5. Milestone roadmap`.

### Task 2 — `CLAUDE.md`, the `src/memory/index.rs` row

Extend that table row's `reconcile_index()` clause with the same facts, in the
row's existing terse style. It must contain the literal `daemoneye reindex`.

**Keep it one table row.** The row is a single line of Markdown; adding a real
newline inside it breaks the table. Do not restructure the table or add a column.

### Task 3 — gate both, so this cannot silently regress

`tests/doc_truth.rs` currently guards against *forbidden* strings via
`RETIRED_CLAIMS`. Add the symmetric case. Insert this **above** the existing
`#[test] fn docs_do_not_carry_retired_index_claims()`, leaving that test and the
`RETIRED_CLAIMS` table untouched:

```rust
/// (doc path, required substring, why it must be documented)
///
/// Checked against the **durable** part of each doc: for `docs/architecture.md`
/// everything before the milestone roadmap, because that section is rewritten
/// every milestone and a claim living only there disappears on the next close.
const REQUIRED_CLAIMS: &[(&str, &str, &str)] = &[
    (
        "CLAUDE.md",
        "daemoneye reindex",
        "the operator entry point to reconcile_index() must stay documented",
    ),
    (
        "docs/architecture.md",
        "daemoneye reindex",
        "the operator entry point to reconcile_index() must stay documented",
    ),
];

/// The heading that begins the transient part of `docs/architecture.md`.
const ROADMAP_HEADING: &str = "## 5. Milestone roadmap";

fn durable_part(doc: &str, text: &str) -> String {
    if doc == "docs/architecture.md" {
        match text.find(ROADMAP_HEADING) {
            Some(i) => text[..i].to_string(),
            None => panic!("{doc} no longer contains {ROADMAP_HEADING:?}"),
        }
    } else {
        text.to_string()
    }
}

#[test]
fn docs_document_the_reindex_command() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut missing = Vec::new();
    for (doc, phrase, why) in REQUIRED_CLAIMS {
        let path = root.join(doc);
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
        if !durable_part(doc, &text).contains(phrase) {
            missing.push(format!("{doc}: missing {phrase:?} — {why}"));
        }
    }
    assert!(
        missing.is_empty(),
        "docs no longer document these:\n{}",
        missing.join("\n")
    );
}
```

This block was compiled and run against the current tree before this spec was
written. **On today's tree it FAILS with both docs listed as missing** — which is
the proof that the milestone-roadmap mentions do not satisfy it:

```
test docs_document_the_reindex_command ... FAILED
CLAUDE.md: missing "daemoneye reindex" — the operator entry point ...
docs/architecture.md: missing "daemoneye reindex" — the operator entry point ...
```

The `panic!` if `ROADMAP_HEADING` is absent is deliberate: if that heading is ever
renamed, the gate must fail loudly rather than silently start checking the whole
file.

## Acceptance criteria

- [ ] `cargo test --test doc_truth` reports **2** passed (was 1). Both
      `docs_document_the_reindex_command` and
      `docs_do_not_carry_retired_index_claims` pass.
- [ ] `grep -c 'daemoneye reindex' CLAUDE.md` is **≥ 1** (today **0**).
- [ ] In `docs/architecture.md`, `daemoneye reindex` appears **before** the
      `## 5. Milestone roadmap` heading — today it appears only after it:
      `awk '/^## 5\. Milestone roadmap/{exit} {print}' docs/architecture.md | grep -c 'daemoneye reindex'`
      must be **≥ 1** (today **0**).
- [ ] Nothing under `## 5. Milestone roadmap` is modified: `git diff` on
      `docs/architecture.md` shows no hunk at or below that heading.
- [ ] `CLAUDE.md`'s key-files table still renders — the `src/memory/index.rs`
      entry is still exactly **one** line: `grep -c '^| .src/memory/index.rs.' CLAUDE.md`
      is **1**, and `wc -l < CLAUDE.md` is still **189** — the row grows in place,
      it does not gain a line.
- [ ] `cargo test --lib` still reports **1038** — this phase adds no lib tests.
- [ ] `RETIRED_CLAIMS` is unchanged: `grep -c 'grep fallback' tests/doc_truth.rs`
      is still **2** — the phrase appears once as the forbidden string and once
      in its rationale text.
- [ ] `cargo fmt --all --check`, `cargo build`, and `cargo clippy --all-targets
      --all-features -- -D warnings` all clean.
- [ ] Only these three files change: `CLAUDE.md`, `docs/architecture.md`,
      `tests/doc_truth.rs`.

## Test plan

New: `docs_document_the_reindex_command`.
Unchanged and must stay green: `docs_do_not_carry_retired_index_claims`.

**Mutation-check before reporting complete, and state both results.** The second
one is the point of the whole phase — it is what distinguishes this gate from a
plain grep:

1. Delete the `daemoneye reindex` sentence you added to `CLAUDE.md`.
   `docs_document_the_reindex_command` must **FAIL**. Restore.
2. Delete the sentence you added to `docs/architecture.md` § 2.3, **leaving the
   milestone-roadmap mentions in place**. The test must still **FAIL**, naming
   `docs/architecture.md`. Restore.

If step 2 passes, the gate is reading the transient section and is worthless.

## End-to-end verification

Paste the **literal output** of this block into the Update Log, not a summary:

```sh
cargo test --test doc_truth 2>&1 | grep -E '^test |test result'
echo "CLAUDE.md:        $(grep -c 'daemoneye reindex' CLAUDE.md)   # >= 1"
echo "arch durable:     $(awk '/^## 5\. Milestone roadmap/{exit} {print}' docs/architecture.md | grep -c 'daemoneye reindex')   # >= 1"
echo "arch whole file:  $(grep -c 'daemoneye reindex' docs/architecture.md)   # was 2, now >= 3"
echo "index row lines:  $(grep -c '^| .src/memory/index.rs.' CLAUDE.md)   # 1"
echo "RETIRED intact:   $(grep -c 'grep fallback' tests/doc_truth.rs)   # 2
echo "CLAUDE.md lines:  $(wc -l < CLAUDE.md)   # 189, unchanged"
cargo test --lib 2>&1 | grep 'test result' | head -1     # 1038
cargo fmt --all --check && echo "fmt ok"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -2
```

## Authorizations

- Edit `CLAUDE.md` (the `src/memory/index.rs` table row only),
  `docs/architecture.md` (§ 2.3 only), and `tests/doc_truth.rs` (add the new
  table, helper, and test).

## Out of scope

- **Anything under `## 5. Milestone roadmap`** in `docs/architecture.md`. The
  architect maintains that section at milestone close.
- Changing `RETIRED_CLAIMS` or `docs_do_not_carry_retired_index_claims`.
- Restructuring the `CLAUDE.md` key-files table, or documenting any other
  subcommand. Only `reindex` is in scope; `audit-prompts`, `install-sudoers` and
  `session import` are already documented and are not this phase's business.
- Any change to `src/` — this phase touches no production code.
- Adding `reindex` to the "Current AI tools" table. It is a **CLI subcommand**,
  not an AI tool; the AI cannot call it.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-08-02 20:10 (started)

**Executor:** Qwen/Qwen3.6-27B-FP8 (corrected at review — the entry self-reported as Claude)

Implementing phase-03: document `daemoneye reindex` in `CLAUDE.md` and `docs/architecture.md`, and add a tripwire test in `tests/doc_truth.rs`.

### Update — ts=1785701829552 (complete, server-authored)

**Summary:** All three tasks complete. Summary + Notes for review:

**What was built:** Documented `daemoneye reindex` in the durable sections of both `CLAUDE.md` (extended the `src/memory/index.rs` table row in place, keeping it one line at 189 total) and `docs/architecture.md` (§ 2.3 Knowledge flow, added two sentences covering the stale-index use case, single-transaction safety, and idempotency). Added a tripwire test `docs_document_the_reindex_command` in `tests/doc_truth.rs` that checks only the durable part of `docs/architecture.md` (everything before `## 5. Milestone roadmap`), proving the gate rejects transient-only mentions.

**E2E verification:** `cargo test --test doc_truth` reports **2 passed** (both new and existing). All acceptance criteria met: `CLAUDE.md` has 1 mention, durable `architecture.md` has 1, whole file has 3, the index row is still 1 line, `CLAUDE.md` is still 189 lines, `RETIRED_CLAIMS` is intact (2 grep-fallback hits), lib tests still 1038, fmt/clippy clean. Mutation checks confirmed: removing the `CLAUDE.md` mention fails the test, and removing the durable `architecture.md` mention (leaving roadmap mentions) also fails — the gate correctly distinguishes durable from transient content.

**Executor:** Qwen/Qwen3.6-27B-FP8

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.06s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.07s


TEST
tests::load_messages_max_count_truncates ... ok
test session_store::tests::rename_nonexistent_errors ... ok
test session_store::tests::rename_to_existing_errors ... ok
test session_store::tests::rename_updates_dir_and_index ... ok
test session_store::tests::save_and_load_round_trip ... ok
test session_store::tests::update_in_place_allowed ... ok
test test_home_guard_tests::guard_restores_home_on_drop ... ok

test result: ok. 1038 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.51s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 6 tests
test header_status_reads_bare_word ... ok
test header_status_strips_trailing_prose ... ok
test open_bug_on_in_progress_phase_is_clean ... ok
test open_bug_on_done_phase_is_a_finding ... ok
test header_status_uses_first_occurrence_only ... ok
test repository_bug_tracker_is_consistent ... ok

test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 2 tests
test docs_document_the_reindex_command ... ok
test docs_do_not_carry_retired_index_claims ... ok

test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 32 tests
test daemon_ping_status_loop ... ignored
test g4_briefing_injection_block_format ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g5_depth_limit_enforced ... ok
test g5_child_inherits_depth_and_parent ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ghost_config_parsing ... ok
test ipc_tool_call_response_round_trip ... ok
test ipc_ask_round_trip ... ok
test ipc_session_info_round_trip ... ok
test minimal_config_parsing ... ok
test window_switch_does_not_corrupt_chat ... ignored
test event_log_append_read ... ok
test schedule_store_persistence ... ok
test config_pricing_round_trip ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test event_log_entry_format ... ok
test g4_briefing_injects_on_next_run ... ok
test g4_briefing_read_and_clear ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test session_jsonl_round_trip ... ok
test session_index_persistence ... ok
test g4_briefing_masking_applied ... ok
test webhook_alert_no_severity_passes_gate ... ok
test webhook_alert_unrankable_severity_passes_gate ... ok
test webhook_alert_to_event_log ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_below_threshold_discarded ... ok

test result: ok. 30 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.02s


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

- `CLAUDE.md` — +1 -1
- `docs/architecture.md` — +4 -1
- `docs/dev/milestones/M10-residual-hygiene/README.md` — +1 -1
- `docs/dev/milestones/M10-residual-hygiene/phase-03-document-reindex.md` — +7 -1
- `tests/doc_truth.rs` — +51 -0

**Commit:** 46389d334b9b4f43bc2b2826486f417359aa689a

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

### Review verdict — 2026-08-02

- **Verdict:** approved_first_try
- **Bounces:** none
- **Executor:** Qwen/Qwen3.6-27B-FP8
- **Scope deviations:** none — exactly the three files authorized, plus the
  phase doc's own Status line and Update Log.
- **Calibration:** none on the criteria — all nine were satisfiable and all nine
  landed. One cosmetic nit, below.

**Independently re-run:** `cargo fmt --all --check` clean, `cargo build` clean,
`cargo clippy --all-targets --all-features -- -D warnings` exit 0, and 1038 lib +
30 integration (2 ignored) + 9 isolation (1 ignored) + 6 bug_tracker + **2**
doc_truth. Lib is unchanged at 1038, as specified — this phase adds no lib tests.

| Criterion | Required | Measured |
|---|---|---|
| `doc_truth` tests | 2 | **2** |
| `daemoneye reindex` in `CLAUDE.md` | ≥ 1 | **1** |
| …in `architecture.md` **before** the roadmap | ≥ 1 | **1** |
| …in `architecture.md` whole file | ≥ 3 | **3** |
| `src/memory/index.rs` row is one line | 1 | **1** |
| `wc -l < CLAUDE.md` | 189 | **189** |
| `RETIRED_CLAIMS` intact | 2 | **2** |
| Hunks at/below `## 5. Milestone roadmap` (line 295) | none | **none** |
| `cargo test --lib` | 1038 | **1038** |

#### The gate does what a grep could not — verified three ways

| Mutation | Result |
|---|---|
| Remove the `CLAUDE.md` sentence | FAILED, naming `CLAUDE.md` |
| **Remove the durable `architecture.md` sentence, leaving both roadmap mentions** | **FAILED, naming `architecture.md`** |
| Rename `## 5. Milestone roadmap` → `## 5. Roadmap` | FAILED: `docs/architecture.md no longer contains "## 5. Milestone roadmap"` |

The second row is the whole point. With `daemoneye reindex` still appearing
**twice** in the file, the gate still fails — so it is genuinely reading only the
durable part, and a plain `grep -c` would have passed. The third confirms the
deliberate `panic!`: renaming the heading fails loudly instead of silently
widening the check to the whole file.

Both edits landed where the spec asked. The `CLAUDE.md` row grew in place and
still renders as one table row; the `architecture.md` sentence sits in § 2.3
Knowledge flow, immediately after the reconcile-on-empty clause it completes.

#### Nit — one unwrapped line, not worth a bounce

`docs/architecture.md:194` is **111 characters**; the surrounding paragraph wraps
at 70–79. The new sentence was appended without re-flowing the join into
`Recall merges three candidate sources — tag`. Purely cosmetic, invisible in
rendered Markdown, and the spec pinned behaviour rather than wrap width, so it is
a `nit` under `WORKFLOW.md` § "Severity meanings" and not grounds to bounce the
milestone's last phase. **The architect will reflow it at milestone close**, when
§ 5 is rewritten anyway — not fixed here, because reviewing is not executing.

#### Process note — the E2E transcript came back

Phases 01 and 02 summarised the end-to-end block instead of pasting it, which was
recorded as a two-occurrence trend heading for a fold. **This phase pasted the
literal transcript** (146 lines of Update Log). The trend does not advance to
three; it is reset to a single unaddressed instance in phase 02, and no fold is
warranted.

The executor's own Update Log entry again self-reported the wrong model
(`claude-opus-4-5-20251101`); it is Qwen3.6-27B-FP8. Corrected in place. That
mislabel is now a **third** occurrence and does qualify for a fold — recorded here
for the milestone close to decide, since folding `WORKFLOW.md` needs PE sign-off.

