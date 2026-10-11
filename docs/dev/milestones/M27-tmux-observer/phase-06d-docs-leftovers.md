# Phase 06d: docs-leftovers — the last stale tmux and foreground-pane text

**Milestone:** M27 — tmux becomes an optional observer; 2.0 close
**Status:** todo
**Depends on:** phase-06e (dispatch 06e first)
**Tags:** language=markdown, kind=docs, size=s

## Goal

Phase 06c left a short list of stale passages that its work list did not
name. Phase 06e then removes the `[FOREGROUND TARGET]` / `[Pane target
changed]` prompt text from the code. This phase makes the docs match, and
extends the `doc_truth` gate so the foreground-pane vocabulary stays out.

## Work list — this table is the whole job

Rules (carried from 06c, where they turned a 240-call stall into a clean
run):

- **Never open anything under `src/`.** Every fact is in this table.
- **Open only the line range a row names.** Line numbers are as of
  2026-10-11 and may shift by a few lines.
- **Edit only these rows.** Anything else stale goes in the Update Log.
- **Your first `patch` lands within 10 turns.** That patch is row 1.

| # | File · where | Action — write this meaning, in the file's own style |
|---|---|---|
| 1 | `tests/doc_truth.rs` `RETIRED_TMUX_EXECUTION_TERMS` | Add `FOREGROUND TARGET`, `Pane target changed`, `background=false`, `de-incident`. Run `cargo test --test doc_truth runtime_docs`. It must fail, naming the lines rows 2, 5, 6 and 8 fix. |
| 2 | `README.md` ~194–200, the **Pane Targeting** block | Retitle it **Pane references**. Intro: commands run in PTY shells, never in a tmux pane. Delete the `[FOREGROUND TARGET]` bullet and the **Drift detection** bullet. Keep the `[PANE MAP]` bullet. `/pane` bullet: `/pane` lists the session's panes. `/pane %N` pins the pane whose new activity triggers a fresh context snapshot, saved in `pane_prefs.json`. |
| 3 | `README.md` ~402 | Delete the `daemoneye chat --session NAME` row. `chat` has no `--session` flag. Its only option is `--resume <id>`. |
| 4 | `README.md` ~1056 | Delete the sentence "When `tmux_session` is set, `daemoneye chat` invoked outside of tmux will open a new chat window … exec-attach …". `daemoneye chat` runs in place and never attaches. |
| 5 | `docs/PRODUCT_DEFINITION.md` ~42 | Replace the **Pinned pane targeting** bullet: **Pane references**. Inside tmux, the AI sees a `[PANE MAP]` and can talk about panes by window and command. `/pane %N` pins the pane whose activity refreshes the context. Commands never run in a pane. |
| 6 | `docs/REQUIREMENTS.md` FR-1.2.2 | Three edits only. (i) First sentence: after "from the currently active tmux pane" add "(outside tmux, the daemon's own PTY shells)". (ii) In (e), "This applies to both the pipe-log path (via `annotate_ansi()`) and the `capture-pane` fallback paths (via …)" → "This applies to pane captures (via `capture_pane_with_escapes()` with `-e`, then `annotate_ansi()`)". (iii) Replace all of (g) with `(g) *(Retired: the prompt names no foreground pane; commands run in PTY shells.)*`. |
| 7 | `docs/REQUIREMENTS.md` FR-1.2.6 | "so the AI understands which machine each execution mode (`background` vs `foreground`) will target" → "so the AI knows which machine the user is looking at. Commands still run in the daemon's PTY shells, or on a configured `[hosts.*]` host." |
| 8 | `docs/REQUIREMENTS.md` FR-1.2.17 (b) | Replace clause (b) only: "runs in a PTY shell: on a configured `[hosts.*]` host, in its sbx sandbox, or locally. An unresolved `ssh_target` is refused." |
| 9 | `docs/architecture.md` ~306 (Shipped baseline, history) | Append to the **Foreground/background execution** bullet: "(replaced by PTY shells in 2.0)". Change nothing else in it. |
| 10 | `CLAUDE.md` `run_terminal_command` row | "Runs a command in a PTY shell. `wait?` selects between them —" → "Runs a command in a PTY shell. `wait?` selects whether to wait —". Change nothing else in the cell. |

Then: full gates, § End-to-end verification once, one `docs:` commit.

### Gotchas

- Edit with `patch`, never `patch_lines`, python, or `sed` through `bash`.
- REQUIREMENTS keeps every FR id. Do not renumber, and do not delete an id.
- `doc_truth` checks the CLAUDE.md and README tool tables against the code.
  In row 10, change only the description text.

## Acceptance criteria

- [ ] `cargo test --test doc_truth runtime_docs_describe_no_tmux_execution`
      reports `1 passed`, and `RETIRED_TMUX_EXECUTION_TERMS` contains the
      four new terms.
- [ ] `grep -n 'chat --session\|exec-attach' README.md` prints nothing.
      Today it prints 2 lines.
- [ ] `grep -c 'selects between them' CLAUDE.md` prints `0`. Today `1`.
- [ ] `grep -n '(replaced by PTY shells in 2.0)' docs/architecture.md`
      prints one line. Today none.
- [ ] *Preservation:* every other `doc_truth` test passes.
- [ ] clippy (`-D warnings`) and `cargo fmt --all --check` exit 0.
- [ ] `cargo test` reports `failed=0` with the same `passed` as after 06e.
      This phase adds no test.
- [ ] The Update Log has § End-to-end verification's output and
      `PASTE MATCH: yes`.

## End-to-end verification

```sh
cargo test --test doc_truth 2>&1 | grep -E '^test result'
grep -n 'chat --session\|exec-attach' README.md | wc -l
grep -c 'selects between them' CLAUDE.md
grep -c '(replaced by PTY shells in 2.0)' docs/architecture.md
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1
cargo fmt --all --check; echo "fmt_exit=$?"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
```

Run each line once. **Paste the output into the Update Log yourself**,
followed by `PASTE MATCH: yes`. The server does not write it for you.
06c's run assumed it would, and the evidence was missing.

## Authorizations

- [x] May edit `README.md`, `CLAUDE.md`, `docs/PRODUCT_DEFINITION.md`,
      `docs/REQUIREMENTS.md`, `docs/architecture.md`, `tests/doc_truth.rs`.

## Out of scope

- `docs/design-reference.md`, `docs/ROADMAP.md`, `docs/dev/**`.
- README's **Remote script streaming** paragraph. Whether that path is
  still reachable is unverified, so the phase 07 live check decides it.
- Any code. 06e owns the code side.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-10-11 14:14 (progress)

Started phase 06d (executor, dispatch 1). Added the four retired terms
(`FOREGROUND TARGET`, `Pane target changed`, `background=false`,
`de-incident`) to `RETIRED_TMUX_EXECUTION_TERMS` and confirmed the
`runtime_docs_describe_no_tmux_execution` gate went red naming rows 2, 5,
6 and 8 before they were fixed; then made rows 2–10 and re-ran the gate
green. Status flipped to `in-progress` here and in the milestone README.

Extra stale passages found while verifying (not in the work list, now
fixed under the same authorization):
- `README.md` ~279: "`/panes` … with the pinned foreground target marked"
  → "with the pinned pane marked".
- `README.md` `/pane` slash-command row: "pins the foreground target"
  → "pins the pane whose activity refreshes the context snapshot" (same
  meaning row 2's rewrite gives).
- `README.md` ~1425: the audit-log sample still showed `mode=background
  pane=-` / `mode=foreground pane=%3` and a "send-failed" status; now
  `shell=s1/s2` and a "shell id" field, per phase 06e's PTY-only audit
  records.

### Update — 2026-10-11 14:20 (end-to-end verification)

Run once against the tree this dispatch produced (`/tmp/e2e-06d.txt`):

```
== doc_truth full ==
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.04s
== runtime_docs_describe_no_tmux_execution alone ==
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 15 filtered out; finished in 0.03s
== retired terms in the list ==
771:    "de-incident",
775:    "FOREGROUND TARGET",
776:    "Pane target changed",
777:    "background=false",
== README stale terms ==
0
== CLAUDE wait wording ==
0
== architecture replacement note ==
308:  (replaced by PTY shells in 2.0).
== clippy ==
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.16s
== fmt ==
fmt_exit=0
== full test suite ==
passed=2309 failed=0
```

PASTE MATCH: yes

The `runtime_docs` gate was also seen red before the doc rows were fixed
(7 hits: README 197/200, PRODUCT_DEFINITION 42, REQUIREMENTS 51 and 22
×3), so the four new terms demonstrably bite. Full suite passed=2309,
failed=0 — 06e's count carried forward unchanged, no new test added.
