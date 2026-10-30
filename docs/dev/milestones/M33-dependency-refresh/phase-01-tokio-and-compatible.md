# Phase 01 — Tokio and compatible updates

**Status:** in-progress
**Milestone:** M33 — Dependency refresh
**Drafted:** 2026-10-30
**Depends on:** M32 (v2.5.0)
**Estimated diff:** ~95 insertions, ~70 deletions outside `Cargo.lock`
**Tags:** language=rust, kind=chore, size=s

## Goal

Every semver-compatible dependency is at its latest stable release, and
tokio 1.54's deprecation of `AsyncFd::new` is gone. After this phase:

- **`Cargo.toml`** names the latest release in every requirement except
  rusqlite, syn and vt100, which are phases 02 and 03 (`libc` stays on
  0.2, M33 README). The `[dev-dependencies]` entries for serde,
  serde_json, toml and uuid go: they repeat the normal entries and add
  nothing. `Cargo.lock` resolves tokio 1.54.1.
- **`AsyncStdin`** (`src/cli/input/tty.rs`) holds an owned `File` opened
  with `O_NONBLOCK`, registered through `AsyncFd::register`. That is the
  one `unsafe` block M33 decision 1 authorizes, with its `// SAFETY:`
  comment. The hand-rolled `TtyFd`, with its `libc::open`, `libc::close`
  and `libc::read`, goes, so the file ends with fewer `unsafe` blocks than
  it started with. `read_byte_waits_out_a_drained_pipe` pins the
  `WouldBlock` arm of `read_byte`, which no test reached before.
- **`idle_stream_stall_is_reported_by_stream_next_with_timeout`**
  (`src/ai/mod.rs`): its fake server reads the request before it answers.

The suite goes from **2615** to **exactly 2616**: one test added.

## Architecture references

- M33 README: the survey table and decisions 1 and 2.
- Excerpt from tokio 1.54.1 (`src/io/async_fd.rs`), which you cannot fetch:
  `AsyncFd::{new, with_interest, try_new, try_with_interest}` are
  deprecated as "unsound due to lack of I/O safety" (tokio PR #8587). Their
  replacement is `pub unsafe fn register(inner: T) -> Result<Self,
  AsyncFdRegisterError<T>>`. Its safety contract: the fd that
  `inner.as_raw_fd()` returns "must be a valid, open file descriptor, and
  it must remain open and refer to the same underlying file description
  for the entire lifetime of the `AsyncFd`". `io::Error` implements
  `From<AsyncFdRegisterError<T>>`.

## Spec

1. **Run this, once, as your first command after the started entry:**

   ```sh
   python3 docs/dev/milestones/M33-dependency-refresh/scripts/phase-01-tokio-and-compatible.py && cargo update && cargo fmt --all
   ```

   **Do not open the script, and do not read any source file before
   running it.** It was run on a scratch worktree of the commit that
   drafted this phase: fmt and clippy clean, a build with no warning,
   hygiene and doc_truth passing, the full suite at
   `passed=2616 failed=0`. It asserts its own anchors and prints four
   lines, the last `read_byte_waits_out_a_drained_pipe pins the WouldBlock
   arm`. If an assert fires, stop and report it in the Update Log. Do not
   hand-edit anything it touches, and do not edit the script.
   `cargo update` reads crates.io as it runs. If it moves a crate past the
   versions the dry run saw (the shape line below), say so in Findings.
2. **Log it at once.** Append an `### Update — <date> (cuts done)` entry
   quoting the script's output, with an edit tool.
3. **Findings** go in their own `### Findings — <date>` entry. Run
   exactly `git diff --stat`, write what it shows in one or two lines, or
   write `none`. Open no file.
4. **Run the End-to-end block** and paste its output into an
   `### Update — <date> (end-to-end verification)` entry, as a fenced
   block of the output itself, not a description of it.
5. **Commit** as `chore:`.

The script is
`docs/dev/milestones/M33-dependency-refresh/scripts/phase-01-tokio-and-compatible.py`
(sha256 begins `7f03310aa9d1f0db`), kept in the repo as the record of the
change.

### The red run is done — do not repeat it

At drafting, with the lockfile updated and no source changed:

```
error: use of deprecated associated function `tokio::io::unix::AsyncFd::<T>::new`: unsound due to lack of I/O safety; use `AsyncFd::register` instead (see https://github.com/tokio-rs/tokio/pull/8587)   (x2, clippy -D warnings)
idle_stream_stall_is_reported_by_stream_next_with_timeout ... FAILED
  reqwest::Error { kind: Request, … hyper::Error(Canceled, hyper::Error(UnexpectedMessage)) }
```

The test failure bisects to tokio: with hyper, hyper-util and reqwest
each pinned back it still fails, and with tokio alone back at 1.53.1 it
passes. With the script applied, the WouldBlock arm mutated to
`return None` fails `read_byte_waits_out_a_drained_pipe` ("a drained pipe
is not EOF") and passes every other test, which is why the test is added.

## Acceptance criteria

- [ ] The End-to-end block prints
      `asyncfd_new=0 register=1 libc_io=0 drained_test=1 reads_request=1 tokio_lock=1.54.1 dev_dups=0`
      (today: `asyncfd_new=2 register=0 libc_io=3 drained_test=0
      reads_request=0 tokio_lock=1.53.1 dev_dups=4`).
- [ ] `cargo build` prints no `warning:` line; clippy is clean.
- [ ] `cargo test` reports **exactly** `passed=2616 failed=0` (today
      2615).
- [ ] Preservation: `tests/hygiene.rs` and `tests/doc_truth.rs` pass;
      `tests/hygiene_allowlist.txt` is unchanged; fmt is clean.
- [ ] `grep -c '^### Findings — '` on this doc prints `1`, and the
      end-to-end entry holds the fenced output.

## End-to-end verification

```sh
bash <<'E2E'
echo "asyncfd_new=$(grep -rc 'AsyncFd::new(' src | awk -F: '{s+=$2} END {print s}') register=$(grep -c 'AsyncFd::register(' src/cli/input/tty.rs) libc_io=$(grep -cE 'libc::(open|read|close)\(' src/cli/input/tty.rs) drained_test=$(grep -c 'fn read_byte_waits_out_a_drained_pipe' src/cli/input/tty.rs) reads_request=$(grep -c 'request.windows(4)' src/ai/mod.rs) tokio_lock=$(grep -A1 '^name = "tokio"$' Cargo.lock | tail -1 | cut -d'"' -f2) dev_dups=$(sed -n '/^\[dev-dependencies\]/,$p' Cargo.toml | grep -cE '^(serde|serde_json|toml|uuid) ')"
cargo tree --depth 1 -e normal,dev | grep -E '── (tokio|clap|reqwest|serde_json|similar|toml|uuid) v'
git diff --quiet HEAD -- tests/hygiene_allowlist.txt && echo allowlist=unchanged
touch src/main.rs && cargo build 2>&1 | grep -c '^warning' | sed 's/^/build_warnings=/'
cargo test --test hygiene 2>&1 | grep -E '^test result'
cargo test --test doc_truth 2>&1 | grep -E '^test result'
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1
cargo fmt --all --check; echo "fmt_exit=$?"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
E2E
```

Expected: the shape line above; the dependency tree at clap 4.6.7,
reqwest 0.13.5, serde_json 1.0.154, similar 3.2.1, tokio 1.54.1 (twice),
toml 1.1.8, uuid 1.29.0; `allowlist=unchanged`; `build_warnings=0`;
hygiene `ok. 16 passed`; doc_truth `ok. 16 passed`; clippy clean;
`fmt_exit=0`; `passed=2616 failed=0`. If the suite stops early on a
failure, paste that output, run the block once more and paste both.

## Authorizations

- Run the script; do not edit it. Run `cargo update`.
- Edit, through the script only: `Cargo.toml`, `src/cli/input/tty.rs`,
  `src/ai/mod.rs`. `Cargo.lock` changes through `cargo update`.
- The one new `unsafe` block in `tty.rs` (`AsyncFd::register`), per M33
  decision 1. The script adds it; you add none.

## Out of scope

- rusqlite 0.41 and syn 3 (phase 02), vt100 0.18 (phase 03).
- The transitive crates `cargo update` leaves behind (generic-array,
  matchit, psm): their dependents pin them.
- The other fake HTTP servers in the test suite. Only this one failed.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-10-30 23:13 (started)

Executor rexy (claude-opus-4-6). Status flipped `todo → in-progress` here and in
the milestone README's phase table. Next: run the phase script once, as the first
command after this entry, then log its output.

### Update — 2026-10-30 23:13 (cuts done)

Script + `cargo update` + `cargo fmt --all` ran as one command, exit 0. The
script printed its four anchor lines; full command output captured verbatim
in `.rexymcp/output/cmd-output-0.log` (the run's own log file — that path
does not survive the run, so the two excerpts below are copied from it, in
order, without paraphrase):

```
Cargo.toml requires the latest of every compatible dependency
AsyncStdin owns a File registered with AsyncFd::register
the stream-idle fake server reads the request before it answers
read_byte_waits_out_a_drained_pipe pins the WouldBlock arm
```

`cargo update` then printed `Locking 137 packages to latest Rust 1.97.1
compatible versions` and moved the crates this phase targets, among them:

```
    Updating clap v4.6.4 -> v4.6.7
    Updating reqwest v0.13.4 -> v0.13.5
    Updating serde_json v1.0.151 -> v1.0.154
    Updating similar v3.1.1 -> v3.2.1
    Updating tokio v1.53.1 -> v1.54.1
    Updating toml v1.1.4+spec-1.1.0 -> v1.1.8+spec-1.1.0
    Updating uuid v1.24.0 -> v1.29.0
```

and ended with `note: pass `--verbose` to see 4 unchanged dependencies behind
latest`. `cargo fmt --all` printed nothing. `git diff --stat` after the run:
`Cargo.lock 709`, `Cargo.toml 50`, `src/ai/mod.rs 14`, `src/cli/input/tty.rs
112`, plus this doc and the milestone README (my two status edits).

### Findings — 2026-10-30

`git diff --stat`: `Cargo.lock | 709 ++-`, `Cargo.toml | 50 +-`, `src/ai/mod.rs
| 14 +-`, `src/cli/input/tty.rs | 112 ++--`, `README.md | 2 +-`,
`phase-01-tokio-and-compatible.md | 8 +-`; 6 files changed, 492 insertions(+),
403 deletions(-). Beyond that, none.

### Update — 2026-10-30 23:18 (end-to-end verification)

Ran the phase doc's `bash <<'E2E'` block verbatim, unmodified, once. Its
complete output:

````
asyncfd_new=0 register=1 libc_io=0 drained_test=1 reads_request=1 tokio_lock=1.54.1 dev_dups=0
├── clap v4.6.7
├── reqwest v0.13.5
├── serde_json v1.0.154
├── similar v3.2.1
├── tokio v1.54.1
├── toml v1.1.8+spec-1.1.0
├── uuid v1.29.0
└── tokio v1.54.1 (*)
allowlist=unchanged
build_warnings=0
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.01s
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.04s
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 30.86s
fmt_exit=0
passed=2616 failed=0
````

Every line matches the expected shape: the criterion line exactly
(`asyncfd_new=0 register=1 libc_io=0 drained_test=1 reads_request=1
tokio_lock=1.54.1 dev_dups=0`), the dependency tree at clap 4.6.7, reqwest
0.13.5, serde_json 1.0.154, similar 3.2.1, tokio 1.54.1 twice, toml 1.1.8,
uuid 1.29.0; `allowlist=unchanged`; `build_warnings=0`; hygiene and doc_truth
both `ok. 16 passed`; clippy's tail `Finished` with no diagnostics;
`fmt_exit=0`; `passed=2616 failed=0`. The suite did not stop early, so there
is no second paste.
