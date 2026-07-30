# Phase 04: `daemoneye audit-prompts`

**Milestone:** M6 — Verification & Hygiene
**Status:** todo
**Depends on:** phase-02 (done), phase-03 (done)
**Estimated diff:** ~350 lines
**Tags:** language=rust, kind=feature, size=m

## Goal

Ship the operator-facing `daemoneye audit-prompts`: it reads the **installed**
prompt and knowledge memories from `~/.daemoneye/`, classifies every path literal
they assert as *current* / *superseded* / *unknown* against the phase-02
inventory, prints a report, and exits non-zero if anything is not current.

It **never writes**. That is the whole contract.

## Architecture references

Read before starting:

- `src/config/path_audit.rs` — the extractor, `INVENTORY`, and `audit_text`. You
  **reuse** this. Growing a second extractor is a scope violation.
- `docs/dev/milestones/M6-verification-and-hygiene/README.md` § "Defect
  inventory" item 6 — why this command exists and why it must not rewrite.
- `src/cli/commands/costs.rs` — the shape to follow for a read-only report
  command that works with the daemon down.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read `src/config/path_audit.rs` in full.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is clean and `cargo test` is green at 955 lib tests.

## Current state

**Phase 02 gave you the machinery; it is not yet enough on its own.**
`audit_text` returns `Vec<Finding>` — only the *bad* literals. This command must
report **every** literal with its status, including the good ones, so the
operator can see what was checked. That gap is task 1.

**The command audits INSTALLED files, not the embedded assets.** This is the
point of defect 6: `overwrite_sre_prompt()` / `overwrite_knowledge_memories()`
(`src/config/seeds.rs:147`, `:103`) are only ever called from
`src/cli/commands/setup.rs`, and first-run seeding is `if !exists`. An install
predating a change keeps the stale copy forever and nothing says so. Auditing the
`include_str!` consts would therefore audit the wrong thing — it would always
pass, and tell the operator nothing about their own tree.

Installed locations, verified against source:

| What | Path | Constructor |
|---|---|---|
| prompt | `~/.daemoneye/etc/prompts/sre.toml` | `prompts_dir().join("sre.toml")` (`seeds.rs:148`) |
| knowledge memories | `~/.daemoneye/memory/knowledge/` | `config_dir().join("memory").join("knowledge")` (`seed_memory_inner`, `seeds.rs:76`) |

Read the seeded filename convention out of `seed_memory_inner` rather than
assuming an extension.

**CLI shape.** Subcommands are variants of `enum Commands` in `src/main.rs`
(`:14`), dispatched in the `match` around `:358` to a `cli::run_*` function.
`Commands::Prompts => cli::run_prompts()?` is the minimal example;
`Commands::Costs` is the closest report-shaped one.

## Spec

### 1. `classify_text` in `src/config/path_audit.rs`

Add a function returning **every** extracted literal with its classification —
not just the bad ones. Reuse `extract_path_literals` + `normalise` + `INVENTORY`;
do not re-implement any of them.

Three outcomes, matching the milestone's exit criterion vocabulary:

- **Current** — normalises to an `INVENTORY` entry with `PathStatus::Current`.
- **Superseded** — normalises to an entry with `PathStatus::Legacy`, carrying the
  reason string.
- **Unknown** — normalises to something not in `INVENTORY`.

Literals that normalise to `None` (the bare runtime root) are always valid and
must not appear as findings; whether you list them as Current or omit them is
yours, but be consistent and say which in the doc comment.

Express `audit_text_with` in terms of this, or leave it alone — your call, as
long as there is exactly one extraction path and the existing tests still pass
unchanged.

### 2. `src/cli/commands/audit_prompts.rs`

A read-only report. No daemon round-trip — read the files directly, like
`costs.rs`, so it works with the daemon down.

For each installed asset (the prompt, then each knowledge memory), print the
asset's name and each path literal with its status. Superseded entries must show
the reason from `INVENTORY`. End with a summary line giving the totals per
status.

**Behavior to pin (rendering is yours):**

- **Never write, create, or modify any file** — no directory creation, no
  seeding, no "helpfully" refreshing a stale copy. This is the command's entire
  contract and the reason the PE ruled out auto-refresh: local prompt edits
  belong to the operator.
- **Exits non-zero when any literal is Superseded or Unknown**, zero when all are
  Current. The full report must be printed before exiting, and a non-zero exit
  must not look like a crash — no panic, no `anyhow` backtrace dump.
- **A missing installed file or missing knowledge directory is reported plainly
  and exits non-zero.** It must not panic and must not create anything. An
  operator who has not run `daemoneye setup` should get a clear "not installed"
  line, not a stack trace.
- Reading a file that is not valid UTF-8, or a knowledge directory containing
  unexpected files, must not panic.

### 3. Wire it up

Add the `Commands` variant in `src/main.rs` with a doc comment that renders as
useful `--help` text, and dispatch it to the new `cli::run_audit_prompts`.
Export from `src/cli/commands/mod.rs` following the existing pattern.

Name the subcommand `audit-prompts`.

## Acceptance criteria

- [ ] `daemoneye audit-prompts` prints a per-asset report classifying every path
      literal as current / superseded / unknown.
- [ ] It exits **0** against a tree seeded from the current (phase-03-corrected)
      assets, and **non-zero** against a tree whose installed prompt contains a
      superseded path.
- [ ] It creates and modifies **nothing** — proven by comparing a recursive
      listing with mtimes of the throwaway `~/.daemoneye/` before and after.
- [ ] A missing installed prompt is reported without panicking and exits
      non-zero.
- [ ] `classify_text` reports Current literals, not only findings.
- [ ] No second path extractor exists in the tree.
- [ ] All four gates green.

## Test plan

**Tests that touch `HOME` must take `crate::test_home_guard()`** (`src/lib.rs:45`)
— **not** the raw `TEST_HOME_LOCK` (`:32`), which panics for every later
HOME-dependent test in the binary once one poisons it. The accessor recovers.
Edition 2024, so `std::env::set_var` needs an `unsafe` block. Working RAII idiom
to copy: `src/daemon/context/recall.rs:259`.

Cover:

- `classify_text` returns Current for a good literal, Superseded (with reason)
  for `var/log/events.jsonl`, Unknown for an invented one.
- The command exits 0 on a clean seeded tree and non-zero on a tree with a
  superseded literal injected into the installed prompt.
- The no-write property: snapshot the tree (paths + mtimes) before and after,
  assert equality.
- Missing prompt file → non-zero, no panic.

**Do not pin a test count in advance.** Report the resulting count in the Update
Log and explain the delta.

## End-to-end verification

Quote in the Update Log, from a **real run** — each command run once, its actual
stdout pasted, nothing retyped or reconstructed:

1. `daemoneye audit-prompts` against a throwaway `HOME` seeded from the current
   assets: the full report and `echo $?` showing `0`.
2. The same against a tree where one installed literal was edited to a superseded
   path: the full report and `echo $?` showing non-zero.
3. The before/after tree listing proving nothing was written.

**This requirement has bounced twice on phase 03** — once for paraphrasing
instead of quoting, once for a transcript in which 24 of 25 lines were real and
one was spliced in from another file. Redirect each command's output to a file
and paste that file's contents. Do not reconstruct a transcript by hand, and do
not copy any line from this doc or from a previous Update Log entry.

## Authorizations

- [ ] May add `src/cli/commands/audit_prompts.rs` and export it from
      `src/cli/commands/mod.rs`.
- [ ] May add one `Commands` variant and its dispatch arm in `src/main.rs`.
- [ ] May add `classify_text` (and any private helper it needs) to
      `src/config/path_audit.rs`.

No new dependencies. No changes to `docs/architecture.md`.

## Out of scope

- **Do not rewrite, refresh, or repair the operator's installed files** — not
  behind a flag, not with a prompt, not at all. Report only.
- **Do not modify `overwrite_sre_prompt()` / `overwrite_knowledge_memories()`**
  or anything in `src/cli/commands/setup.rs`.
- **Do not add a `--json` output mode.** Not required by the exit criterion;
  keep the phase small.
- **Do not widen `extract_path_literals`** to code fences or bare prose. Phase 03
  recorded that limitation deliberately.
- **Do not change `INVENTORY` contents** or reclassify `var/log/events.jsonl`.
- **Do not fix the stale `~/.daemoneye/daemon.log` help strings** in
  `src/main.rs:17` and `:30` (the real path is `var/log/daemon.log`). They are
  the same drift class and are noted for phase 11 — this phase audits assets,
  not the CLI's own help text.
- **Do not touch `src/pane_prefs.rs`** (defect 10). Phase 11.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->
