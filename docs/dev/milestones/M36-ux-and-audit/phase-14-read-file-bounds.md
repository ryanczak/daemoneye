# Phase 14 — read-file-bounds

**Milestone:** M36 — Chat UX and audit fixes
**Status:** todo
**Drafted:** 2026-11-07
**Depends on:** none
**Estimated diff:** ~300 lines
**Tags:** language=rust, kind=bug, size=m

## Goal

This phase fixes two findings from `FINDINGS.md`.

### S8 (security/bug, major): `read_file` reads anything, whole, on the async runtime

The local branch of `run_read_file` (`src/daemon/executor/file_ops/read.rs`)
calls `std::fs::read_to_string` on any path:

- It does no file-type check.
- It has no size cap.
- It runs synchronously inside an async fn, and nothing asks for approval.

So:

- `read_file /dev/zero` reads until the daemon runs out of memory.
- A FIFO blocks a tokio worker for ever. The turn's `timeout_at` cannot
  preempt a blocking read.
- A multi-GB log is read whole to return 200 lines.

### S7 (security, major): the `~/.daemoneye` guard misses a symlinked home

Three guards compare a **canonicalized** candidate path against the
**raw** `config_dir()` (`$HOME/.daemoneye`) with `starts_with`:

- `read.rs`, which blocks the credential files;
- `write.rs`, which blocks the whole directory for `edit_file`;
- `ops_copy.rs`, which blocks copies out of the directory.

When `~/.daemoneye` or a parent of `$HOME` is a symlink, the candidate
resolves to the real target. It then never starts with the symlinked
prefix:

- `read_file` returns `etc/config.toml`, the API key file;
- the `edit_file` block is bypassed the same way.

The copy destination is not checked at all.

## Behaviour

### `read_file`, local host

1. **Only regular files are read.** After resolving symlinks, the path must
   be a regular file; anything else gets
   `Error: <path> is not a regular file (read_file reads only regular files).`
   That covers:
   - devices, such as `/dev/zero` and `/dev/null`;
   - FIFOs;
   - sockets;
   - directories.

   The check runs on metadata, **before** the file is opened. Opening a
   FIFO blocks.
2. **The read is streamed, never read whole.**
   - Lines up to `offset + limit` are kept.
   - After the window, the reader keeps going only to **count** lines.
   - It never reads past `READ_BYTE_CAP` (8 MiB) in total.
3. **Totals.**
   - When EOF falls within the cap, the total is exact, as today. Every
     existing header, such as `(lines 1-10 of 300)`, is unchanged.
   - When the cap stops the count, the total is written `N+`, where `N` is
     the number of lines counted. For example, `(lines 1-10 of 1048576+)`.
     The same `N+` form appears in the pattern header and in the "no lines
     matched" message.
4. **Hitting the cap inside the window.**
   - If the cap is reached before the window is complete, return the
     complete lines read, followed by the line
     `[read stopped at the 8 MiB cap]`.
   - A partial line at the cap is dropped. If no complete line was read,
     the result is the path and that note alone.
5. **Bytes decode lossily** (`String::from_utf8_lossy`), so a binary
   regular file no longer errors.
6. **The whole local read runs in `tokio::task::spawn_blocking`:**
   canonicalize, metadata, open and stream.
7. **Unchanged:**
   - the pattern filter, which applies within the window;
   - masking;
   - `MAX_LINES` and `DEFAULT_LINES`;
   - the remote (`host`) branch.

### The `~/.daemoneye` guard

There are two new helpers in `src/daemon/executor/file_ops/mod.rs`.

- **`guard_root() -> PathBuf`** returns `canonicalize(config_dir())`. If
  that fails because the directory is absent, it falls back to
  `config_dir()`.
- **`in_daemoneye_dir(path: &str) -> bool`** resolves the path with the
  existing `resolve_path_for_guard`. It is true when the result starts with
  either `guard_root()` or the raw `config_dir()`.

Every guard goes through these helpers:

- **`read.rs`:** blocks the credential files when the resolved candidate
  equals either `guard_root()` or `config_dir()`, joined with
  `etc/config.toml` or `etc/prompts/sre.toml`.
- **`write.rs`:** uses `in_daemoneye_dir(path)`.
- **`ops_copy.rs`:** checks `in_daemoneye_dir` on **both** the source and
  `dest_path`. The destination check is new. Its error text is the same as
  for the source.

After this phase, none of those three files calls `config::config_dir()`
directly.

## Constraints

**The names below are fixed. Use them as given, and do not search the repo
for them: they do not exist yet.**

- **In `src/daemon/executor/file_ops/mod.rs`:** `guard_root` and
  `in_daemoneye_dir`.
- **In `read.rs`:**
  - `const READ_BYTE_CAP: usize = 8 * 1024 * 1024;`
  - `fn read_window(path: &Path, offset: usize, limit: usize) -> std::io::Result<ReadWindow>`
  - `struct ReadWindow { lines: Vec<String>, counted: usize, at_eof: bool, capped: bool }`
- **Never run a FIFO or `/dev/zero` read against the pre-fix code.** The
  FIFO blocks the test thread for ever, and `/dev/zero` exhausts memory.
  - The FIFO test carries a different prefix (`read_fifo_refused`), so the
    red run `cargo test read_bounds_` never reaches it. Write it only after
    the fix.
  - Do not use `/dev/zero` in any test. Use `/dev/null` for the device
    case.
- **Test harness:** the tests go in `read_tests.rs` and use its existing
  `TmpHome` / `with_home`. The large files are written to the `TmpHome`
  directory. Do not keep a 20 MiB `String` in memory: write it in chunks.

## Spec

1. **Red tests first.** Run them against the current tree, and keep the real
   output. Each one below fails as an assertion.
   - `read_bounds_refuses_device`: `/dev/null` gives the "not a regular
     file" error. Today it returns "no lines matched".
   - `read_bounds_refuses_directory`: the `TmpHome` directory itself gives
     the same error. Today it is "Error reading … Is a directory".
   - `read_bounds_large_file_total_is_open`: a 10 MiB file of short lines,
     with limit 10. The header shows lines 1-10, and the total ends in `+`.
   - `read_bounds_byte_cap_single_line`: a 20 MiB file with no newline. The
     result contains `[read stopped at the 8 MiB cap]` and is under 1 MiB.
   - `read_bounds_symlinked_home_blocks_config`: `HOME/.daemoneye` is a
     symlink to a real directory holding `etc/config.toml`. Read that file
     by its **real** path: you get the credential-files error. Today it
     returns the file.
   - `read_bounds_guard_follows_symlink`: under the same layout,
     `in_daemoneye_dir(<real path>/etc/x)` is true. This one is red only as
     a compile error.
2. **Implement,** running `cargo build` after each edit, in this order:
   1. the helpers, and the three guards (with the copy destination);
   2. `read_window` and the streamed local branch inside `spawn_blocking`;
   3. the file-type check.
3. **Write `read_fifo_refused`:** `mkfifo` (via `libc::mkfifo` or
   `std::process::Command::new("mkfifo")`) in `TmpHome`, then `read_file`
   it. You get the "not a regular file" error, and the call returns at
   once.
4. **Mutation check:** remove the file-type check, and run only
   `cargo test read_bounds_refuses_directory`. Confirm that it fails, then
   restore the check. **Do not run the FIFO test while the check is
   removed.** Paste the real result.
5. **Red runs:** add a `### Update — <date> (red runs)` entry with the real
   cargo output.
6. **Findings:** add a `### Findings — <date>` entry of two or three lines,
   or `none`.
7. **End-to-end:** run § End-to-end verification once, and paste its
   output.
8. **Commit** as
   `fix: read_file reads only regular files, streams under a byte cap off the runtime; the ~/.daemoneye guard compares canonical paths (M36 phase-14)`.

## Acceptance criteria

- [ ] `read_bounds passed=6 failed=0` and `read_fifo passed=1`. Today
      these tests do not exist.
- [ ] `whole_reads=0`: `read.rs` no longer calls `read_to_string`. Today it
      is 1.
- [ ] `blocking_off_runtime=1` or more: `read.rs` calls `spawn_blocking`.
      Today it is 0.
- [ ] `raw_guard_sites=0`: `read.rs`, `write.rs` and `ops_copy.rs` call
      `config::config_dir()` 0 times. Today it is 3.
- [ ] `copy_guards=2` or more: `ops_copy.rs` calls `in_daemoneye_dir(` for
      both the source and the destination. Today it is 0.
- [ ] The existing `read_file_` tests still pass, unmodified. Their headers
      are unchanged, because their files are under 8 MiB.
- [ ] Hygiene, `doc_truth` and `bug_tracker` are ok, `build_warnings=0`,
      clippy is clean, `fmt_exit=0` and `failed=0`.

## End-to-end verification

```sh
bash <<'E2E'
cargo test read_bounds_ 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "read_bounds passed=" p, "failed=" f}'
cargo test read_fifo_refused 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "read_fifo passed=" p, "failed=" f}'
grep -c 'read_to_string' src/daemon/executor/file_ops/read.rs | sed 's/^/whole_reads=/'
grep -c 'spawn_blocking' src/daemon/executor/file_ops/read.rs | sed 's/^/blocking_off_runtime=/'
cat src/daemon/executor/file_ops/read.rs src/daemon/executor/file_ops/write.rs src/daemon/executor/file_ops/ops_copy.rs | grep -c 'config::config_dir()' | sed 's/^/raw_guard_sites=/'
grep -c 'in_daemoneye_dir(' src/daemon/executor/file_ops/ops_copy.rs | sed 's/^/copy_guards=/'
touch src/main.rs && cargo build 2>&1 | grep -c '^warning' | sed 's/^/build_warnings=/'
for t in hygiene doc_truth bug_tracker; do cargo test --test $t 2>&1 | grep -E '^test result'; done
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1
cargo fmt --all --check; echo "fmt_exit=$?"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
E2E
```

## Authorizations

- **Edit these files:**
  - `src/daemon/executor/file_ops/mod.rs`
  - `read.rs`
  - `read_tests.rs`
  - `write.rs`
  - `ops_copy.rs`
- **Allowlist:** if the hygiene gate reports `run_read_file` over its cap,
  add or raise its entry, and give the reason in Findings.

## Out of scope

- The remote (`host`) read path, which already bounds its read with `sed`.
- `ops_copy`'s own `read_to_string` of the source, which the copy approval
  shows in full.
- The `edit_file` operations other than the guard.

## Update Log

<!-- entries appended below this line -->
