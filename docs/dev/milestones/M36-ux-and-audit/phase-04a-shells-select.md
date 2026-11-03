# Phase 04a — shells-select

**Milestone:** M36 — Chat UX and audit fixes
**Status:** todo
**Drafted:** 2026-11-03
**Depends on:** phase 03 (done)
**Estimated diff:** ~250 lines
**Tags:** language=rust, kind=feature, size=s

## Goal

The first half of design item 4 (`docs/design/2.7.0-UX-improvements.md`),
split from the original phase 04 after its first dispatch read for 514
turns without writing: the live `daemoneye top` gets a **selected shell**
in its Shells panel (phase 04b opens the attach view on it), and FINDINGS
**P5** is fixed — the disk refresh moves off the task that handles keys.

## Architecture references

- `src/cli/commands/top_tui.rs` · `TopModel` (`selected`, `shells_top`,
  `scroll_up` / `scroll_down` / `scroll_home` / `scroll_end`,
  `clamp_offsets`), `run_tui`, `plan_refresh`, `Refresh`;
  `top_tui_draw.rs` · how the selected run is drawn reversed.
- `src/cli/commands/top_load.rs` · `disk_panels`, `DiskPanels`.
- `docs/dev/milestones/M36-ux-and-audit/FINDINGS.md` row P5.

## Pre-flight

1. Read `docs/dev/STANDARDS.md`, then the files above **once each**, then
   this doc. The design below is decided; do not re-derive it. Start Spec
   step 1 as soon as you have read them.

## Behaviour

**Shells selection.** With Shells focused, `↑↓` / `k`·`j` / `PageUp` /
`PageDown` / `Home` / `End` move a selected shell row — drawn reversed as
the selected run is — and the panel's window follows the selection (today
they scroll the rows). A shells refresh keeps the selection on the same
shell **id**; when that shell is gone it clamps into the new list. With no
rows (`none`, `daemon not running`) the keys do nothing. Runs, Budget and
Activity keys are unchanged.

**P5.** `disk_panels` runs on a blocking thread, never on the task that
handles keys; the loop applies its result when it arrives. At most one disk
refresh is in flight: while one runs, `plan_refresh` asks for no other.
What applying a result does is unchanged — runs, budget and activity
replaced, `taken_at` kept, follow re-targeted, the open detail reloaded.

## Constraints

- **The async shape is fixed — use the one `run_tui` already has.** Its
  crossterm reader is a thread that sends each event on an
  `mpsc::unbounded_channel`, and the `select!` has one `event_rx.recv()`
  arm. The disk refresh is the same: start it with
  `tokio::task::spawn_blocking`, give it a clone of a second channel's
  sender, and add one `disk_rx.recv()` arm that applies the result. **Do
  not** keep a `JoinHandle`, a `oneshot` receiver or any other borrowed
  future alive across `select!` iterations, and do not read tokio's source
  to work out whether you may — the previous dispatch spent 30+ turns
  there. The in-flight flag is a `bool` on the loop.
- **Pure seams:** the selection on `TopModel` (driven by `handle_key`,
  refreshed by the shells refresh), the apply step a function over
  `&mut TopModel` and a `DiskPanels`, and the in-flight rule an input to
  `plan_refresh`. `run_tui` only wires them.
- **Docs follow:** README's `daemoneye top` paragraph says Shells' scroll
  keys move a selection (today it says they scroll the rows).
- The allowlist is a ratchet (`tests/hygiene_allowlist.txt`); use `patch`
  with whole blocks on long files, never `patch_lines`.

## Spec

1. **Red tests first** — write these against the current code, stubbing
   new functions where needed, run them, keep the failing output for step 5:
   - `shells_selection_moves_follows_and_survives_refresh` — the keys move
     it and the window follows; a refresh keeps the same id and clamps when
     it is gone; an empty panel ignores the keys.
   - `shells_selection_draws_reversed` — on a `TestBackend`, the selected
     shell's row is reversed and the others are not.
   - `plan_refresh_holds_disk_while_one_is_in_flight` — a changed
     fingerprint or an aged snapshot asks for no disk refresh while one is
     in flight, and does once it lands.
   - `apply_disk_keeps_taken_at_follow_and_detail` — applying a
     `DiskPanels` replaces runs/budget/activity, keeps `taken_at`,
     re-targets follow and reloads an open detail.
2. **The selection** on `TopModel` and in `draw`; update
   `shells_scroll_clamps_at_both_ends` and `screen_draws_scrolled_shells`
   only as far as the keys now move a selection.
3. **P5** — the apply function, the in-flight input, the channel arm; mark
   FINDINGS P5 `fixed: M36 phase-04a,
   plan_refresh_holds_disk_while_one_is_in_flight`.
4. **Docs** — the README sentence. `cargo test` passes with `failed=0`.
5. **Red runs** — one `### Update — <date> (red runs)` entry pasting step
   1's `test result:` lines and assertion messages verbatim from the run's
   output (no elisions or notes inside the fence).
6. **Findings** — one `### Findings — <date>` entry, two or three lines, or
   `none`.
7. **End-to-end** — run § End-to-end verification once before committing
   and paste its output, fenced, in an `### Update — <date> (end-to-end
   verification)` entry.
8. **Commit** as `feat: top's Shells panel selects a shell; the disk refresh leaves the key path (M36 phase-04a)`.

## Acceptance criteria

- [ ] The E2E block's first line is `shells_selection passed=2 failed=0`
      and its second `refresh passed=2 failed=0` (today both `passed=0`).
- [ ] The red-runs entry shows the new tests failing on the pre-phase code.
- [ ] The E2E block's `blocking_sites=` line is at least 1 (today 0), and
      `disk_panels` is no longer awaited inline in `run_tui`'s tick arm —
      the reviewer reads the arm.
- [ ] The other `top_tui` tests pass (preservation), notably
      `enter_opens_the_selected_runs_detail` and
      `esc_restores_focus_selection_and_offsets`.
- [ ] FINDINGS P5's triage cell reads `fixed: M36 phase-04a, …`.
- [ ] The E2E block shows `hygiene`, `doc_truth` and `bug_tracker` `ok`,
      `build_warnings=0`, clippy clean, `fmt_exit=0`, and `failed=0`.

## Test plan

Pure tests over `TopModel` and `plan_refresh` pin the selection and the
refresh rule; a `TestBackend` test pins the highlight. The milestone's live
check exercises it inside the attach flow after phase 04b.

## End-to-end verification

```sh
bash <<'E2E'
cargo test shells_selection_ 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "shells_selection passed=" p, "failed=" f}'
( cargo test plan_refresh_holds_disk; cargo test apply_disk_keeps ) 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "refresh passed=" p, "failed=" f}'
grep -c 'spawn_blocking' src/cli/commands/top_tui.rs | sed 's/^/blocking_sites=/'
touch src/main.rs && cargo build 2>&1 | grep -c '^warning' | sed 's/^/build_warnings=/'
for t in hygiene doc_truth bug_tracker; do cargo test --test $t 2>&1 | grep -E '^test result'; done
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1
cargo fmt --all --check; echo "fmt_exit=$?"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
E2E
```

## Authorizations

- Edit `src/cli/commands/top_tui.rs`, `top_tui_draw.rs`, `top_load.rs`,
  `README.md`, `docs/dev/milestones/M36-ux-and-audit/FINDINGS.md` (row P5),
  and `tests/hygiene_allowlist.txt` (ratchet only).

## Out of scope

- Opening anything on the selected shell, `Enter`/`a` on Shells, the footer
  hint — phase 04b.
- `top --once` output and the other FINDINGS rows.

## Update Log

<!-- entries appended below this line -->
