# Phase 03: Fix Stale Prompt Paths

**Milestone:** M6 — Verification & Hygiene
**Status:** todo
**Depends on:** phase-02 (done)
**Estimated diff:** ~150 lines
**Tags:** language=rust, kind=fix, size=s

## Goal

Empty `PENDING_FIX`. Correct every stale path literal in the shipped prompt and
knowledge memories so the phase-02 audit passes with **no quarantine at all**,
and fix the two stale spans the audit is structurally blind to.

Phase 02 built the gate and proved it fires. This phase makes the assets clean
and takes the scaffolding down.

## Architecture references

Read before starting:

- `src/config/path_audit.rs` — the gate you are satisfying. `INVENTORY` is the
  authority on what each path should be; `PENDING_FIX` is the list you empty.
- `docs/dev/milestones/M6-verification-and-hygiene/README.md` § "Defect
  inventory" items 4 and 5 — what these literals cost in practice.
- `docs/dev/WORKFLOW.md` § "Coverage claims are inadmissible without mutation
  proof" — task 4 depends on it.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read `src/config/path_audit.rs` in full — especially `normalise()` and the
   test module.
3. Read this entire phase doc before touching any file.
4. Confirm the repo is clean and `cargo test` is green at 956 lib tests.

## Current state

`PENDING_FIX` (`src/config/path_audit.rs:164`) holds exactly 7 normalised
literals. The audit passes only because those 7 are skipped. Nothing in the
assets has been corrected yet — phase 02 was forbidden from touching them.

**Every path constructor below was verified against source while drafting.**

| Stale literal | Correct replacement | Source of truth |
|---|---|---|
| `var/log/events.jsonl` | `var/log/events/events-<date>.jsonl` | `current_event_segment_path()`, `load.rs:79-84` (`events-%Y%m%d.jsonl`) |
| `~/.daemoneye/config.toml` | `~/.daemoneye/etc/config.toml` | `etc_dir()` |
| `~/.daemoneye/daemon.log` | `~/.daemoneye/var/log/daemon.log` | `default_log_path()`, `load.rs:50` |
| `~/.daemoneye/events.jsonl` | `~/.daemoneye/var/log/events/events-<date>.jsonl` | `events_dir()`, `load.rs:74` |
| `~/.daemoneye/pane_logs/` | `~/.daemoneye/var/log/panes/` | `pane_logs_dir()`, `load.rs:35` |
| `~/.daemoneye/schedules.json` | `~/.daemoneye/var/run/schedules.json` | `Config::schedules_path()` |
| `~/.daemoneye/sessions/ghost-<name>-<uuid>.jsonl` | `~/.daemoneye/var/log/sessions/ghost-<name>-<uuid>.jsonl` | `sessions_dir()` (`load.rs:92`) + `session.rs:180` |

**Note the last row.** The *filename* `ghost-<name>-<uuid>.jsonl` is **correct** —
`ghost.rs:185` builds `session_id` as `format!("ghost-{}-{}", alert_name, uuid)`
and `session.rs:180` joins `{id}.jsonl` onto `sessions_dir()`. Only the directory
is wrong. Do not "simplify" that filename to `<id>.jsonl`.

### The exact sites

Audited (backticked, currently quarantined):

- `assets/prompts/sre.toml:320`
- `assets/memory/knowledge/agent-runtime-layout.md:79`
- `assets/memory/knowledge/ghost-shell-guide.md` — lines 93, 99, 109, 116, 118
- `assets/memory/knowledge/scheduling-guide.md:77`
- `assets/memory/knowledge/webhook-setup.md` — lines 12, 104

**Unaudited — the gate cannot see these, and they are wrong anyway:**

- `assets/memory/knowledge/agent-runtime-layout.md` — the **ASCII directory tree**
  (the fenced block starting line 15) still shows `log/` containing a flat
  `events.jsonl`. It is inside a code fence, not backticks, so
  `extract_path_literals` never sees it.
- `assets/memory/knowledge/webhook-setup.md:24` — inside a fenced shell block:
  `grep -A5 '\[webhook\]' ~/.daemoneye/config.toml`. Same blindness.

This asymmetry is expected, not a bug in phase 02: the extractor is
backtick-delimited by design (task 2 of phase 02 pinned that, because a
"contains a slash" rule produces false failures on `/clear`, `/limits reset` and
shebangs). **Widening the extractor to code fences is out of scope** — record the
limitation, fix the two spans by hand.

## Spec

### 1. Correct the audited literals

Apply the replacement table to all nine audited sites. Preserve surrounding
prose, wrapping, and the `\`-continuation style in `sre.toml`. These are
documentation edits — do not restructure a sentence to fit a path.

`sre.toml:320` currently reads:

```
- `var/log/events.jsonl` — structured event log (prefer \
  `search_repository(kind:"events")`).
```

The parenthetical is already correct — `search_repository` *is* the right tool.
Only the path is wrong. Keep the advice, fix the path.

### 2. Correct the two unaudited spans

- Rewrite the ASCII tree's `log/` subtree in `agent-runtime-layout.md` so it
  matches the real layout: `events/` is a **directory** of dated segments, not a
  flat `events.jsonl`. Match the shape the rest of the tree already uses.
- Fix the `grep` command at `webhook-setup.md:24` to name
  `~/.daemoneye/etc/config.toml`.

### 3. Empty `PENDING_FIX`

`PENDING_FIX` becomes `&[]`. Keep the `static` and its doc comment — phase 04
and any future asset edit rely on the mechanism existing. Update the doc comment
to say the list is empty and that a non-empty entry is a temporary quarantine
owned by a named phase.

Do **not** delete any `INVENTORY` entry. `var/log/events.jsonl` stays in
`INVENTORY` as `Legacy` — that is what makes a future regression detectable.

### 4. Keep the red-run proof alive — the part most likely to go wrong

`red_run_is_reproducible` (phase 02) asserts that the unquarantined audit of the
real assets flags exactly the literals in `PENDING_FIX`. **Once you empty
`PENDING_FIX`, that test degenerates to `assert_eq!(empty, empty)` — it passes no
matter what the extractor does.** That is precisely the vacuous coverage this
milestone exists to eliminate, and it would be introduced *by* the fix.

Split the two properties the test was carrying:

1. **The assets are clean.** Assert `audit_text_with(asset, &[])` returns **no
   findings** for all 8 assets — with an empty quarantine, so it is the real
   property and not quarantine-shaped. This replaces the two existing
   `*_audits_clean_under_pending_fix` tests, whose names are now misleading.
2. **The extractor still detects the historical defects.** Freeze the 7 literals
   as a `const` synthetic corpus in the test module (a string containing each
   one backticked) and assert the audit flags exactly those 7. The corpus is
   test data, not shipped asset text, so it stays red-proof after the assets are
   clean.

Name them so the intent survives; the exact names are yours.

**Mutation-check property 2 before reporting:** break `normalise()` (e.g. make it
return `None` unconditionally), confirm the synthetic-corpus test **fails**,
revert, and state the result in the Update Log. A claimed mutation check is not
one — the review will redo it.

## Acceptance criteria

- [ ] `PENDING_FIX` is `&[]`.
- [ ] `audit_text` (with the standing, now-empty quarantine) returns no findings
      for `SRE_PROMPT_TOML` and all 7 knowledge memories.
- [ ] No `~/.daemoneye/`-prefixed literal naming a pre-`var/` location survives
      in any asset — audited or not. `grep -rn '~/\.daemoneye/' assets/` shows
      only paths that are correct today (e.g. `~/.daemoneye/scripts/`, the bare
      runtime root).
- [ ] The ASCII tree in `agent-runtime-layout.md` shows `events/` as a directory
      of dated segments.
- [ ] A test proves the extractor still flags all 7 historical literals from a
      frozen synthetic corpus.
- [ ] `INVENTORY` still contains `var/log/events.jsonl` as `Legacy`.
- [ ] All four gates green.

## Test plan

- Assets-clean test over all 8 assets, empty quarantine.
- Synthetic-corpus test over the 7 frozen literals.
- The phase-02 tests that are still meaningful (`extracts_real_path_spans`,
  `rejects_slash_commands_and_shebangs`, the two `normalisation_*` tests,
  `legacy_entry_is_reported`, `inventory_contains_all_config_constructors`) must
  survive unchanged. If you find yourself editing one, stop — that is a signal
  you changed behavior this phase does not authorize.

**Do not pin a test count in advance.** Two tests are replaced by two differently
scoped ones and one is added; report the resulting count in the Update Log and
explain any delta.

## End-to-end verification

Quote in the Update Log, from a real run:

1. `grep -rn '~/\.daemoneye/' assets/` — the full output, showing every surviving
   occurrence is a correct path.
2. The assets-clean test passing with an empty quarantine.
3. The mutation check from task 4: the broken-`normalise()` run showing the
   synthetic-corpus test failing, then green after revert.

## Authorizations

- [ ] May edit `assets/prompts/sre.toml` and `assets/memory/knowledge/*.md` —
      **path corrections only**. This is the one phase that may touch them.
- [ ] May edit `PENDING_FIX` and the test module in `src/config/path_audit.rs`.
- [ ] May replace the two `*_audits_clean_under_pending_fix` tests.

No new dependencies. No changes to `docs/architecture.md`.

## Out of scope

- **Do not widen `extract_path_literals` to code fences or bare prose.** The
  backtick rule is deliberate (phase 02 task 2). The two unaudited spans are
  fixed by hand here; whether the extractor should grow is a later decision.
- **Do not change `INVENTORY` contents**, add entries, or reclassify
  `var/log/events.jsonl` away from `Legacy`.
- **Do not build `daemoneye audit-prompts`.** That is phase 04.
- **Do not touch `overwrite_sre_prompt()` / `overwrite_knowledge_memories()`.**
  Refreshing installed copies is explicitly ruled out (README defect 6, PE
  constraint) — the remedy is an audit that reports, never a write.
- **Do not resolve the `lib/` question** (defect 8) even though the ASCII tree
  names it. Phase 11 decides.
- **Do not fix `src/pane_prefs.rs`'s stale doc comment** (defect 10). Phase 11.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->
