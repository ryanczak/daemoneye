# M33 — Dependency refresh

**Goal:** every DaemonEye dependency is at its latest stable release, and
`cargo install` builds without a warning.

**Status:** done (scoped 2026-10-30, phases closed 2026-10-31; release pending the live check)

**Release:** 2.6.0

**Depends on:** M32 (the tree as released in 2.5.0).

**Why now.** `cargo install --path .` ignores `Cargo.lock`, so it resolves
the newest compatible version of every dependency. Since tokio 1.54.0
(2026-10-19) that build warns:

```
warning: use of deprecated associated function `tokio::io::unix::AsyncFd::<T>::new`:
unsound due to lack of I/O safety; use `AsyncFd::register` instead
(see https://github.com/tokio-rs/tokio/pull/8587)
  --> src/cli/input/tty.rs:45:43
```

`cargo build` in the repo is silent only because the lockfile still pins
tokio 1.53.1. The first `cargo update` turns the warning into a red clippy
gate (`-D warnings`), so the fix and the update land together.

**Survey (2026-10-30, crates.io `max_stable_version`, trial builds in a
scratch worktree).** Of the 29 direct dependencies, 13 are already at their
latest. The rest:

| Crate | Locked | Latest | Trial result |
|---|---|---|---|
| tokio | 1.53.1 | 1.54.1 | the `AsyncFd::new` deprecation (two sites, one of them test-only); and `idle_stream_stall_is_reported_by_stream_next_with_timeout` fails deterministically (bisected to tokio: its fake server writes the response before reading the request, and hyper now sees it as `UnexpectedMessage`) |
| async-trait, clap, futures-util, libc, log, reqwest, serde_json, similar, toml, uuid, zeroize | — | patch/minor | build clean; the library suite passes but for the tokio test above |
| rusqlite | 0.40.1 | 0.41.1 | builds clean; suite not yet run past the tokio failure |
| syn (dev) | 2.0.119 | 3.0.9 | builds clean (`tests/hygiene.rs` is its only user) |
| vt100 | 0.16.2 | 0.18.1 | 25 compile errors: sizes are `NonZeroU16`, the `*_formatted` / `*_diff` methods return `String`, and `Screen::set_size` now pushes rows into scrollback on a shrink and pulls them back on a grow |

`libc` stays on 0.2: its only newer release is `1.0.0-alpha.4`, as the
manifest comment says.

**PE decisions (2026-10-30, signed off by proceeding to phase 01):**

1. **One `unsafe` block is authorized** in `src/cli/input/tty.rs`, for
   `AsyncFd::register`. Every `AsyncFd` constructor is `unsafe` from
   tokio 1.54 on; there is no safe path short of dropping `AsyncFd`. The block
   carries a `// SAFETY:` comment stating why tokio's contract holds, and the
   file ends with no more `unsafe` blocks than it has today.
2. **Breaking upgrades get their own phase**, so a failure bisects to one
   crate. Semver-compatible updates land together.
3. **A behaviour change from upstream is a decision, not a side effect.**
   Where a new version changes what DaemonEye does (vt100's resize), the
   phase doc names the change and whether we take it, and a test pins the
   result.

**Exit criteria:**

- Every direct dependency in `Cargo.toml` (normal and dev) requires its
  latest stable release, and `Cargo.lock` resolves to it; `libc` excepted
  as above.
- `cargo install --path .` (no `--locked`) finishes with no `warning:` line.
- The four gates pass; `cargo test` passes at 2615 tests or more;
  `tests/hygiene.rs` and `tests/doc_truth.rs` pass; the allowlist has gained
  no entry.
- `idle_stream_stall_is_reported_by_stream_next_with_timeout` passes on
  tokio 1.54.
- Live check on pinky: `daemoneye chat` reads typed input and an approval
  answer from a real terminal (the `AsyncStdin` path), and a PTY shell
  resized with `daemoneye shell attach` keeps its screen contents.
- `2.6.0` is released per `REXYMCP.md` § Versioning.

## Architecture references

- `docs/architecture.md` § 1.2 (orchestration, the chat client's input
  path) and the shell engine sections (`ShellScreen`, the PTY host).
- `CLAUDE.md` § Key files: `src/cli/input/tty.rs`, `src/shell/screen.rs`,
  `src/shell/keys.rs`, `src/cli/tail.rs` / `tail_view.rs`, `src/memory/index/`.

## Phases

Drafted one at a time with `/rexymcp:architect next`. Expected count: 3.

| #  | Phase | Scope | Status |
|----|-------|-------|--------|
| 01 | tokio-and-compatible ([phase-01-tokio-and-compatible.md](phase-01-tokio-and-compatible.md)) — `cargo update`; every semver-compatible requirement raised to its latest; `AsyncFd::register` in `tty.rs` (decision 1); the stream-idle test's fake server reads the request before it answers | tokio + 11 compatible crates | done        |
| 02 | rusqlite-and-syn ([phase-02-rusqlite-and-syn.md](phase-02-rusqlite-and-syn.md)) — rusqlite 0.41 and syn 3, the two breaking upgrades that build clean | rusqlite, syn | done        |
| 03 | vt100-0.18 ([phase-03-vt100-0.18.md](phase-03-vt100-0.18.md)) — the `NonZeroU16` sizes, the `String` returns, and the resize behaviour (decision 3) | vt100 | done        |

## Notes

- **Scoped 2026-10-30** at the PE's request, ahead of the function
  milestone. Numbering: this milestone takes M33, so MCP surface becomes
  M34 and Knowledge M35; the function milestone (the 40 allowlisted
  functions) stays next after this one, unnumbered until scoped.

## Retrospective — 2026-10-31

**Outcome.** All three phases done. Every direct dependency is at its latest
stable release (`libc` excepted, as scoped). The suite went from 2615 to 2618
tests. `cargo install --path .` without `--locked` builds with no compile
warning: checked 2026-10-31 in a scratch root, where the only `warning:` line
was cargo's PATH notice for that root. The allowlist gained nothing.

| Phase | Verdict | Turns | Note |
|---|---|---|---|
| 01 tokio-and-compatible | approved_first_try | — | The architect wrote a script that made every edit; the executor ran it |
| 02 rusqlite-and-syn | approved_after_1 | 204 + 17 | The upgrade needed no code. The first run stalled on an open-ended Findings audit; resumed with a bounded step |
| 03 vt100-0.18 | approved_first_try | 249 | The resize semantics change was taken in both the viewer and the backend (PE choice) |

**Behaviour changes shipped.**
- `AsyncStdin` owns its `/dev/tty` `File` through `AsyncFd::register`. That is
  the one authorized `unsafe` block, and the hand-rolled `libc` I/O is gone.
- A resized PTY shell keeps its screen and scrollback, where it used to be
  rebuilt blank. vt100 0.18 pushes rows into scrollback on a shrink and pulls
  them back on a grow. `read_shell_screen` after a resize now shows the old
  content until the program redraws.

**What the milestone taught (architect side).**
1. *The executor writes the code.* Phase 01's pre-written script was the
   architect doing the phase. The PE stopped it (2026-10-30). Phases 02 and 03
   were lean specs and GLM did the work.
2. *Every Spec step must terminate.* Phase 02's "Findings: anything the
   upgrade changed", plus a pointer at the crate sources, had no stopping
   point. GLM diffed two crates for about 120 read-only calls on an
   already-green tree. Two named commands and a three-line answer finished it
   in 17 turns.
3. *A spec's file list comes from the compiler, not a grep.* Phase 03's list
   (from `grep vt100::`) missed `src/cli/attach.rs`, which reaches the size
   type through a method. The executor found it from the build errors and
   reported it.
4. *Pin the values you can.* Phase 03's 249 turns went partly to measuring
   vt100 0.18's post-resize rows, which the spec described but did not pin.

Each of these is one occurrence. Nothing is folded into WORKFLOW.md; all four
are in the architect's memory.

**Open before the release.** The live exit criterion: on pinky, `daemoneye
chat` reads typed input and an approval answer from a real terminal (the
`AsyncStdin` path), and `daemoneye shell attach` on a resized shell keeps its
contents.
