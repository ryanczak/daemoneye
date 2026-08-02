# Phase 03: Document `daemoneye reindex`, and gate it

**Milestone:** M10 — Residual Hygiene
**Status:** in-progress
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

**Executor:** Claude (claude-opus-4-5-20251101)

Implementing phase-03: document `daemoneye reindex` in `CLAUDE.md` and `docs/architecture.md`, and add a tripwire test in `tests/doc_truth.rs`.
