# Phase 02 — rusqlite 0.41 and syn 3

**Status:** todo
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
  `.../syn-3.0.9/`. Their `CHANGELOG.md` (where present) and source are the
  reference if something fails.

## Spec

1. **Raise the requirements** — edit `Cargo.toml`: rusqlite to `0.41.1`,
   syn to `3.0.9`. Run `cargo update -p rusqlite -p syn` (and
   `-p proc-macro2` if needed). Do not run a bare `cargo update`.
2. **Fix what breaks** — build, clippy and test. Fix any failure in the
   files named in the Goal, using the changed API rather than a shim.
   A hygiene test that fails because syn 3 counts or visits differently
   is a real finding: report the cause, and keep the gate's meaning
   (the same caps, the same exemptions); do not loosen it to pass.
3. **Findings** — one `### Findings — <date>` entry: anything the upgrade
   changed in behaviour (e.g. a different line count from the hygiene
   gate, a rusqlite default that moved), or `none`.
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
