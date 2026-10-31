# Phase 02 — rusqlite 0.41 and syn 3

**Status:** in-progress
**Milestone:** M33 — Dependency refresh
**Drafted:** 2026-10-30
**Depends on:** phase 01 (done)
**Tags:** language=rust, kind=chore, size=s

## Goal

The two breaking upgrades that the survey found build clean land here,
so a failure points at one of two crates (M33 decision 2):

- **rusqlite** 0.40 → **0.41.1** (normal dependency, `bundled` feature
  kept). Its only users are `src/memory/index/` (`mod.rs`, `search.rs`,
  `reconcile.rs`, `tests.rs`).
- **syn** 2.0.119 → **3.0.9** (dev dependency, `full` and `visit` features
  kept). Its only user is `tests/hygiene.rs`, which parses every `.rs` file
  and enforces the line caps. Raise `proc-macro2` too if syn 3 requires a
  newer one.

When this phase is done, `Cargo.toml` requires both new versions,
`Cargo.lock` resolves them, and the suite still passes **exactly 2616**
tests (no test is added or removed).

## Pre-flight

- The survey's trial build of both upgrades compiled with no error. The
  memory-index tests and the hygiene gate have not been run against them;
  that is this phase's real check.
- You have no web access, but the new crates' source is on disk after the
  first build, under `~/.cargo/registry/src/*/rusqlite-0.41.1/` and
  `.../syn-3.0.9/`. Open it **only** to fix a failing build, clippy or
  test. If the gates are green, there is nothing to look up: do not diff
  or audit the crates.

## Spec

1. **Raise the requirements** — edit `Cargo.toml`: rusqlite to `0.41.1`,
   syn to `3.0.9`. Run `cargo update -p rusqlite -p syn` (and
   `-p proc-macro2` if needed). Do not run a bare `cargo update`.
2. **Fix what breaks** — build, clippy and test. Fix any failure in the
   files named in the Goal, using the changed API rather than a shim.
   A hygiene test that fails because syn 3 counts or visits differently
   is a real finding: report the cause, and keep the gate's meaning
   (the same caps, the same exemptions); do not loosen it to pass.
3. **Findings** — one `### Findings — <date>` entry, from exactly two
   commands and nothing else: `git diff --stat` and
   `git diff Cargo.lock | grep -E '^[-+](name|version) '`. Write in two or
   three lines what moved in the lockfile, and any gate that failed and
   how you fixed it — or `none`. Open no crate source for this step.
4. **End-to-end** — run the block below and paste its output, fenced, in an
   `### Update — <date> (end-to-end verification)` entry.
5. **Commit** as `chore: upgrade rusqlite to 0.41 and syn to 3`.

## Acceptance criteria

- [ ] The E2E block's first line prints `rusqlite=0.41.1 syn=3.0.9 ` (the direct dependencies, per `cargo tree --depth 1`)
      (today `rusqlite=0.40.2 syn=2.0.119`).
- [ ] `allowlist=unchanged`, `build_warnings=0`, clippy clean,
      `fmt_exit=0`.
- [ ] `tests/hygiene.rs` and `tests/doc_truth.rs` pass, unchanged in what
      they enforce.
- [ ] `cargo test` reports exactly `passed=2616 failed=0`.
- [ ] No new `#[allow]`, `#[ignore]`, `unwrap()` in production, or
      `unsafe`.

## End-to-end verification

```sh
bash <<'E2E'
cargo tree --depth 1 -e normal,dev | grep -E '── (rusqlite|syn) v' | awk '{print $2"="$3}' | tr -d v | sort | tr '\n' ' '; echo
git diff --quiet HEAD -- tests/hygiene_allowlist.txt && echo allowlist=unchanged
touch src/main.rs && cargo build 2>&1 | grep -c '^warning' | sed 's/^/build_warnings=/'
cargo test --test hygiene 2>&1 | grep -E '^test result'
cargo test --test doc_truth 2>&1 | grep -E '^test result'
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1
cargo fmt --all --check; echo "fmt_exit=$?"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
E2E
```

(The lockfile already holds a syn 3.0.9 pulled in by another crate, and
syn 2 may stay for others; the first line reads only DaemonEye's direct
dependencies.)

## Authorizations

- Edit `Cargo.toml`, `Cargo.lock` (via `cargo update -p`), the four
  `src/memory/index/` files and `tests/hygiene.rs`.

## Out of scope

- vt100 0.18 (phase 03). Any other crate's version.
- `tests/hygiene_allowlist.txt`: no entry may be added or grown.

## Update Log

<!-- entries appended below this line -->

### Notes for executor — 2026-10-30

The first run (session-phase-02-6ae5287f) did steps 1 and 2 correctly, then
stalled after ~120 read-only calls diffing the rusqlite and syn sources for
Findings. The architect re-ran the gates on that tree: fmt clean,
`build_warnings=0`, clippy clean, `passed=2616 failed=0`, `cargo tree` at
rusqlite 0.41.1 and syn 3.0.9. **The upgrade is done; do not redo or audit
it.** What remains is steps 3–5 only: the Findings entry from the two
commands step 3 names (the lockfile also moved `libsqlite3-sys` to 0.39.1
and `tempfile`'s `getrandom` from 0.4.3 to 0.3.4 — say so), the E2E block
pasted, and the commit. Steps 3 and Pre-flight were tightened so that the
crate sources are read only to fix a red gate.

### Update — 2026-10-30 (escalation)

**Chosen lever:** resume
**Rationale:** the stalled tree is complete and green against every gate,
so only the bookkeeping is left; the one spec gap (an open-ended Findings
audit) is fixed in the doc that the resumed run reads.

### Update — 2026-10-30 23:37 (started)

Executor rexy (claude-opus-4-6). Status flipped `todo → in-progress` here and in
the milestone README's phase table. Next: raise the rusqlite and syn
requirements in `Cargo.toml`, `cargo update -p` them, then build, clippy and
test against the memory index and the hygiene gate.


### Findings — 2026-10-30

`git diff --stat`: only `Cargo.toml` and `Cargo.lock` carry the upgrade
(4 and 12 lines); no source file changed, so nothing in rusqlite 0.41 or
syn 3 broke the build, clippy or tests. `git diff Cargo.lock`:
rusqlite 0.40.2 → 0.41.1, with libsqlite3-sys 0.38.2 → 0.39.1 underneath,
and syn 2.0.119 → 3.0.9; tempfile's getrandom also moved 0.4.3 → 0.3.4
(resolution side-effect, not a requirement change). Gates: none failed —
build, clippy, fmt, hygiene, doc_truth and the full suite (passed=2616
failed=0) were already green on this tree.

### Update — 2026-10-30 (end-to-end verification)

```text
rusqlite=0.41.1 syn=3.0.9 
allowlist=unchanged
build_warnings=0
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.78s
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.04s
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.43s
fmt_exit=0
passed=2616 failed=0
```
