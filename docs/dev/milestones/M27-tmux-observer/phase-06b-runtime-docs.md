# Phase 06b: runtime-docs — what the model and the user read says tmux is optional

**Milestone:** M27 — tmux becomes an optional observer; 2.0 close
**Status:** in-progress
**Depends on:** phase-06
**Tags:** language=markdown, kind=docs, size=m

## Goal

Phases 01–06 removed every tmux write from the daemon. The text the model
and the user read still describes the old design. The seeded system prompt
(`assets/prompts/sre.toml`) tells the model that foreground commands go
through `send-keys` and background ones into `de-bg-*` windows. The seeded
memories describe `de-gs-*` ghost windows. README lists tmux as required
and documents `daemoneye notify`. CLAUDE.md's lifecycle and key-files
table describe deleted modules.

This phase makes that text true and adds a `tests/doc_truth.rs` gate so it
stays true. The design docs (`architecture.md`, `PRODUCT_DEFINITION.md`,
`REQUIREMENTS.md`, `security.md`) are phase 06c.

### What is true now (write from this)

> **This list was verified against the source by the architect on
> 2026-10-11. It is the source of truth for this phase. Do not open any
> file under `src/` to re-check it.** Run 1 spent its whole budget doing
> exactly that (75 `grep`s and 21 `sed` reads across `chat.rs`,
> `scheduled.rs`, `sbx.rs` and others) and never made an edit.

- **Commands.** Chat commands run in the session's PTY shell
  (`run_terminal_command` → `executor/shell_exec.rs`). `wait: false` runs
  in a dedicated PTY shell, or in the session's sbx sandbox when
  `[sandbox] enabled`. A ghost runs on a configured `[hosts.*]` host, in
  its sbx sandbox, or in a local PTY shell. A call with no PTY route is
  refused with the reason. Nothing is typed into a tmux pane or run in a
  tmux window.
- **tmux is optional and read-only.** Inside tmux, the prompt carries pane
  summaries for the user's session (capture and list only). Outside tmux,
  the context is the daemon's own shells. The daemon installs no hooks,
  starts no `pipe-pane`, and never creates a tmux session. It adopts the
  session it was launched in, or a configured `[daemon] tmux_session` that
  already exists.
- **Alerts** reach every attached `daemoneye chat` over its presence
  connection. The tmux `display-message` overlay is an optional extra.
- **Catch-up** fires when a chat client re-attaches (`chat --resume <id>`)
  after ≥ 30 s away.
- **Scheduled scripts** run in a scheduler-owned PTY shell.
  `ActionOn::Command` loads but refuses to fire.
- **`[execution] backend = "tmux"`** still loads, with a startup warning
  that the tmux backend was removed.
- **`daemoneye doctor`** reports host readiness (config, api key, shell,
  daemon, sandbox, hosts, tmux) and exits 1 only on a `FAIL`.
- **Leftover `de-*` windows** from an older daemon are still filtered out
  of context (`is_daemon_window`), and `daemoneye schedule windows` still
  lists them. That is the only remaining reason to mention them. Use the
  generic `de-*`, never a specific prefix.

## Spec

1. **Add the gate** — in `tests/doc_truth.rs`, add
   `runtime_docs_describe_no_tmux_execution`. It scans `CLAUDE.md`,
   `README.md` and every file under `assets/` (reuse `walk_files`). It
   fails, naming `file:line` for each hit, when any of these substrings
   appears: `send-keys`, `send_keys`, `de-bg-`, `de-gs-`, `de-sj-`,
   `pipe-pane`, `set-hook`, `pane-died`, `daemoneye notify`, `hook.rs`,
   `notify.rs`, `daemon/background`, `BG_DONE_TX`, `FG_HOOK_COUNTER`,
   ``into `tmux` ``, `into tmux`. Add it first and run it: today it names
   about 40 occurrences. That list is your work list for steps 2–5.
2. **Seeded prompts** — `assets/prompts/sre.toml` and
   `assets/prompts/ghost-shell.txt`. Rewrite the execution paragraphs
   from § What is true now. In the context-exclusion sentence, say
   "daemon-managed `de-*` windows". Keep every tool name and every other
   instruction unchanged. The existing `prompt_assets_name_no_retired_tools`
   and `e2e_prompt_asset_tool_hygiene` tests must stay green.
3. **Seeded memories** — `assets/memory/knowledge/ghost-shell-guide.md`,
   `scheduling-guide.md` and `agent-runtime-layout.md`. Replace the window
   tables and the `pipe/` line with where the work runs now.
4. **README** —
   - Make the tmux dependency row "optional". Remove the "Linux-specific
     tmux hooks" note from the Linux row.
   - Delete the `daemoneye notify` row and add a `daemoneye doctor` row.
   - Rewrite the `run_terminal_command` row, the ghost and scheduling
     prose, the troubleshooting `tmux list-windows … de-gs-` recipes, the
     `pipes/` line in the directory tree, and the
     `backend = "tmux"` sandbox paragraph (now: the value still loads with
     a warning).
5. **CLAUDE.md** —
   - Rewrite the Architecture Overview's first sentence and the
     Request/Response lifecycle steps 1–5 from § What is true now.
   - In the Key files table, delete the `hook.rs` and `background/` rows,
     fix the `ghost.rs` row (no windows), and add `src/cli/doctor.rs`.
   - Delete `BG_DONE_TX` and `FG_HOOK_COUNTER` from Global statics.
   - Rewrite the tmux window-name invariant as a note on legacy `de-*`
     windows.
   - Fix the Sandbox section's last bullet (no tmux route remains to
     refuse; an unresolved ghost `ssh_target` is refused in every mode).
6. **Evidence** — run § End-to-end verification once, paste it into the
   Update Log, then make one `docs:` commit.

### Gotchas

- Edit with `patch` or `write_file`, never `patch_lines`, and never python
  or `sed` through `bash`. These are long files.
- `tests/doc_truth.rs` already holds CLAUDE.md's and README's tool tables
  to the code (`claude_md_tools_table_*`, `readme_tools_*`,
  `readme_approval_markers_*`). Do not rename, reorder or drop a tool row.
  Rewrite only description text.
- Describe, do not narrate history. Write "commands run in PTY shells",
  not "commands used to run in tmux windows but now…". Phase numbers and
  milestone names do not belong in user docs.
- ` (x2)` at the end of a line of tool output is the output filter's
  marker for a repeated line. It is never in the file.

## Edit order (escalation, 2026-10-11) — read this before anything else

Your first edit lands within **10 turns**: it is step 1. You only ever
open the file you are about to edit. After each file, run the gate
(`cargo test --test doc_truth runtime_docs`) and move to the next file it
names.

1. **The gate.** Copy the structure of the existing
   `seeded_assets_name_no_retired_tools` test in `tests/doc_truth.rs`:
   same `walk_files(ASSETS_DIR…)` loop, plus `CLAUDE.md` and `README.md`
   read from the repo root, with the retired list from Spec 1 in place of
   `RETIRED_TOOL_NAMES`, and one violation line per hit as
   `<file>:<line>: <term>`. Run it. It must fail.
2. **Work list.** Today's hits, by file (occurrences of each term):

   | File | Terms |
   |---|---|
   | `assets/prompts/sre.toml` | `send-keys` ×2, `de-bg-` ×2, `de-gs-` ×2, `de-sj-` ×1 |
   | `assets/prompts/ghost-shell.txt` | `de-gs-` ×1 |
   | `assets/memory/knowledge/ghost-shell-guide.md` | `de-gs-` ×2 |
   | `assets/memory/knowledge/scheduling-guide.md` | `de-gs-` ×1, `de-sj-` ×1 |
   | `assets/memory/knowledge/agent-runtime-layout.md` | `pipe-pane` ×1 |
   | `README.md` | `de-gs-` ×4, `de-bg-` ×1, `de-sj-` ×1, `pipe-pane` ×1, `daemoneye notify` ×1 |
   | `CLAUDE.md` | `de-gs-` ×6, `hook.rs` ×4, `de-bg-` ×3, `de-sj-` ×1, `send-keys` ×1, `send_keys` ×1, `pane-died` ×1, `BG_DONE_TX` ×1, `FG_HOOK_COUNTER` ×1, `daemon/background` ×1, ``into `tmux` `` ×1 |

   Do the files in this order. For each, read it, rewrite each flagged
   passage from § What is true now in one `patch`, and re-run the gate.
3. **The non-term items in Spec 4 and 5**: the README doctor row and
   "Required for hook support", and CLAUDE.md's `src/cli/doctor.rs` row
   and lifecycle steps 1–5. The acceptance greps tell you when each is
   done.
4. Full `cargo test`, clippy, fmt, § End-to-end verification once, commit.

## Tests

- `runtime_docs_describe_no_tmux_execution` (`tests/doc_truth.rs`): fails
  on today's tree naming every hit, and passes after steps 2–5. Negative
  control: the generic `de-*` (as in the `schedule windows` row) must
  **not** trip it.

## Acceptance criteria

- [ ] `cargo test --test doc_truth runtime_docs_describe_no_tmux_execution`
      reports `1 passed`. Today the test does not exist.
- [ ] `grep -c 'daemoneye doctor' README.md` prints at least `1`, and
      `grep -c 'src/cli/doctor.rs' CLAUDE.md` prints at least `1`. Today
      both print `0`.
- [ ] `grep -n 'Required for hook support' README.md` prints nothing.
      Today it prints one line.
- [ ] *Preservation:* every other `tests/doc_truth.rs` test still passes.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` exits 0;
      `cargo fmt --all --check` exits 0.
- [ ] `cargo test` reports exactly `passed=2308 failed=0`: today's 2307
      plus the one new test. If you add another test, name it and say why.
- [ ] The Update Log contains the § End-to-end verification output, every
      line as printed, and a line `PASTE MATCH: yes`.

## End-to-end verification

```sh
cargo test --test doc_truth 2>&1 | grep -E '^test result'
grep -c 'daemoneye doctor' README.md
grep -c 'src/cli/doctor.rs' CLAUDE.md
grep -n 'Required for hook support' README.md | wc -l
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1
cargo fmt --all --check; echo "fmt_exit=$?"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
```

Each line runs **once**.

## Authorizations

- [x] May edit `CLAUDE.md`, `README.md`, `tests/doc_truth.rs`, and any
      file under `assets/prompts/` and `assets/memory/knowledge/`.

## Out of scope

- `docs/architecture.md`, `docs/PRODUCT_DEFINITION.md`,
  `docs/REQUIREMENTS.md`, `docs/security.md`. Those are phase 06c.
- Any `src/` change. If the text and the code disagree, the code is right.
  Note the disagreement in the Update Log.
- Prompts already seeded into a user's `~/.daemoneye/`. Only the repo's
  `assets/` change.

## Update Log

<!-- entries appended below this line -->

### Notes for executor — 2026-10-11

Run 1 stalled at turn 135 without a single edit outside this doc. It read
§ What is true now and then tried to verify every bullet against `src/`.
Those facts were already verified by the architect, so that work was
wasted. The refinement marks them as authoritative, forbids opening
`src/`, pins the first edit to step 1 within 10 turns, names the existing
test to copy for the gate, and lists today's hits per file so nothing
needs to be searched for. Your Update Log entry names the executor model
from your own configuration; run 1 wrote a wrong one.

### Update — 2026-10-11 (escalation)

**Chosen lever:** refined re-dispatch.
**Rationale:** NoProgressStall at turn 135 (105 `bash`, 26 `read_file`,
zero edits to the target files). The spec gave facts to write from but
did not say they were authoritative, so the executor re-derived them from
the source. A spec gap, not a model limit, and the first failure on this
phase.
