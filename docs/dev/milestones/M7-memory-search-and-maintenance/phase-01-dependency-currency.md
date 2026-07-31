# Phase 01: Dependency Currency

**Milestone:** M7 — Memory Search & Maintenance
**Status:** in-progress
**Depends on:** none
**Estimated diff:** ~15 lines in `Cargo.toml` (plus a regenerated `Cargo.lock`)
**Tags:** language=rust, kind=refactor, size=s

## Goal

Bring every direct dependency to its latest **stable** release and refresh the
lockfile's 132 stale transitive packages, so the rest of M7 builds on a current
base. Phase 06 adds `rusqlite`; adding it to a stale tree and then bumping
everything underneath it would mean re-validating finished work.

## Architecture references

None. This phase changes no code and no design. Read `docs/dev/STANDARDS.md`
§2.6 (Dependencies) and §5 (Files You Must Not Touch) — this phase is an
explicitly authorized exception to §5's build-file rule, and that authorization
is recorded under "Authorizations" below.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read this entire phase doc before touching any file.
3. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

`Cargo.toml` declares 23 direct dependencies plus 7 dev-dependencies. The
lockfile resolves 389 packages, of which **132 are behind** their latest
compatible release.

**The architect has already run this entire migration in a throwaway copy of the
tree at `HEAD`.** Build, clippy and the full test suite were green with exactly
the version requirements specified below. You are not exploring — you are
applying a verified change. If something fails, re-read the spec before changing
any source file; **no `.rs` file needs to be edited in this phase.**

### The three requirements that actually block an upgrade

Every other declared requirement is a caret range that already admits the latest
release, so `cargo update` alone moves it. These three do not:

| Crate | Declared | Latest stable | Why blocked |
|---|---|---|---|
| `toml` | `"0.8"` | `1.1.4+spec-1.1.0` | `0.8` → `1.x` is a major bump |
| `similar` | `"2"` | `3.1.1` | `2` → `3` is a major bump |
| `cron` | `"0.15"` | `0.17.0` | pre-1.0, so a minor bump is breaking |

Their usage surface in this repo is small, which is why all three migrate with
zero source changes:

- `toml` — only `toml::from_str` (24 sites), `toml::to_string_pretty` (5) and
  `toml::to_string` (3), across 7 files. All three are unchanged in `toml` 1.x.
- `similar` — one file, `src/cli/diff.rs`, importing `ChangeTag` and `TextDiff`
  (line 1) plus `similar::DiffTag::Equal` (line 49). All unchanged in 3.x.
- `cron` — one file, `src/scheduler.rs`, using `cron::Schedule` and
  `cron::error::Error` (line 17). Both unchanged in 0.17.

### The one dependency that must NOT be upgraded

`libc`'s latest published version is **`1.0.0-alpha.4`** — a pre-release, not a
stable release. The milestone's exit criterion says *latest stable*. Leave
`libc = "0.2"` alone and add the pin-back comment the spec requires. `cargo
update` will not pull a pre-release on its own; this note exists so you do not
"helpfully" raise the requirement by hand after seeing `1.0.0-alpha.4` in a
registry listing.

### A tooling trap that will give you wrong answers

**`cargo info <crate>` reports different versions depending on where you run
it.** Inside this repo it reports the version selected by the *current manifest
constraints*; outside a workspace it reports the true registry maximum:

```
$ cd /home/matt/src/daemoneye && cargo info clap | grep ^version
version: 4.5.60          # <- constrained by the manifest, NOT the latest

$ cd /tmp && cargo info clap | grep ^version
version: 4.6.4           # <- the actual registry latest
```

`cargo-outdated` is **not installed** on this host and you are **not** authorized
to install it. Do not attempt to. Everything this phase needs comes from
`cargo update` and the table above.

## Spec

1. **Raise the three blocking requirements** — in `Cargo.toml`, set
   `toml = "1.1"`, `similar = "3"`, and `cron = "0.17"`. Note that `toml`
   appears **twice** — once under `[dependencies]` and once under
   `[dev-dependencies]`. Change **both**. Leave `similar`'s and `cron`'s
   surrounding formatting alone.

2. **Refresh the six precise-pinned minimums** — in `Cargo.toml`, several
   requirements state an exact patch floor rather than a bare major. They
   document "the oldest release we have tested against", so refresh them to the
   versions this phase resolves:

   | Requirement | From | To |
   |---|---|---|
   | `anyhow` | `"1.0.102"` | `"1.0.104"` |
   | `nix` | `"0.31.1"` | `"0.31.3"` |
   | `reqwest` | `"0.13.2"` | `"0.13.4"` |
   | `serde` | `"1.0.228"` | `"1.0.229"` |
   | `serde_json` | `"1.0.149"` | `"1.0.151"` |
   | `tokio` | `"1.49.0"` | `"1.53.1"` |

   Keep each one's existing `features = [...]` / `default-features` settings
   exactly as they are — only the version string changes.

3. **Record the `libc` pin-back** — in `Cargo.toml`, add a single comment line
   directly above `libc = "0.2"`:

   ```toml
   # Pinned to 0.2: libc's only newer release is 1.0.0-alpha.4, a pre-release.
   libc = "0.2"
   ```

   This satisfies the milestone's exit criterion that any dependency held back
   carries a one-line note saying why.

4. **Regenerate the lockfile** — run `cargo update`. This moves 132 transitive
   packages. Do **not** hand-edit `Cargo.lock`; it is generated. Commit the
   resulting `Cargo.lock` along with `Cargo.toml`.

5. **Run the gates and confirm the baseline is unchanged** — build, clippy,
   format and test must all pass, and the test counts must match the M6 closing
   baseline exactly (see Acceptance criteria). A changed test count means
   something other than dependency versions moved, and is a bug in this phase.

## Acceptance criteria

- [ ] `cargo build` succeeds with zero new warnings.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` exits 0.
- [ ] `cargo fmt --all` leaves the tree unchanged.
- [ ] `cargo test` passes with **exactly** these counts, unchanged from M6's
      close: **991** lib tests, **30** integration (2 ignored), **8** isolation
      (1 ignored). No test is added, removed, or newly ignored by this phase.
- [ ] `grep -c '\[\[package\]\]' Cargo.lock` reports a package count, and
      `cargo update --dry-run` afterwards reports **no** further `Updating`
      lines for any package.
- [ ] `Cargo.toml` contains `toml = "1.1"` in **both** `[dependencies]` and
      `[dev-dependencies]`, `similar = "3"`, and `cron = "0.17"`.
- [ ] `Cargo.toml` contains the `libc` pin-back comment from spec task 3.
- [ ] No `.rs` file is modified by this phase — `git diff --stat` lists only
      `Cargo.toml` and `Cargo.lock`.

## Test plan

**No new tests.** This phase adds no code, no function, and no behavior — it
changes version requirements only. STANDARDS §3.1 requires tests for new
functions, parsers and integrations; none of those appear here. The existing
1029-test suite *is* the regression test for a dependency sweep, and criterion 4
pins its counts exactly.

Do **not** invent a test that asserts dependency versions by parsing
`Cargo.toml`. It would restate the manifest, break on every future bump, and
test cargo rather than this project.

## End-to-end verification

The real artifact is the built binary and the resolved lockfile. Run this block
verbatim and paste the resulting file's contents into your Update Log entry:

```bash
cd /home/matt/src/daemoneye
{
  echo "=== git diff --stat (must list ONLY Cargo.toml and Cargo.lock) ==="
  git diff --stat
  echo "exit=$?"

  echo "=== the four changed requirements ==="
  grep -nE '^(toml|similar|cron|libc) = ' Cargo.toml
  echo "exit=$?"

  echo "=== libc pin-back comment present ==="
  grep -B1 '^libc = ' Cargo.toml
  echo "exit=$?"

  echo "=== resolved versions of the three major bumps ==="
  grep -A1 '^name = "toml"$'    Cargo.lock
  grep -A1 '^name = "similar"$' Cargo.lock
  grep -A1 '^name = "cron"$'    Cargo.lock
  echo "exit=$?"

  echo "=== no further updates available ==="
  cargo update --dry-run 2>&1 | grep -E '^\s+Updating' | grep -v 'crates.io index'
  echo "grep-exit=$?   # 1 == nothing left to update == PASS"

  echo "=== the binary actually runs against the new tree ==="
  cargo build 2>&1 | tail -3
  ./target/debug/daemoneye --version
  echo "exit=$?"

  echo "=== full gate ==="
  cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -3
  echo "clippy-exit=$?"
  cargo test 2>&1 | grep -E '^test result'
  echo "exit=$?"
} > /tmp/phase01-e2e.txt 2>&1
cat /tmp/phase01-e2e.txt
```

Note the `grep-exit=1` case above: "no further updates available" is proven by
**empty** output, so the exit marker is the whole proof. An empty block with no
marker demonstrates nothing.

Paste the captured file into an Update Log entry titled
`### Update — <date> (end-to-end verification)`. **The server-authored
`(complete)` entry does not satisfy this** — its "Command output tails" block is
the automatic gate capture every phase receives, and it shows that build/lint/test
ran, not that this phase's acceptance criteria were exercised.

## Authorizations

- [x] **May modify `Cargo.toml` and `Cargo.lock`** — STANDARDS §5 otherwise
      forbids touching build/config files. That is the entire point of this
      phase, so it is authorized here.
- [ ] May add dependencies: **none**. Changing an existing requirement's version
      is not adding a dependency. `rusqlite` arrives in phase 06, not here — do
      not add it now even though the milestone README mentions it.
- [ ] May touch `docs/architecture.md`: no.

## Out of scope

- **Adding `rusqlite`.** That is phase 06, and it carries its own authorization.
- **Upgrading `libc` to `1.0.0-alpha.4`.** Pre-release, explicitly held back.
- **Installing `cargo-outdated`** or any other new host binary.
- **The duplicate `nix` in the lockfile.** `nix 0.29.0` is pulled transitively
  by `mac_address` and `termwiz` alongside our direct `nix 0.31.3`. We do not
  control those crates' requirements; the duplicate is expected and is not a
  defect to chase.
- **Editing any `.rs` file.** The verified migration needs none. If you believe
  one is required, that is a spec-versus-reality conflict — **file a blocker**
  per STANDARDS §7 rather than improvising a source change.
- **Reformatting or reordering `Cargo.toml`.** Change the version strings and
  add the one comment; leave everything else, including the existing `ratatui`
  comment block, byte-identical.
- **Tightening the loose requirements** (`regex = "1"`, `uuid = "1"`,
  `zeroize = "1"`, `tempfile = "3"`, `log = "0.4"`, and similar). They already
  admit the latest release, so pinning them tighter is churn with no benefit and
  makes future bumps noisier.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-07-31 16:30 (started)

**Executor:** Claude (Anthropic)

Raising `toml` to `1.1`, `similar` to `3`, `cron` to `0.17`; refreshing six
precise-pinned minimums; recording the `libc` pin-back comment; regenerating
`Cargo.lock`.

### Update — 2026-07-31 16:30 (end-to-end verification)

```
=== git diff --stat (must list ONLY Cargo.toml and Cargo.lock) ===
 Cargo.lock                                         | 1265 ++++++++------------
 Cargo.toml                                         |   21 +-
 .../M7-memory-search-and-maintenance/README.md     |    2 +-
 .../phase-01-dependency-currency.md                |   10 +-
 4 files changed, 504 insertions(+), 794 deletions(-)
exit=0
=== the four changed requirements ===
17:cron = "0.17"
22:libc = "0.2"
35:similar = "3"
37:toml = "1.1"
47:toml = "1.1"
exit=0
=== libc pin-back comment present ===
# Pinned to 0.2: libc's only newer release is 1.0.0-alpha.4, a pre-release.
libc = "0.2"
exit=0
=== resolved versions of the three major bumps ===
name = "toml"
version = "1.1.4+spec-1.1.0"
name = "similar"
version = "3.1.1"
name = "cron"
version = "0.17.0"
exit=0
=== no further updates available ===
grep-exit=1   # 1 == nothing left to update == PASS
=== the binary actually runs against the new tree ===
    Finished `dev` profile [unoptimized] target(s) in 0.07s
daemoneye 0.9.9
exit=0
=== full gate ===
    Finished `dev` profile [unoptimized] target(s) in 0.07s
clippy-exit=0
test result: ok. 991 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 4.39s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 30 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.02s
test result: ok. 8 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.14s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
exit=0
```

Test counts match M6 baseline exactly: **991** lib tests, **30** integration (2 ignored), **8** isolation (1 ignored).
