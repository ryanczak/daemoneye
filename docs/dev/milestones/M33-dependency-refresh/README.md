# M33 — Dependency refresh

**Goal:** every DaemonEye dependency is at its latest stable release, and
`cargo install` builds without a warning.

**Status:** planning (scoped 2026-10-30)

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

**Proposed decisions (awaiting PE sign-off):**

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
| 01 | tokio-and-compatible — `cargo update`; every semver-compatible requirement raised to its latest; `AsyncFd::register` in `tty.rs` (decision 1); the stream-idle test's fake server reads the request before it answers | tokio + 11 compatible crates | todo |
| 02 | rusqlite-and-syn — rusqlite 0.41 and syn 3, the two breaking upgrades that build clean | rusqlite, syn | todo |
| 03 | vt100-0.18 — the `NonZeroU16` sizes, the `String` returns, and the resize behaviour (decision 3) | vt100 | todo |

## Notes

- **Scoped 2026-10-30** at the PE's request, ahead of the function
  milestone. Numbering: this milestone takes M33, so MCP surface becomes
  M34 and Knowledge M35; the function milestone (the 40 allowlisted
  functions) stays next after this one, unnumbered until scoped.
