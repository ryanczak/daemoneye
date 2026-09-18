# Phase 10b: retire the pane-toolkit memory and widen the prompt gate to every asset

**Milestone:** M22 — Tool surface & context
**Status:** todo
**Depends on:** 10a (the `prompt_assets_name_no_retired_tools` gate this phase
widens), 09a–09c (the six deleted tools), 08b (`get_shell_context`). All
**done**. **This is M22's last row.**
**Estimated diff:** ~260 lines, mostly deletion
**Tags:** language=rust, kind=removal+gate, size=m

## Goal

10a gated the two prompt assets. Widen that gate to **every seeded asset and
`docs/REQUIREMENTS.md`**, then fix everything it catches. Measured at drafting,
it catches exactly four files:

| File | Retired names it contains |
|---|---|
| `assets/memory/knowledge/tmux-pane-toolkit.md` | all seven, 19 occurrences |
| `assets/memory/session/pane-referencing-convention.md` | `read_pane`, `find_in_panes`, `tmux_control` |
| `assets/agents/sysadmin/config.toml` | `watch_pane` |
| `docs/REQUIREMENTS.md` | `list_panes`, `close_background_window`, `get_terminal_context` ×3 |

The seeded `sysadmin` agent telling its ghosts to call `watch_pane` is the same
live defect 10a fixed in the ghost prompt, in a file 10a was not authorized to
touch.

**Three different remedies, and which applies is measured per file** — the
whole point of this phase is that "mentions a pane" is not the test:

- **Delete** the pane-toolkit memory. Its stated purpose is choosing among the
  deleted tools.
- **Edit** the pane-referencing convention. Most of it is still true and
  load-bearing; only two bullets are about deleted tools.
- **Correct** the requirements doc, without deleting a requirement.

## Architecture references

- `CLAUDE.md` § "Current AI tools" — the 42 that exist.
- 10a's `prompt_assets_name_no_retired_tools` in `tests/doc_truth.rs` — the
  gate being widened, and the shape to follow.

## Pre-flight

Run these and confirm, at HEAD `effd4e69`:

```sh
git rev-parse --short HEAD                                       # effd4e69
grep -c 'watch_pane' assets/agents/sysadmin/config.toml          # 1
ls assets/memory/knowledge/tmux-pane-toolkit.md                  # exists
grep -c 'get_terminal_context' docs/REQUIREMENTS.md              # 3
cargo test --test doc_truth | tail -3                            # 12 passed
cargo test --lib | tail -3                                       # 1885 passed, 4 ignored
```

## Current state

### The retired-name census, measured

Run against `assets/**` and `docs/REQUIREMENTS.md` at drafting. **No other file
under `assets/` contains any of the seven names** — the remaining pane and tmux
words in `ghost-shell-guide.md`, `agent-runtime-layout.md`, `webhook-setup.md`,
`scheduling-guide.md` and `assets/etc/config.toml` describe things that still
exist (the `de-gs-bg-*` windows, `var/log/panes/`, `var/log/pipe/`, the
`backend = "tmux"` opt-out, `read_file`'s surviving `target_pane`). **Leave all
of them alone.**

### 1. `assets/memory/knowledge/tmux-pane-toolkit.md` — delete

92 lines, six sections: the cache model, pane status, **"Which tool"**, costs
and limits, what is refused. Its own frontmatter states the purpose:

```
tags: [daemoneye, tmux, panes, windows, sessions, read_pane, find_in_panes, tmux_control, list_panes, status, cache]
summary: How DaemonEye sees tmux — the multi-session pane cache, what is live vs cached, pane status classification, and which tool to reach for
```

Four of those tags are deleted tools. The file teaches a model to choose among
tools that no longer exist, so it goes.

Its constant and call sites, **all four** — the build breaks if any is missed:

```
src/config/seeds.rs:67    seed_knowledge_memory("tmux-pane-toolkit", TMUX_PANE_TOOLKIT_MEMORY)?;
src/config/seeds.rs:197   "tmux-pane-toolkit",            (inside overwrite_knowledge_memories)
src/config/seeds.rs:248   pub(crate) const TMUX_PANE_TOOLKIT_MEMORY: &str = include_str!(…);
src/config/path_audit.rs:657   ("tmux-pane-toolkit", TMUX_PANE_TOOLKIT_MEMORY),   (in a test's asset table)
```

**`src/cli/commands/setup.rs` needs no edit.** Its `seeded_knowledge` display
array already lists only seven keys and **already omits** `tmux-pane-toolkit` —
a pre-existing inconsistency with `seeds.rs`, which seeds eight. Retiring the
memory makes the two agree. Do **not** "fix" that array by adding the key.

### 2. `assets/memory/session/pane-referencing-convention.md` — edit, do not delete

Eleven lines. Most are still true: the `[PANE MAP]` block is still emitted
(M23 owns it), foreign-session panes still exist and still are not valid
foreground targets, pane `status:` is still classified, and the
"pane index N in 'window'" convention is still how the model should speak to
the user. **Exactly two bullets are about deleted tools** and only those two go:

```
- **Look before asking.** If you need output the context block only summarises, read it: `read_pane` for a known pane, `find_in_panes` when you don't yet know which pane holds it. Ask the user to paste output only after those have failed.
```

```
- **Moving the user is an action, not a courtesy.** `tmux_control` (focus/zoom/split/rename/kill) changes what the user is looking at and is approval-gated. Prefer telling them where to look; use the tool when being taken there is genuinely more useful.
```

Delete those two lines. **Change nothing else in the file**, and keep the file,
its `seeds.rs` constant and both its seeding call sites.

### 3. `assets/agents/sysadmin/config.toml` — one line

```
4. Use background mode for long-running commands and watch_pane to monitor completion.
```

Two defects in one sentence: `watch_pane` was deleted, and "background mode" is
the parameter spelling phase 07b replaced with `wait`.

### 4. `docs/REQUIREMENTS.md` — correct, do not delete a requirement

Five references across three requirements:

- Line 27, **FR-1.2.2** — names `get_terminal_context` as the tool the AI calls
  for a fresh snapshot.
- Line 46, the *Terminal Context* bullet — same tool, same fix.
- Line 47, the *Pane Discovery* bullet — mandates `list_panes`, which no longer
  exists.
- Line 33, the *Background mode* bullet — mentions `close_background_window`.
- Line 54, **FR-1.2.16** — names `get_terminal_context`.

**Deleting a requirement is a product decision and is not authorized here.** A
requirement that mandates a deleted tool gets marked superseded with its
replacement named, which is a statement of fact about the shipped product. The
exact form is in Task 4.

## Spec

### Task 1 — retire the pane-toolkit memory

Delete `assets/memory/knowledge/tmux-pane-toolkit.md` and all four code sites
listed in § Current state 1: the `seed_knowledge_memory` call, the
`seed_memory_inner` block inside `overwrite_knowledge_memories`, the
`TMUX_PANE_TOOLKIT_MEMORY` constant, and the `path_audit.rs` test-table entry.

The `overwrite_knowledge_memories` block to remove looks like this — remove the
whole `seed_memory_inner(…)?;` call, not just the constant name:

```rust
    seed_memory_inner(
        "knowledge",
        "tmux-pane-toolkit",
        TMUX_PANE_TOOLKIT_MEMORY,
        true,
    )?;
```

Do not touch `src/cli/commands/setup.rs`.

**Existing installs keep their seeded copy.** Seeding writes on first run only,
and nothing in this codebase deletes a seeded memory. Retiring the asset stops
*new* installs from getting it; it does not reach back. Do **not** add a
deletion path — that is a migration question, not this phase's. Say in the
Update Log that you left existing installs alone.

### Task 2 — trim the pane-referencing convention

Delete exactly the two bullets quoted in § Current state 2. Keep every other
line, the file itself, its constant and its two seeding call sites.

### Task 3 — fix the sysadmin agent line

Replace that line with:

```
4. Use `wait: false` for long-running commands and wait for the [Background Task Completed] message before acting on the result.
```

This matches the rule 10a wrote into `assets/prompts/ghost-shell.txt`: an agent
config drives ghost sessions, whose background commands run in a tmux window
with no shell id, so there is no watch tool to name.

### Task 4 — correct `docs/REQUIREMENTS.md`

Rename all three `get_terminal_context` occurrences to `get_shell_context`. In
FR-1.2.2 and FR-1.2.16 that is the only change.

For the two bullets that mandate a deleted tool, append a superseded note in
this form, and change nothing else in the bullet:

```
**Superseded (M22):** `list_panes` was removed in M22; `list_shells` and
`get_shell_context` provide this capability over the PTY shell substrate.
```

```
**Superseded (M22):** `close_background_window` was removed in M22; a finished
background window is reclaimed by the daemon's own sweep, so there is nothing
to close.
```

Do not delete, renumber or reword the requirements themselves.

### Task 5 — widen the gate

Extend `tests/doc_truth.rs`. Keep `prompt_assets_name_no_retired_tools` as it
is — it pins the two prompt assets by `include_str!`, which is the right shape
for the bytes that get seeded — and add a **second, broader** test named
`seeded_assets_name_no_retired_tools` that:

- Walks `concat!(env!("CARGO_MANIFEST_DIR"), "/assets")` recursively at runtime
  and reads every file, rather than naming each asset with `include_str!`. A
  directory walk covers an asset added later **without editing this test**,
  which is the property that matters; the 10a test already covers the
  seeded-bytes property for the two files where it is load-bearing.
- Also reads `docs/REQUIREMENTS.md` from the same manifest root.
- Reuses 10a's existing `RETIRED_TOOL_NAMES` constant. Do not define a second
  copy of that list.
- Collects **every** violation across all files and fails once with all of them
  listed as `<relative path>: <name>`, rather than panicking on the first. A
  gate that reports one file at a time costs a run per violation.
- **Negative pin:** it must **not** assert on the bare words `pane` or `tmux`.
  Nine asset files legitimately contain them — `ghost-shell-guide.md`,
  `agent-runtime-layout.md`, `webhook-setup.md`, `scheduling-guide.md`,
  `assets/etc/config.toml`, both prompts, the surviving session memory and the
  sysadmin agent — and a blanket ban would fail on correct prose and have to be
  weakened later. Assert on **tool names**, which is the property that can be
  wrong.
- Must **not** skip the superseded notes Task 4 adds. Those notes name
  `list_panes` and `close_background_window` deliberately, so
  `docs/REQUIREMENTS.md` **is expected to still contain them** after Task 4.
  Handle this by checking the requirements doc against the retired list **minus
  those two**, and say so in a comment. Every asset under `assets/` is checked
  against the full list. Do not add a blanket allowlist or a magic-string skip —
  the shape must make the exemption visible and narrow.

### Task 6 — end-to-end capture

Add `e2e_seeded_asset_tool_hygiene` to `tests/doc_truth.rs`, printing one line
and asserting each fact it prints:

```
retired_in_assets=0 asset_files_scanned=15 toolkit_memory_gone=true sysadmin_watch_pane=0
```

- `retired_in_assets` — total retired-name occurrences across `assets/**`, 0.
- `asset_files_scanned` — the number of files the walk read under `assets/`.
  **16 today, 15 after Task 1 deletes one.** Assert the value; if your walk
  sees a different number, do not change the assertion to match — find out why
  and say so in the Update Log.
- `toolkit_memory_gone` — `assets/memory/knowledge/tmux-pane-toolkit.md` does
  not exist, `true`.
- `sysadmin_watch_pane` — occurrences of `watch_pane` in the sysadmin agent
  config, 0.

Capture it with:

```sh
cargo test --test doc_truth e2e_seeded_asset_tool_hygiene -- --nocapture \
  | sed -n '/PASTE MATCH >>>/,/<<</p' > /tmp/e2e-10b.txt
```

Paste the captured block into an Update Log entry titled
`### Update — <date> (end-to-end verification)`, **as that entry's first fenced
block**, before the mutation output — the self-check below extracts the first
fence after the heading:

```sh
D=docs/dev/milestones/M22-tool-surface/phase-10b-prompt-assets-seeded.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-10b.txt
diff /tmp/pasted-10b.txt /tmp/e2e-10b.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

The literal `PASTE MATCH` line goes **into** the entry. The server-authored
`(complete)` entry does **not** satisfy this.

### Task 7 — mutation pair

Run both, paste all four `test result:` lines into the Update Log **after** the
capture block.

**Mutation A — a retired name in a non-prompt asset.** Put `watch_pane` back in
`assets/agents/sysadmin/config.toml`. `seeded_assets_name_no_retired_tools`
must go **red**, and its message must name both the file and the token. This is
the mutation that proves the widening did something: 10a's narrower gate stays
green here, because it never reads the agent config.

**Mutation B — the walk stops seeing a file.** Add an early `continue` that
skips `.toml` files in the walk. `e2e_seeded_asset_tool_hygiene` must go **red**
on `asset_files_scanned`. This proves the count is pinning the walk rather than
being decorative.

If either mutation leaves its test green, **stop and say so in the Update
Log** — the gate is not pinning the property.

## Acceptance criteria

Each was run against HEAD `effd4e69` and **fails** there; "now" is the measured
current value.

- [ ] `ls assets/memory/knowledge/tmux-pane-toolkit.md` → **does not exist** (now exists)
- [ ] `grep -rc 'TMUX_PANE_TOOLKIT_MEMORY' src/ --include=*.rs | grep -v ':0' | wc -l` → **0** (now 2 — `seeds.rs` and `path_audit.rs`)
- [ ] `grep -c 'tmux-pane-toolkit' src/config/seeds.rs` → **0** (now **3** — the two call sites and the `include_str!` path)
- [ ] `grep -c 'watch_pane' assets/agents/sysadmin/config.toml` → **0** (now 1)
- [ ] `grep -c 'background mode' assets/agents/sysadmin/config.toml` → **0** (now 1)
- [ ] `grep -cE 'read_pane|find_in_panes|tmux_control' assets/memory/session/pane-referencing-convention.md` → **0** (now **2** — `grep -c` counts matching *lines*, and the three names sit on two bullets)
- [ ] `wc -l < assets/memory/session/pane-referencing-convention.md` → **9** (now 11) — exactly two bullets removed
- [ ] `grep -c 'get_terminal_context' docs/REQUIREMENTS.md` → **0** (now 3)
- [ ] `grep -c 'get_shell_context' docs/REQUIREMENTS.md` → **3** (now 0)
- [ ] `grep -c 'Superseded (M22)' docs/REQUIREMENTS.md` → **2** (now 0)
- [ ] `grep -c 'fn seeded_assets_name_no_retired_tools' tests/doc_truth.rs` → **1** (now 0)
- [ ] `grep -c 'fn e2e_seeded_asset_tool_hygiene' tests/doc_truth.rs` → **1** (now 0)
- [ ] `cargo test --test doc_truth` reports **14 passed** (now 12)
- [ ] Task 6's `PASTE MATCH` block reads exactly `retired_in_assets=0 asset_files_scanned=15 toolkit_memory_gone=true sysadmin_watch_pane=0`
- [ ] Task 7's mutation pair, all four `test result:` lines, in the Update Log
- [ ] `cargo fmt --all` clean, `cargo build` clean, `cargo clippy --all-targets --all-features -- -D warnings` clean

**Must-survive criteria** — these pass now and must still pass after. The first
five are the accurate pane prose this phase must **not** sweep up.

- [ ] `grep -c 'PANE MAP' assets/memory/session/pane-referencing-convention.md` → **2, unchanged** — the block is still emitted, and neither mention is on a deleted bullet
- [ ] `grep -c 'de-gs-bg-\*' assets/memory/knowledge/ghost-shell-guide.md` → **2, unchanged**
- [ ] `grep -c 'var/log/panes' assets/memory/knowledge/agent-runtime-layout.md` → **1, unchanged** — the directory exists
- [ ] `grep -c 'target_pane' assets/memory/knowledge/agent-runtime-layout.md` → **1, unchanged** — `read_file` keeps it until M21
- [ ] `grep -ci 'tmux' assets/etc/config.toml` → **3, unchanged** — the `backend = "tmux"` opt-out is real config
- [ ] `grep -c 'pane-referencing-convention' src/config/seeds.rs` → **3, unchanged** — the two call sites and the `include_str!` path; the memory stays seeded
- [ ] `grep -c 'seeded_knowledge' src/cli/commands/setup.rs` → **at least 1**, and `grep -c 'tmux-pane-toolkit' src/cli/commands/setup.rs` → **0, unchanged** — that array already omitted the key; do not add it
- [ ] `grep -c '^        name: "' src/ai/tools/defs.rs` → **42, unchanged**; `grep -c 'deferred_group: None' src/ai/tools/defs.rs` → **28, unchanged**
- [ ] `cargo test --lib` reports **1885 passed, 4 ignored, unchanged** — the new tests are integration-level
- [ ] `git diff --stat effd4e69..HEAD -- src/ai/ src/ipc.rs src/cli/ assets/prompts/` is **empty** — no tool, wire, renderer or prompt-asset changes here; 10a owns the prompts
- [ ] `grep -c 'RETIRED_TOOL_NAMES' tests/doc_truth.rs` → **at least 3** (now 3: the definition and 10a's two uses) — the new tests reuse the list rather than redefining it

## Test plan

**Added (2), both in `tests/doc_truth.rs`:**
`seeded_assets_name_no_retired_tools` (Task 5) and
`e2e_seeded_asset_tool_hygiene` (Task 6). `doc_truth` goes 12 → **14**.

**`cargo test --lib` does not move** — it stays at 1885. Deleting the memory
constant removes no test. If the number changes, say so in the Update Log.

**Expected removals: none.** `path_audit.rs`'s
`all_assets_audit_clean_with_empty_quarantine` loses one row from its asset
table but stays a test — that is a **row edit, not a test deletion**. If you
find yourself deleting a test, stop and say so.

## End-to-end verification

The Task 6 `PASTE MATCH` block **as the entry's first fenced block**, then the
Task 7 mutation pairs, in an Update Log entry titled
`### Update — <date> (end-to-end verification)`. Per `STANDARDS.md` §1 the
server-authored `(complete)` entry does **not** satisfy this.

## Authorizations

Edits are authorized in exactly these files:

- `assets/memory/knowledge/tmux-pane-toolkit.md` — **delete**
- `assets/memory/session/pane-referencing-convention.md` — the two bullets
  **only**
- `assets/agents/sysadmin/config.toml` — the one line **only**
- `docs/REQUIREMENTS.md` — the five references **only**
- `src/config/seeds.rs` — the `TMUX_PANE_TOOLKIT_MEMORY` constant and its two
  call sites **only**
- `src/config/path_audit.rs` — the one test-table row **only**
- `tests/doc_truth.rs` — the two new tests **only**
- This phase doc's Update Log.

**Not authorized:** `src/cli/`, `src/ai/`, `src/ipc.rs`, `assets/prompts/`,
every other file under `assets/memory/` and `assets/agents/`, `CLAUDE.md`,
`README.md`.

**If the lint gate reports a `dead_code` or `unused_imports` orphan this
closure missed** — likely, since a deleted constant can orphan an import — do
**not** mask it with `#[allow]`, and do **not** attribute a decision to anyone.
Name it in the Update Log, then: if it lives in an **authorized** file, delete
it and record that you did so; if it lives in an **unauthorized** file, either
stop with a blocker entry, **or** make the deletion and label it plainly as
**your own** unauthorized-but-forced decision with the evidence. **State which
you chose and that it was your decision.** There is no human in the loop
mid-run.

## Out of scope

- **Deleting an already-seeded memory from an existing install.** Retiring the
  asset changes what new installs get. A migration that removes a stale memory
  from a live `~/.daemoneye/` is a separate decision with data-loss risk.
- **The `[PANE MAP]` / `[FOREGROUND TARGET]` context blocks** — M23.
- **`target_pane` on `read_file` / `edit_file`** — M21's `host?`.
- **Rewriting or deleting any requirement in `docs/REQUIREMENTS.md`.** Task 4
  corrects tool names and marks two capabilities superseded. Re-deciding what
  the product must do is the PE's call, not a phase's.
- **The remaining pane and tmux words in the other assets.** Measured at
  drafting: every one of them describes something that still exists. Do not
  touch them, and do not extend the gate to ban the words.

## Update Log

<!-- entries appended below this line -->
