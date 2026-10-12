# Phase 07a: docs-truth — the stale passages the live rehearsal found

**Milestone:** M27 — tmux becomes an optional observer; 2.0 close
**Status:** todo
**Depends on:** phase-07c (done)
**Tags:** language=markdown, kind=docs, size=s

## Goal

Rehearsal 1 of phase 07 (S5) and the review of what it found turned up
eight passages that still describe the removed tmux execution path, or a
record shape the daemon no longer writes. This phase makes each one true.
Two rows edit Rust doc comments; no code behaviour changes.

## Work list — this table is the whole job

Rules (from 06c and 06d, where they turned a stall into a clean run):

- **Open only the file and line range a row names.** Line numbers are as
  of 2026-10-12 and may shift by a few lines. Every fact you need is in
  this table.
- **Edit only these rows.** Anything else that looks stale goes in the
  Update Log, not into an edit.
- **Your first `patch` lands within 10 turns.** That patch is row 1.

| # | File · where | Action — write this meaning, in the file's own style |
|---|---|---|
| 1 | `README.md` ~200, the **Remote script streaming** paragraph | Replace the whole paragraph. New meaning: a **Ghost Shell** that runs a daemon-host script (one in `~/.daemoneye/scripts/`) on its remote `ssh_target` host streams the script instead of copying it: hex-encoded and piped into the remote interpreter's stdin (`python3`/`perl` decode → `bash /dev/stdin <args>`), with no remote disk write. Under `sudo` the script is written to its `~/.daemoneye/scripts/<name>` path on the remote instead, because a NOPASSWD sudoers rule must name a fixed path. Point at the `ssh_target` row of the runbook fields table. Do not mention panes or foreground commands. |
| 2 | `README.md` ~1002, the `[execution]` `backend` table row | Description → `"pty"` is the only substrate: the daemon's PTY shells, recorded as asciicast v2 logs in `~/.daemoneye/var/log/shells/`. `"tmux"` was removed in 2.0: it still loads, with a startup warning, and commands run in PTY shells. Any other value falls back to `"pty"` with a warning. Keep the row's name, type and default cells. |
| 3 | `README.md` ~1248, the `ssh_target` row | Replace the one sentence "When the value does not resolve to a configured host, commands are wrapped in `ssh <target> <cmd>` as before." with: a value that resolves to no configured host is refused — the ghost's commands do not run. Change nothing else in the cell. |
| 4 | `README.md` ~1422–1432, § **Command audit log** | Replace the two `[1748000000] session=…` sample lines and the "Fields for command records" sentence. Keep the intro paragraph, the `ai_cost` sample line and the retention paragraph. New samples, one JSON object per line, keys in this order: `{"cmd":"ps aux --sort=-%mem","event":"command","exit":0,"mode":"shell","out":"USER PID ...","pane":"s0","session":"abc123","status":"approved","ts":"2026-05-16T10:22:58Z","pid":41237}` and `{"cmd":"sudo rm -rf /tmp/old","event":"command","mode":"foreground","out":"","pane":"","session":"abc123","status":"denied","ts":"2026-05-16T10:22:59Z","pid":41237}`. New fields sentence: a command that ran has `mode` `shell`, `pane` = the PTY shell id, and `exit` = its real exit code. An approval decision that ran nothing has `mode` `foreground` (waited) or `background` (`wait: false`) and an empty `pane`. `status` is `approved` / `denied` / `timeout` / `refused`, or a path-specific failure. `out` is the first 200 characters of output. |
| 5 | `assets/etc/config.toml` ~274–277, the `[execution]` comment block | Same meaning as row 2, as TOML comments: `"pty"` is the only substrate; `"tmux"` was removed in 2.0 and loads with a warning. The commented `# backend = "pty"` line stays, with the trailing comment `# the only supported value`. |
| 6 | `src/config/types.rs` ~682–687, the `ExecutionConfig` doc comments | Struct comment: `backend` defaults to `"pty"`, the only substrate; `"tmux"` was removed in 2.0 and is accepted only so old configs load (`validate` warns). Field comment: `"pty"` (the only supported value). Comments only. |
| 7 | `src/scheduler.rs` ~169, the `JobStatus::Failed` doc comment | "Failed; the tmux window `de-<id>` is left open for inspection." → "Failed, with the error message." Comment only. |
| 8 | `assets/memory/session/pane-referencing-convention.md` lines 6–7 | Line 6: replace the sentence "**A foreign-session pane is not a valid foreground target**; `run_terminal_command` and the `/pane` pin only apply to the user's own session." with "The `/pane` pin applies only to the user's own session." Line 7: "If the target pane for a foreground command cannot be determined …" → "If the pane the user means cannot be determined unambiguously from `[PANE MAP]` or context, ask the user which pane they mean." Drop "before calling `run_terminal_command`": commands never run in a pane. |

Then: full gates, § End-to-end verification once, one `docs:` commit.

### Gotchas

- Edit with `patch`, never `patch_lines`, python, or `sed` through `bash`.
- `doc_truth` checks the README tables against the code. In rows 2 and 3
  change only the description cell.
- `doc_truth`'s `seeded_config_template_*` tests read
  `assets/etc/config.toml`. Row 5 keeps the `backend` key present.

## Acceptance criteria

- [ ] `grep -c 'foreground command is sent to a pane' README.md` prints `0`.
      Today `1`.
- [ ] `grep -c 'session=abc123' README.md` prints `0`. Today `2`.
- [ ] `grep -c '"mode":"shell"' README.md` prints `1`. Today `0`.
- [ ] `grep -c 'as before. Daemon-host' README.md` prints `0`. Today `1`.
- [ ] `grep -c 'tmux panes as before' README.md` prints `0`. Today `1`.
- [ ] `grep -c 'explicit opt-out' assets/etc/config.toml src/config/types.rs`
      prints `0` for both files. Today `2` and `1`.
- [ ] `grep -c 'tmux window' src/scheduler.rs` prints `0`. Today `1`.
- [ ] `grep -c 'foreground' assets/memory/session/pane-referencing-convention.md`
      prints `0`. Today `2`.
- [ ] *Preservation:* `cargo test --test doc_truth` reports `failed=0`.
- [ ] clippy (`-D warnings`) and `cargo fmt --all --check` exit 0.
- [ ] `cargo test` reports `passed=2319 failed=0`. This phase adds no tests,
      so a different `passed` count is scope creep.

## End-to-end verification

Run each acceptance `grep` and `cargo test --test doc_truth`, then paste
the outputs verbatim in the Update Log under `PASTE MATCH: yes`.

## Authorizations

- [x] Edit the eight rows above, comments only in the two `.rs` files.

## Out of scope

- The daemon.log line "foreground/background command approved" and the
  `mode` values `foreground`/`background` in approval records. They are
  log values; renaming them is a code change for a later milestone.
- The `[ACTIVE PANE]` label on a non-selected pane (rehearsal observation).
- Any other stale text you notice: record it in the Update Log.

## Update Log

<!-- entries appended below this line -->
