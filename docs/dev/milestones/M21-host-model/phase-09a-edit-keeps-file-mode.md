# Phase 09a: edit-keeps-file-mode — an `edit_file` edit keeps the file's mode and owner, locally and on a host

**Milestone:** M21 — Host model & SSH
**Status:** in-progress
**Depends on:** phase-07b (`edit_file { host }` through `run_in_host_shell`)
**Estimated diff:** ~60 lines of production code across 2 files, plus 5 tests and 1 ignored live test
**Tags:** language=rust, kind=bugfix, size=s

## Goal

`edit_file`'s `edit` operation writes the new content to a `.de_tmp` file and
renames it over the original. The temp file is created with the writer's
**umask**, not the original's mode, so every edit silently re-permissions the
file: a private `0600` file comes back `0644` on the daemon host and `0664` on
`pinky`, where it is **group-writable**. After this phase, an edit keeps the
file's permission bits on every path — local, remote Python, remote Perl —
and keeps its owner and group wherever the writer is allowed to set them.

This is also the prerequisite for row 09b (a remote **sudo** edit). Under sudo
the same rename would turn a file owned by `www-data` into a root-owned one,
and a `0440` file into a world-readable one.

## Architecture references

- `docs/security.md` — files the daemon writes on the operator's behalf must
  not widen their access.
- `docs/dev/milestones/M21-host-model/README.md` § Notes — "Edits widen a
  file's mode" (measured at 09a drafting).

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read this entire phase doc before touching any code.
3. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

Measured on **2026-09-29** at commit `956566b9`.

Baseline: `cargo test --lib` → **2073 passed / 0 failed / 12 ignored**.

### 1. The defect, measured on every path

| Path | Before | After one edit |
|---|---|---|
| Local (`HostId::Local`), daemon umask `0022` | `0600` | **`0644`** |
| Remote Python branch, run by hand locally | `0600` | **`0644`** |
| Remote Perl branch (`python3` hidden from `PATH`) | `0600` | **`0644`** |
| `pinky`, the real built command, the PTY shell's umask `0002` | `600:matt` | **`664:matt`** |

The daemon's PTY shell on `pinky` reports `umask` → `0002`, so on that host an
edited file becomes **group-writable**.

### 2. The local write (`src/daemon/executor/file_ops/ops.rs:220-235`)

```rust
    let tmp_path = std_path.with_extension("de_tmp");
    if let Err(e) = std::fs::write(&tmp_path, &updated) {
        crate::daemon::stats::finish_command(cmd_id, 1);
        return Ok(ToolCallOutcome::Result(format!(
            "Error writing temp file: {}",
            e
        )));
    }
    if let Err(e) = std::fs::rename(&tmp_path, &std_path) {
        let _ = std::fs::remove_file(&tmp_path);
        crate::daemon::stats::finish_command(cmd_id, 1);
        return Ok(ToolCallOutcome::Result(format!(
            "Error committing edit: {}",
            e
        )));
    }
```

`std_path` is already `canonicalize()`d (`:166`) and the original is read at
`:175`. The same `with_extension("de_tmp")` shape at **`:450`** is the
`create` operation writing a **new** file. It has no original mode to keep,
so it is **out of scope**; leave it unchanged.

### 3. The remote builder (`src/daemon/executor/file_ops/write.rs`, `build_remote_edit_cmd`)

The Python payload (`:192-204`), inside a Rust `format!` string:

```rust
    let py = format!(
        "import os,sys\n\
         p=bytes.fromhex('{path_hex}').decode()\n\
         o=bytes.fromhex('{old_hex}').decode()\n\
         n=bytes.fromhex('{new_hex}').decode()\n\
         c=open(p).read()\n\
         cnt=c.count(o)\n\
         if cnt==0: print('DE_ERROR: old_string not found in '+p); sys.exit(1)\n\
         if cnt>1: print('DE_ERROR: old_string appears '+str(cnt)+' times in '+p); sys.exit(1)\n\
         t=p+'.de_tmp'\n\
         open(t,'w').write(c.replace(o,n,1))\n\
         os.rename(t,p)\n\
         print('DE_OK: Edited '+p)\n"
    );
```

The Perl payload follows it (`:208-223`): it reads `$c`, checks the match
count, writes `$t` with `open(my $g,'>',$t)` / `print $g $c;close $g;`, then
`rename($t,$p)`. Both payloads are hex-encoded into the wire command, so they
only ever appear in this source file and in the decoded payloads.

### 4. The fix, measured

Both forms below were run at drafting, from a shell with umask `0002`: a
`0750` file stayed `750` (Python) and a `0600` file stayed `600` (Perl), and
the content was edited. A `chown` the writer is not allowed to make raised
`PermissionError` in Python, which the `except` absorbs. In Perl, `chown`
just returns `0`. So restoring the owner is **best-effort by design**: it
succeeds when the writer is the owner or root, and is a no-op otherwise. The
same Python sequence (stat → write → chmod → chown → rename) kept `600:matt`
on `pinky` through the daemon's own PTY shell.

Python, the lines between the `cnt>1` check and the `DE_OK` print:

```python
st=os.stat(p)
t=p+'.de_tmp'
open(t,'w').write(c.replace(o,n,1))
os.chmod(t,st.st_mode&0o7777)
try: os.chown(t,st.st_uid,st.st_gid)
except OSError: pass
os.rename(t,p)
```

Perl, `my @st=stat($p);` after the read, and before `rename`:

```perl
chmod($st[2]&07777,$t);
chown($st[4],$st[5],$t);
```

## Spec

### Task 1 — the local edit keeps mode and owner (`src/daemon/executor/file_ops/ops.rs`)

In `run_edit_with_spawn`'s local path, take the original's metadata
(`std::fs::metadata(&std_path)`) before the temp write, and after
`std::fs::write(&tmp_path, &updated)` succeeds:

1. `std::fs::set_permissions(&tmp_path, <the original's permissions>)`. On
   `Err(e)`: remove the temp file, `finish_command(cmd_id, 1)`, and return
   `Ok(ToolCallOutcome::Result(format!("Error preserving the file's permissions: {e}")))`.
   **Never** rename a temp file whose mode could not be set.
2. Best-effort owner: `std::os::unix::fs::chown(&tmp_path, Some(uid), Some(gid))`
   from `std::os::unix::fs::MetadataExt`'s `uid()` / `gid()`. Ignore its
   result with `let _ =` and a one-line comment saying why (a non-root daemon
   may not change ownership; mode is the guarantee, owner is kept when
   permitted).

If the metadata call itself fails, return an error the same way as the
existing read failure (`"Error reading {path}: {e}"`). Do **not** touch the
`create` path at `:450`.

### Task 2 — the remote payloads keep mode and owner (`src/daemon/executor/file_ops/write.rs`)

Change `build_remote_edit_cmd`'s two payloads to the measured forms in
§ Current state 4:

- **Python:** insert `st=os.stat(p)` before `t=p+'.de_tmp'`, then
  `os.chmod(t,st.st_mode&0o7777)` and the two-line `try:` / `except OSError: pass`
  `os.chown` after the write and before `os.rename`. Write each Python line
  exactly as quoted, with no spaces inside the calls. Each one is a
  `...\n\` line in the `format!` string, like its neighbours.
- **Perl:** `my @st=stat($p);` after `close $f;`, then
  `chmod($st[2]&07777,$t);` and `chown($st[4],$st[5],$t);` after
  `print $g $c;close $g;` and before `rename(`.

Everything else in the builder stays byte-identical: the interpreter choice,
the hex encoding, the `DE_OK:` / `DE_ERROR:` sentinels and the trailing
`echo '__DE_DONE__'` (`the_write_sentinels_are_unchanged` pins them).

### Task 3 — tests (`src/daemon/executor/file_ops/ops.rs` tests module)

There are six tests, with the names pinned. Every mode test uses **two** files,
one `0600` and one `0750`. The `0750` file makes a missing chmod visible under
**any** umask: a temp file is never created executable, so without the chmod it
comes back without its `x` bits.

1. `a_local_edit_keeps_the_files_mode`: `remote_write_test_home()` for the
   HOME guard. Two files in its temp dir, each containing `a\nold-line\nb\n`,
   set to `0o600` and `0o750`. `run_edit_with_spawn` with `HostId::Local` and
   `auto_approve_channel()`, `old-line` → `new-line`. Assert the outcome is a
   `Result` containing `Edited`, each file's content contains `new-line`, and
   `metadata().permissions().mode() & 0o7777` equals its original mode.
2. `a_remote_edit_keeps_the_files_mode`: the same two files, run through
   `HostId::Named("web01".to_string())` with
   `Box::new(|id| Box::pin(spawn_in_process_host(id)))`, as
   `edit_on_a_named_host_runs_through_the_shell` does. Assert
   `Edited … on host web01.`, the content, and both modes. **After the
   loop, close the shell the test opened**. `spawn_in_process_host` leaks its
   bash on purpose, and a leaked shell here made unrelated PTY tests flake
   (measured at escalation: 5 red runs out of 17 without the close, 7 green
   out of 8 with it; the one red was the known flake):

   ```rust
        let owner = Owner::Chat(owner_key());
        if let Some(id) = crate::shell::registry_lookup(&owner, |id| id.to_string()) {
            let _ = crate::daemon::executor::knowledge::close_shell_with(
                crate::shell::registry(),
                &id,
                &owner,
                std::time::Duration::from_millis(3000),
            )
            .await;
        }
   ```
3. `the_python_branch_keeps_the_files_mode`: pure, with no HOME guard.
   Build the command with `build_remote_edit_cmd` for each file and run it
   with `std::process::Command::new(<bash's absolute path>).arg("-c").arg(&cmd)`,
   using `.env("PATH", bin)`, where `bin` is a temp dir holding **only** a
   symlink named `python3` to the real interpreter. Resolve both absolute
   paths first, before narrowing `PATH`, with
   `Command::new("sh").args(["-c", "command -v python3"])` (and the same for
   `bash`). **Gotcha:** Rust looks the program up in the child's `PATH` once
   `.env("PATH", …)` is set, so a bare `Command::new("bash")` fails with
   "No such file or directory". Assert stdout contains `DE_OK:` and both
   modes are kept.
4. `the_perl_branch_keeps_the_files_mode`: the same, with `bin` holding
   only a `perl` symlink, so `command -v python3` fails and the Perl branch
   runs.
5. `the_edit_payloads_restore_ownership`: decode both hex payloads out of
   `build_remote_edit_cmd("/tmp/x", "a", "b")`, the way
   `the_write_sentinels_are_unchanged` does with its `decode_hex_from`
   closure (copy it), and assert:
   - the Python payload contains `os.chmod(t,st.st_mode&0o7777)` and
     `os.chown(t,st.st_uid,st.st_gid)`;
   - the Perl payload contains `chmod($st[2]&07777,$t)` and
     `chown($st[4],$st[5],$t)`.

   Owner restoration cannot be exercised without root, so this test pins the
   payload that does it. The mode tests above pin the observable half.

Live, ignored: `#[ignore = "requires the configured host \`pinky\` reachable over ssh"]`:

6. `a_remote_edit_keeps_mode_on_pinky`: copy `edit_file_on_pinky_end_to_end`'s
   setup (HOME guard, `[hosts.pinky]` config, `crate::config::ssh_dir()`).
   - Create `/tmp/de-09a-mode-probe` on pinky with `run_in_host_shell`
     (`crate::shell::registry()`, `Owner::Chat(owner_key())`, spawn
     `crate::daemon::executor::knowledge::spawn_pinky_ssh`), running
     `printf 'a\nold-line\nb\n' > /tmp/de-09a-mode-probe && chmod 600 /tmp/de-09a-mode-probe && stat -c before=%a /tmp/de-09a-mode-probe`.
   - Run `run_edit_with_spawn` on it with `HostId::Named("pinky")` and
     `auto_approve_channel()`.
   - Read back with `stat -c after=%a /tmp/de-09a-mode-probe; rm -f /tmp/de-09a-mode-probe`.
   - `println!` both `stat` lines, and assert `before=600` and `after=600`.
   - Clean up the same way `edit_file_on_pinky_end_to_end` does (close the
     shell, `ssh -O exit`).

**Test count:** five new and run, one new and ignored, so **2078 passed /
13 ignored**.

### Task 4 — capture the end-to-end evidence

Run the block in § End-to-end verification verbatim. Paste its output into a
new Update Log entry **as that entry's first fenced block**. **Re-run the
capture as the last thing before committing.**

### Task 5 — the live check

Run test 6 by hand and paste its output into the same entry, after the
mutations:

```sh
cargo test --lib a_remote_edit_keeps_mode_on_pinky -- --ignored --nocapture 2>&1 | grep -E 'before=|after=|^test |test result'
```

**If `pinky` is unreachable, report the criterion unmet.** Do not describe
what it would have printed.

## Acceptance criteria

Parenthesised values are what each line produces **against the current
tree**. Every non-preservation line fails today; each was checked at drafting.

- [ ] `cargo test --lib` → **2078 passed / 0 failed / 13 ignored**, on three
      consecutive runs (current: 2073/0/12). **Both counts are exact.**
- [ ] `cargo fmt --all` clean; `cargo build` clean;
      `cargo clippy --all-targets --all-features -- -D warnings` exits 0.
- [ ] `cargo test` → every other target unmoved: bug_tracker 6, doc_truth 14,
      integration 32, isolation 10 (*preservation*).
- [ ] The five new non-ignored tests pass:

      ```sh
      cargo test --lib -- \
        a_local_edit_keeps_the_files_mode \
        a_remote_edit_keeps_the_files_mode \
        the_python_branch_keeps_the_files_mode \
        the_perl_branch_keeps_the_files_mode \
        the_edit_payloads_restore_ownership
      ```

      → **5 passed** (current: `0 passed; 0 failed; … filtered out`).
- [ ] Neighbours still pass (*preservation*):
      ``cargo test --lib the_write_sentinels_are_unchanged`` → **1 passed**;
      ``cargo test --lib edit_on_a_named_host_runs_through_the_shell`` →
      **1 passed**; ``cargo test --lib file_ops`` → **0 failed**.
- [ ] The `create` path is untouched (*preservation*):
      ``grep -c 'Error committing new file' src/daemon/executor/file_ops/ops.rs``
      → **1**.
- [ ] Mutations A–C were applied, run, and their observed results recorded in
      the Update Log.
- [ ] The § End-to-end verification block was run verbatim, pasted as the
      **first fenced block** of an entry titled
      `### Update — <date> (end-to-end verification)`, and the diff snippet
      printed the literal `PASTE MATCH` into that entry.
- [ ] The Task 5 live check was run and its output pasted into the same
      entry, or reported unmet with the reason.

## Mutations

Apply each with the `patch` tool (the executor contract forbids `sed -i`,
`perl -i`, `> file` and `rm -rf`), run it, restore it, and report what you saw.
**Run budget: one run each.** Record what you actually observe.

- **Mutation A:** delete the Python `os.chmod(t,st.st_mode&0o7777)` line.
  Run tests 2 and 3. Expected **red**: the `0750` file comes back without its
  `x` bits.
- **Mutation B:** delete the Perl `chmod($st[2]&07777,$t);`. Run test 4.
  Expected **red**.
- **Mutation C:** delete the local `set_permissions` call. Run test 1.
  Expected **red**.

After each restore, confirm `git diff --stat` matches its pre-mutation state.

## Test plan

There are six tests (Task 3):
- the local path;
- the remote path through an in-process host;
- each interpreter branch alone, with `PATH` narrowed to that interpreter;
- the ownership payload, pinned by decoding it;
- a live run on `pinky`, where the shell's umask `0002` is what makes the
  defect group-writable.

## End-to-end verification

```sh
cd /home/matt/src/daemoneye
{
  for i in 1 2 3; do
    out=$(cargo test --lib 2>&1)
    printf 'lib%d=%s %s\n' "$i" "$(printf '%s\n' "$out" | grep -E '^test result' | tail -1 | grep -oE 'ok|FAILED')" "$(printf '%s\n' "$out" | grep -E '^test .* FAILED$' | sed -E 's/^test (.*) \.\.\. FAILED$/\1/' | tr '\n' ' ')"
  done
  printf 'lib_counts=%s\n' "$(cargo test --lib 2>&1 | grep -E '^test result' | tail -1 | sed -E 's/.*ok\. ([0-9]+) passed; ([0-9]+) failed; ([0-9]+) ignored.*/\1p-\2f-\3i/')"
  printf 'new_tests=%s\n' "$(cargo test --lib -- a_local_edit_keeps_the_files_mode a_remote_edit_keeps_the_files_mode the_python_branch_keeps_the_files_mode the_perl_branch_keeps_the_files_mode the_edit_payloads_restore_ownership 2>&1 | grep -E '^test result' | tail -1 | sed -E 's/.*ok\. ([0-9]+) passed; ([0-9]+) failed.*/\1p-\2f/')"
  printf 'neighbours=%s/%s/%s\n' "$(cargo test --lib the_write_sentinels_are_unchanged 2>&1 | grep -E '^test result' | tail -1 | grep -oE '[0-9]+ passed')" "$(cargo test --lib edit_on_a_named_host_runs_through_the_shell 2>&1 | grep -E '^test result' | tail -1 | grep -oE '[0-9]+ passed')" "$(cargo test --lib file_ops 2>&1 | grep -E '^test result' | tail -1 | grep -oE '[0-9]+ failed')"
  printf 'create_untouched=%s\n' "$(grep -c 'Error committing new file' src/daemon/executor/file_ops/ops.rs)"
  printf 'ignore_total=%s\n' "$(grep -rc '#\[ignore' --include=*.rs src/ | grep -v ':0' | awk -F: '{s+=$2} END {print s}')"
} > /tmp/e2e-09a.txt
cat /tmp/e2e-09a.txt
```

Expected: `lib1..3=ok` with **nothing after `ok`**, `lib_counts=2078p-0f-13i`,
`new_tests=5p-0f`, `neighbours=1 passed/1 passed/0 failed`,
`create_untouched=1`, `ignore_total=13`.

**Known pre-existing flake.** `shell::backend::tests::input_after_a_prompt_logs_a_redaction_marker`
fails intermittently under full-suite load. If it fails, report its name. Do
not edit it; re-run the **whole block**. **Re-run budget: three.** If the
block has not come back with `lib1..3=ok` after three re-runs, stop
re-running. Paste the last capture as it is, and name every failing test in
the Update Log. Do **not** loop on `cargo test`: the prior run spent hundreds
of turns doing exactly that. A failing test **other** than this one is a real
finding; report it by name, do not re-run it away.

**Run this block with nothing else running `cargo test`.**

Paste the file's contents into an Update Log entry titled
`### Update — <date> (end-to-end verification)` **as that entry's first fenced
block**, then run:

```sh
D=docs/dev/milestones/M21-host-model/phase-09a-edit-keeps-file-mode.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-09a.txt
diff /tmp/pasted-09a.txt /tmp/e2e-09a.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

The literal `PASTE MATCH` line goes **into** the entry. The mutation results
and the live check go in the same entry, **after** that first block.

## Authorizations

You may edit these files, and nothing else:

- `src/daemon/executor/file_ops/ops.rs`: Task 1, and Task 3's tests.
- `src/daemon/executor/file_ops/write.rs`: Task 2 (the two payloads only).
- This phase doc's Update Log.

## Out of scope

- **`create`, `delete` and `copy`**: `create` writes a new file, and `copy`
  gets its mode from `cp` / `std::fs::copy`. Neither is an edit.
- **Symlinks**: the local path edits the `canonicalize()`d target. The remote
  `os.rename` replaces a symlink at `p` with a regular file, as it does today.
  This is recorded here and not changed.
- **Extended attributes, ACLs, SELinux labels**: mode bits and owner only.
- **Sudo edits**: row 09b.

## Update Log

### Update — 2026-09-29 13:35 (started)

Executor: Claude (Opus 4.6). Status flipped `todo → in-progress` (phase doc and
milestone README row). Beginning Task 1 (the local edit keeps mode and owner).

<!-- entries appended below this line -->

### Notes for executor — 2026-09-29

**The uncommitted diff in the working tree is your own prior run's work. Keep
it; do not redo it.** The prior run hard-failed on a network outage
(`BackendError`, after a firewall reboot). The architect ran the gates on the
partial tree. What is already done and correct:
- Task 1 (the local `set_permissions` + best-effort `chown` in `ops.rs`).
- Task 2 (both payloads in `write.rs`).
- All six Task 3 tests. The five non-ignored ones pass, and
  `cargo test --lib` counts **2078 passed / 13 ignored** exactly.

There are exactly four things left, in this order:

1. **`cargo fmt --all`.** The tree is not formatted (`cargo fmt --all -- --check`
   fails).
2. **Test 2's shell close.** Add the block now quoted in Task 3, test 2,
   after the `for` loop in `a_remote_edit_keeps_the_files_mode`. The architect
   measured that without it, the leaked in-process shell makes unrelated PTY
   tests fail at random (`pty_stop_with_no_foreground_job_remembers_nothing`,
   `an_answered_prompt_restarts_the_connect_timeout`,
   `for_each_streams_across_segments_in_order`). That is why the prior run
   could never get three green runs in a row. The test count does not change.
3. **Mutations A–C**, one run each, observed results recorded.
4. **The end-to-end capture and the Task 5 live check.** Follow the new
   re-run budget in § End-to-end verification: at most three re-runs of the
   block, then record what you saw.

Also correct the `(started)` entry's `Executor:` line: this phase runs on
`local-inference-lab/GLM-5.3-Flash-NVFP4-Spark`, not "Claude (Opus 4.6)".

### Update — 2026-09-29 (escalation)

**Chosen lever:** refined re-dispatch, via `execute_phase` on the in-progress
doc, continuing from the dirty tree.
**Rationale:** The run ended on an infrastructure fault (the firewall
rebooted, per the PE) after a verify loop on a gate it could not pass. The
implementation is complete and correct. The gate kept failing because test 2
leaked a shell and made other tests flake, which the architect measured and
fixed in the spec. Resume via `continue_phase` was passed over because it
blocks and cannot be stopped.
