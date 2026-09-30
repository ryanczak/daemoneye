# Phase 01a: pty-spawn-transient — a PTY shell never fails to spawn because `$HOME` is gone, and the tests stop racing `HOME`

**Milestone:** M23 — Client decoupling & shell UX
**Status:** todo
**Depends on:** none (M21 carry-forward)
**Estimated diff:** ~10 lines of production code in 1 file; ~30 lines of test changes in 2 files; 2 new tests
**Tags:** language=rust, kind=bugfix, size=s

## Goal

M21 08e recorded an intermittent `spawn_command failed: No such file or
directory (os error 2)` from `PtyShell::spawn` under full-suite load. It
blamed portable-pty's `TIOCSCTTY` pre-exec and flagged production as exposed.
It worked around the failure with a test-only spawn retry.

**That diagnosis was wrong, and this phase fixes the real cause.** portable-pty
starts every child in **`$HOME`** when no working directory is set. The tests
point `HOME` at temp directories and delete them. A spawn that sees a deleted
`HOME` fails to `chdir`, and the failed `chdir` is the `ENOENT`. After this
phase:

1. **A PTY shell starts in `$HOME` if it is a directory, and in `/`
   otherwise.** It never fails to spawn because `HOME` is missing. (This is
   also a real product edge: a daemon whose `HOME` is not a directory could
   not open any shell.)
2. **The shell tests that spawn PTYs serialise against the tests that mutate
   `HOME`,** so the race is gone rather than retried.
3. **The test-only retry is deleted,** and the two comments that carry the
   wrong diagnosis are corrected.

## Architecture references

- `docs/dev/milestones/M23-client-decoupling/README.md` — row 01a.
- `docs/dev/milestones/M21-host-model/README.md` § Retrospective → "Carried
  forward" (the claim this phase corrects).

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read this entire phase doc before touching any code.
3. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

Measured on **2026-09-30** at commit `05998c1b`.

Baseline: `cargo test --lib` → **2088 passed / 0 failed / 14 ignored**.

### 1. Where the child's working directory comes from

`PtyShell::spawn` (`src/shell/pty.rs:208-226`) never sets a working
directory:

```rust
        let mut cmd = CommandBuilder::new(shell);
        cmd.args(args);
        cmd.env("TERM", "xterm-256color");
        let child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| anyhow::anyhow!("{e}"))
            .context("spawn_command failed")?;
```

portable-pty 0.9.0's `CommandBuilder::as_command` then falls back to the
`HOME` it captured from the environment (quoted from the crate source,
`src/cmdbuilder.rs:501-526`):

```rust
        let home = self.get_home_dir()?;
        let dir: &OsStr = self
            .cwd
            .as_ref()
            .map(|dir| dir.as_os_str())
            .filter(|dir| std::path::Path::new(dir).is_dir())
            .unwrap_or(home.as_ref());
        // ...
        cmd.current_dir(dir);
```

So with no `cwd`, the child `chdir`s into `$HOME`. If `$HOME` does not
exist, the `chdir` fails and `spawn` returns `ENOENT`.

### 2. Measured

All four runs used a throwaway in-crate probe, since reverted:

| Case | Result |
|---|---|
| `HOME` set to a directory that does not exist, then `PtyShell::spawn("/bin/sh", ["-c","exit 0"])` | `Err("spawn_command failed: No such file or directory (os error 2)")` — **the exact 08e string, deterministically** |
| Same, `HOME` an existing temp dir | spawns |
| With the fix in Task 1: `HOME` missing, `bash` spawned, then `run("pwd")` | `"/"` |
| With the fix: `HOME` an existing temp dir, `run("pwd")` | the temp dir's path |

**The fix alone is not enough.** With Task 1 applied and the test retry
removed, the full suite still failed in **2 of 3** runs, on the connect
tests, with the same `ENOENT`. The cause is a check-then-use race: the fix
sees an existing `HOME`, then a HOME-mutating test deletes it before the
child's `chdir`. Only adding Task 2 (the connect tests hold
`crate::test_home_guard()`) made it **5 of 5** green. That run used Task 1 +
Task 2 + the retry removed, and clippy was clean apart from the probe's own
`let_and_return`, which Task 3 avoids.

### 3. The tests that race, and the retry that hides it

`src/daemon/executor/shell_exec.rs`, module `connect_tests`. Six tests begin
with this line, and **none** holds the HOME guard:

```rust
        let _pty = PTY_TEST_LOCK.lock().await;
```

`PTY_TEST_LOCK` (`:2755`) only serialises those six tests against **each
other**. Its doc comment (`:2751-2754`) and `drive_fake`'s (`:2830-2836`)
both state the wrong `TIOCSCTTY` diagnosis. `drive_fake` (`:2837`) retries a
failed spawn once:

```rust
        match outcome {
            Err(msg) if msg.contains("could not be spawned") => {
                tokio::time::sleep(Duration::from_millis(20)).await;
                drive_connect(
                    // ... the same arguments again ...
                )
                .await
            }
            other => other,
        }
```

`crate::test_home_guard()` (`src/lib.rs`) is the serialiser every
HOME-mutating test takes. Holding it across `.await` in these tests is
already done elsewhere in this module (the live `pinky` test's `test_home()`
helper) and passes clippy.

## Spec

### Task 1 — spawn in `$HOME`, or `/` if it is gone (`src/shell/pty.rs`)

In `PtyShell::spawn`, after `cmd.env("TERM", "xterm-256color");`, set an
explicit working directory:

```rust
        // portable-pty starts the child in $HOME when no cwd is set; a HOME
        // that is not a directory makes the child's chdir fail and the whole
        // spawn return ENOENT. Start in HOME when it exists, else in `/`.
        let home = std::env::var_os("HOME")
            .map(std::path::PathBuf::from)
            .filter(|h| h.is_dir());
        cmd.cwd(home.unwrap_or_else(|| std::path::PathBuf::from("/")));
```

Nothing else in `spawn` changes.

### Task 2 — the connect tests serialise against HOME mutation (`src/daemon/executor/shell_exec.rs`)

In `connect_tests`, in **each** of the six tests whose first line is
`let _pty = PTY_TEST_LOCK.lock().await;`, add as the very next line:

```rust
        let _home = crate::test_home_guard();
```

Keep `PTY_TEST_LOCK`. Rewrite its doc comment so it says what is true: it
serialises this module's PTY tests against each other, and the
`test_home_guard()` each one also takes keeps a HOME-mutating test from
deleting `HOME` under a spawn.

### Task 3 — delete the retry and the wrong diagnosis (`src/daemon/executor/shell_exec.rs`)

1. In `drive_fake`, delete the whole `match outcome { … }` retry and return
   the `drive_connect(…).await` result **directly**. Do not bind it to a
   `let` and return that: clippy's `let_and_return` rejects it.
2. Rewrite `drive_fake`'s doc comment: drop the retry and `TIOCSCTTY`
   sentences, keep the description of what it does.

### Task 4 — tests (`src/shell/pty.rs` tests)

Two tests, names pinned. Each holds `crate::test_home_guard()` for its whole
body, as `pty_stop_with_no_foreground_job_remembers_nothing` does
(`let _home = crate::test_home_guard();`), and **also** saves `HOME` first and
restores it at the end. The neighbours do not restore it, but these two point
`HOME` somewhere unusual on purpose:

1. `a_missing_home_still_opens_a_shell` — set `HOME` to a path under
   `std::env::temp_dir()` that does not exist (remove it first if present),
   `PtyShell::spawn("bash", &[], (24, 80))` → `Ok`, then
   `run("pwd", Duration::from_secs(10))`. The output, trimmed, is `"/"`.
2. `a_shell_still_starts_in_home` — set `HOME` to a fresh `tempfile::tempdir()`,
   spawn and run `pwd`. The output, trimmed, equals the temp dir's path.

**Test count:** two new, so **2090 passed / 14 ignored**.

### Task 5 — capture the end-to-end evidence

Run the block in § End-to-end verification verbatim. Paste its output **into
this phase doc** as the first fenced block of a new Update Log entry titled
exactly `### Update — <date> (end-to-end verification)`. A summary in your
final message is not the entry. Then run the paste check and put its literal
`PASTE MATCH` line in the entry.

## Acceptance criteria

Parenthesised values are what each line produces **against the current
tree**. Every non-preservation line fails today; each was run at drafting.

- [ ] `cargo test --lib` → **2090 passed / 0 failed / 14 ignored** on three
      consecutive runs, **with the retry deleted** (current: 2088/0/14, with
      the retry in place). Both counts are exact.
- [ ] `cargo fmt --all` clean; `cargo build` clean;
      `cargo clippy --all-targets --all-features -- -D warnings` exits 0.
- [ ] `cargo test` → every other target unmoved: bug_tracker 6, doc_truth 14,
      integration 32, isolation 10 (*preservation*).
- [ ] The two new tests pass:
      ``cargo test --lib -- a_missing_home_still_opens_a_shell a_shell_still_starts_in_home``
      → **2 passed** (current: `0 passed; … filtered out`).
- [ ] The retry is gone: ``grep -c 'could not be spawned' src/daemon/executor/shell_exec.rs``
      → **1**, the production format string only (current: **2**).
- [ ] Every PTY-locking connect test holds the HOME guard:
      ``grep -A1 'PTY_TEST_LOCK.lock().await' src/daemon/executor/shell_exec.rs | grep -c 'test_home_guard'``
      → **6** (current: **0**).
- [ ] The wrong diagnosis is gone: ``grep -c 'TIOCSCTTY' src/daemon/executor/shell_exec.rs``
      → **0** (current: **1**).
- [ ] The PTY module's own tests still pass (*preservation*):
      ``cargo test --lib shell::pty`` → **21 passed** (19 today + the 2 new).
- [ ] Mutation A was applied, run, and its observed result recorded.
- [ ] The § End-to-end verification block was run verbatim and pasted as the
      **first fenced block** of the `(end-to-end verification)` entry, with
      the literal `PASTE MATCH`:
      ``grep -c '^### Update — .* (end-to-end verification)$' <this doc>``
      → **1** (current: 0).

## Mutations

Apply with the `patch` tool (the executor contract forbids `sed -i`,
`perl -i`, `> file` and `rm -rf`), run once, restore, and record the observed
assertion values.

- **Mutation A:** remove the `unwrap_or_else(|| … "/")` fallback, so `cwd` is
  set only when `HOME` is a directory (`if let Some(h) = home { cmd.cwd(h); }`).
  Run test 1. Expected **red** with `spawn_command failed: No such file or
  directory`.

After the restore, confirm `git diff --stat` matches its pre-mutation state.

## Test plan

Two tests: a missing `HOME` still opens a shell in `/`, and an existing one
still starts in `HOME`. The race fix is evidenced by the full suite: three
consecutive green runs with the retry deleted, which is the thing that
failed 2 runs in 3 at drafting without Task 2.

## End-to-end verification

```sh
cd /home/matt/src/daemoneye
{
  for i in 1 2 3; do
    out=$(cargo test --lib 2>&1)
    printf 'lib%d=%s %s\n' "$i" "$(printf '%s\n' "$out" | grep -E '^test result' | tail -1 | grep -oE 'ok|FAILED')" "$(printf '%s\n' "$out" | grep -E '^test .* FAILED$' | sed -E 's/^test (.*) \.\.\. FAILED$/\1/' | tr '\n' ' ')"
  done
  printf 'lib_counts=%s\n' "$(cargo test --lib 2>&1 | grep -E '^test result' | tail -1 | sed -E 's/.*ok\. ([0-9]+) passed; ([0-9]+) failed; ([0-9]+) ignored.*/\1p-\2f-\3i/')"
  printf 'new_tests=%s\n' "$(cargo test --lib -- a_missing_home_still_opens_a_shell a_shell_still_starts_in_home 2>&1 | grep -E '^test result' | tail -1 | sed -E 's/.*ok\. ([0-9]+) passed; ([0-9]+) failed.*/\1p-\2f/')"
  printf 'pty_module=%s\n' "$(cargo test --lib shell::pty 2>&1 | grep -E '^test result' | tail -1 | grep -oE '[0-9]+ passed')"
  printf 'retry_strings=%s\n' "$(grep -c 'could not be spawned' src/daemon/executor/shell_exec.rs)"
  printf 'guarded=%s\n' "$(grep -A1 'PTY_TEST_LOCK.lock().await' src/daemon/executor/shell_exec.rs | grep -c 'test_home_guard')"
  printf 'tiocsctty=%s\n' "$(grep -c 'TIOCSCTTY' src/daemon/executor/shell_exec.rs)"
} > /tmp/e2e-m23-01a.txt
cat /tmp/e2e-m23-01a.txt
```

Expected: `lib1..3=ok` with **nothing after `ok`**, `lib_counts=2090p-0f-14i`,
`new_tests=2p-0f`, `pty_module=21 passed`, `retry_strings=1`, `guarded=6`,
`tiocsctty=0`.

**Re-run budget: three** (WORKFLOW.md § End-to-end verification). The known
pre-existing flake is `shell::backend::tests::input_after_a_prompt_logs_a_redaction_marker`.
If it fails, name it and re-run the whole block. After three re-runs, stop,
paste the last capture as it is, and name every failing test. **A failing
connect test is not the flake: it means Task 2 missed one; report it.**

**Run this block with nothing else running `cargo test`.** Paste the file into
this phase doc, then run:

```sh
D=docs/dev/milestones/M23-client-decoupling/phase-01a-pty-spawn-transient.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-m23-01a.txt
diff /tmp/pasted-m23-01a.txt /tmp/e2e-m23-01a.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

The literal `PASTE MATCH` line goes into the entry. Mutation A's result goes
in the same entry, after the first block.

## Authorizations

You may edit these files, and nothing else:

- `src/shell/pty.rs`: Task 1 and Task 4's tests.
- `src/daemon/executor/shell_exec.rs`: Tasks 2 and 3, inside `connect_tests`
  only.
- This phase doc's Update Log.

## Out of scope

- **Other test modules that spawn PTYs without the HOME guard.** None failed
  in the measured runs. If one does, it is a finding to report, not something
  to fix here.
- **Restoring `HOME` in `test_home_guard`'s callers.** Tests leave `HOME`
  pointing at a deleted temp dir. Task 1 makes that harmless for PTY spawns;
  changing the guard is a separate decision.
- **Any change to portable-pty.**

## Update Log

<!-- entries appended below this line -->
