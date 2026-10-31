# Phase 03 — vt100 0.18

**Status:** in-progress
**Milestone:** M33 — Dependency refresh
**Drafted:** 2026-10-31
**Depends on:** phase 02 (done)
**Tags:** language=rust, kind=chore, size=m

## Goal

vt100 goes from 0.16.2 to **0.18.1**, and DaemonEye takes its new resize
behaviour in both places a screen is resized (M33 decision 3; PE choice
2026-10-31, "viewer + backend"):

- **The tail viewer** (`src/cli/tail.rs`, `resize`) already calls
  `set_size`; it gets the new behaviour by upgrading.
- **The daemon's shell screen** stops being rebuilt on resize.
  `PtyBackend::resize` (`src/shell/backend.rs`) today replaces the screen
  with a blank `ShellScreen::new` (the old "F2, does not reflow" comment).
  After this phase it resizes the existing screen through a new
  `ShellScreen::resize`, so a resized shell keeps its screen and scrollback.

The suite goes from **2616 to exactly 2618**: two tests added, one renamed.

## What changed upstream (vt100 CHANGELOG, 0.17.0 and 0.18.0)

You cannot fetch this; it is the part that matters here.

- `Parser::new(rows, cols, scrollback)` and `Screen::set_size(rows, cols)`
  take `NonZeroU16`, and `Screen::size()` returns `(NonZeroU16, NonZeroU16)`.
  Passing 0 used to panic.
- These `Screen` methods return `String` instead of `Vec<u8>` (the bytes
  were always UTF-8): `attributes_formatted`, `contents_diff`,
  `contents_formatted`, `cursor_state_formatted`, `input_mode_diff`,
  `input_mode_formatted`, `rows_diff`, `rows_formatted`, `state_diff`,
  `state_formatted`.
- `Screen::set_size`, when the height **decreases**: drops rows from the
  bottom only until the cursor is on the bottom row, then pushes the excess
  rows off the top **into scrollback**. When the height **increases**: pulls
  rows back from scrollback into the top, then pads with blank rows at the
  bottom. It also resets the scroll region and the scrollback offset when the
  size changes. (0.16 cut or padded rows at the bottom.)
- New and unused here: `Screen::scrollback_len`, `clear_scrollback`,
  `contents_formatted_basic`, `Parser::set_size`, `Callbacks::on_scroll`.

## Spec

1. **Upgrade** — `Cargo.toml` vt100 to `0.18.1`; `cargo update -p vt100`.
   Not a bare `cargo update`.
2. **Fix the build** — the errors are in `src/shell/screen.rs`,
   `src/cli/tail.rs`, `src/cli/tail_view.rs`, `src/shell/keys.rs` and the
   test files that construct parsers (`src/shell/backend_tests.rs`,
   `src/daemon/server/shells_tests.rs`). Rules: `ShellScreen` and the tail
   model keep their `u16` signatures; a 0 becomes 1 at the conversion, as
   `tail.rs` already does with `.max(1)`; no `unwrap`/`expect` in production
   code (`NonZeroU16::MIN` is the fallback). Where a caller wants bytes, take
   the `String` and call `.into_bytes()`; do not round-trip through
   `from_utf8`. Done when `cargo build --all-targets` is clean.
3. **`ShellScreen::resize(rows, cols)`** — add it in `src/shell/screen.rs`
   (calls `set_size`, same 0 → 1 rule). `PtyBackend::resize` calls it instead
   of assigning a new `ShellScreen`; replace the F2 comment with one line
   saying the screen keeps its rows across a resize (vt100 0.18 semantics).
   Nothing else in `backend.rs` changes.
4. **Tests** — both new tests go in the existing test modules, beside their
   neighbours:
   - `screen_resize_shrink_pushes_rows_to_scrollback_and_grow_restores_them`
     (`src/shell/screen.rs` tests): print numbered lines into a small screen,
     shrink the height, assert the top lines left `contents()` and appear in
     `scrollback_rows`, grow back, assert `contents()` shows them again.
   - `tail_resize_shrink_then_grow_keeps_the_rows` (`src/cli/tail.rs`
     tests): the same shrink and grow through the tail model's `resize`.
   - Rename `backend_resize_replaces_the_screen_and_does_not_reflow`
     (`src/shell/backend_tests.rs`) to `backend_resize_keeps_the_screen_contents`.
     Before resizing, print a marker and wait for it the way
     `subscribe_with_screen_then_live_chunks_rebuild_the_screen` does. Keep
     the size asserts, and assert the marker is still in `screen_contents()`
     after resizing to 40×120.
5. **Red check** — temporarily put the old `ShellScreen::new` line back in
   `PtyBackend::resize`, run `cargo test backend_resize_keeps_the_screen_contents`,
   restore it. Paste the `test result:` line and the assertion message (two
   lines) in an `### Update — <date> (red check)` entry.
6. **Findings** — one `### Findings — <date>` entry built from
   `git diff --stat` alone: two or three lines naming the files that changed
   and any call site where you departed from step 2's rules, or `none`. Open
   no file and no crate source for this step.
7. **End-to-end** — run the block below once and paste its output, fenced, in
   an `### Update — <date> (end-to-end verification)` entry.
8. **Commit** as `chore: upgrade vt100 to 0.18; keep the shell screen across a resize`.

The vt100 source is under `~/.cargo/registry/src/*/vt100-0.18.1/`. Open it
only to fix a compile error you cannot fix from the compiler message.

## Acceptance criteria

- [ ] The E2E block's first line prints
      `vt100=0.18.1 rebuild=0 screen_resize=1 new_tests=2 old_name=0`
      (today `vt100=0.16.2 rebuild=1 screen_resize=0 new_tests=0 old_name=1`).
- [ ] `allowlist=unchanged`, `build_warnings=0`, clippy clean, `fmt_exit=0`;
      hygiene and doc_truth pass.
- [ ] `cargo test` reports exactly `passed=2618 failed=0`.
- [ ] The red-check entry shows `backend_resize_keeps_the_screen_contents`
      failing against the rebuild.
- [ ] No new `#[allow]`, `#[ignore]`, `unsafe`, or production `unwrap`/`expect`.

## End-to-end verification

```sh
bash <<'E2E'
echo "vt100=$(cargo tree --depth 1 | sed -n 's/.*── vt100 v\([0-9.]*\).*/\1/p') rebuild=$(sed -n '/async fn resize/,/^    }/p' src/shell/backend.rs | grep -c 'ShellScreen::new') screen_resize=$(grep -c 'pub fn resize' src/shell/screen.rs) new_tests=$(grep -hcE 'fn (screen_resize_shrink_pushes_rows_to_scrollback_and_grow_restores_them|tail_resize_shrink_then_grow_keeps_the_rows)\(' src/shell/screen.rs src/cli/tail.rs | paste -sd+ | bc) old_name=$(grep -c 'fn backend_resize_replaces_the_screen_and_does_not_reflow' src/shell/backend_tests.rs)"
git diff --quiet HEAD -- tests/hygiene_allowlist.txt && echo allowlist=unchanged
touch src/main.rs && cargo build 2>&1 | grep -c '^warning' | sed 's/^/build_warnings=/'
cargo test --test hygiene 2>&1 | grep -E '^test result'
cargo test --test doc_truth 2>&1 | grep -E '^test result'
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1
cargo fmt --all --check; echo "fmt_exit=$?"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
E2E
```

## Authorizations

- Edit `Cargo.toml`, `Cargo.lock` (via `cargo update -p vt100`) and the six
  files named in step 2.
- Change `PtyBackend::resize`'s behaviour as step 3 says (PE decision).

## Out of scope

- Any other crate. The new vt100 APIs (`on_scroll`, `clear_scrollback`, …).
- Cap headroom is ample (production lines: `screen.rs` 250, `backend.rs`
  312, `tail.rs` 610 of 800); the allowlist gains nothing.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-10-31 00:40 (end-to-end verification)

Ran the phase doc's E2E block once; output pasted verbatim:

```
vt100=0.18.1 rebuild=0 screen_resize=1 new_tests=2 old_name=0
allowlist=unchanged
build_warnings=0
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.80s
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.04s
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 24.19s
fmt_exit=0
passed=2618 failed=0
```

First line matches the acceptance criterion exactly (`vt100=0.18.1 rebuild=0
screen_resize=1 new_tests=2 old_name=0`), `cargo test` reports exactly
`passed=2618 failed=0` (2616 before + the two new tests).

### Findings — 2026-10-31 (from `git diff --stat`)

Changed: `Cargo.toml`/`Cargo.lock` (vt100 0.16.2 → 0.18.1), `src/shell/screen.rs`
(`ShellScreen::resize` + the `NonZeroU16` conversions), `src/shell/backend.rs`
(`PtyBackend::resize` calls it), `src/cli/tail.rs` (parser construction, `resize`,
the `.get()` on `size()` halves), `src/cli/tail_view.rs` (one `.get()` pair),
plus the test files `src/shell/backend_tests.rs`, `src/daemon/server/shells_tests.rs`.
Departures from step 2's file list: `src/cli/attach.rs` also needed the fix (two
`model.screen().size()` destructures and a `(30, 100)` size assert leaked the
`NonZero` change there); `src/shell/keys.rs` needed nothing (its `Screen` reads
return `bool`, unchanged upstream). Also dropped `PtyBackend`'s `scrollback`
field — the removed `ShellScreen::new` in `resize` was its only reader.

### Update — 2026-10-31 00:36 (red check)

`PtyBackend::resize` temporarily rebuilt the screen with `ShellScreen::new`
(the old behaviour) while `backend_resize_keeps_the_screen_contents` was run
against it:

```
thread 'shell::backend::tests::backend_resize_keeps_the_screen_contents' (1370766) panicked at src/shell/backend_tests.rs:811:5:
the screen keeps its contents across a resize: ""
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 2538 filtered out; finished in 0.05s
```

The old `ShellScreen::new` line is restored; the test passes against the
`ShellScreen::resize` implementation.

### Update — 2026-10-31 00:07 (progress)

Executor 03 started. Plan: upgrade vt100 in `Cargo.toml` + `cargo update -p vt100`,
fix the five production/test files named in step 2 for the `NonZeroU16` sizes and
`String` returns, add `ShellScreen::resize` and rewire `PtyBackend::resize`, then
the two new tests, the red check, findings, E2E, and the commit.

