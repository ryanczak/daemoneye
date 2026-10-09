# Phase 10b: sbx-stage-resolved — stage a script the ghost path has already resolved to its absolute path

**Milestone:** M26 — Agent isolation on Docker Sandboxes (sbx)
**Status:** in-progress
**Depends on:** phase-10a
**Estimated diff:** ~60 lines
**Tags:** language=rust, kind=bugfix, size=xs

## Goal

Found at the row-10 live close on pinky, 2026-10-08 (ghost run 4). A
sandboxed ghost ran its approved script `de-live-tick.sh` and got:

```
bash: /home/matt/.daemoneye/scripts/de-live-tick.sh: No such file or directory
[command exited with status 127]
```

The script was never staged into the sandbox and the command was never
rewritten to `/de/scripts/de-live-tick.sh`.

## Root cause

In `run_background` (`src/daemon/executor/foreground.rs`) a ghost's command
goes through `policy.resolve_command(cmd)` **before** the sandboxed-ghost
branch, turning `de-live-tick.sh` into
`/home/matt/.daemoneye/scripts/de-live-tick.sh`. The branch then calls
`sbx::script_invocation(cmd, …)`, which uses
`crate::scripts::parse_script_invocation`, and that returns `None` for any
first word starting with `/` (`src/scripts.rs`, the `first.starts_with('/')`
check). So staging is skipped and the host path runs inside the VM.

The 06d unit tests only used bare names. `start_background_sandboxed`
(`src/daemon/executor/bg_shell.rs`, the chat `wait: false` path) calls the
same `script_invocation`; check whether chat commands reach it resolved too,
and cover whichever paths do.

## Spec

A sandboxed command whose first word is the absolute path of a file inside
`crate::config::scripts_dir()` must be staged and rewritten exactly as the
bare name is. An absolute path **outside** the scripts directory must still
not be staged. Keep `parse_script_invocation`'s other callers (the remote
ghost path) behaving as they do now. Choose where the fix lives.

Tests, in `src/daemon/executor/sbx.rs` or wherever the logic lands:
- `script_invocation_accepts_an_absolute_path_under_scripts_dir`: the first
  word `<scripts_dir()>/restart.sh` with `exists = |n| n == "restart.sh"`
  gives `Some(("restart.sh", " --now"))` for a ` --now` tail.
- `script_invocation_rejects_an_absolute_path_outside_scripts_dir`:
  `/usr/bin/restart.sh --now` gives `None` even when `exists` returns true.

## Acceptance criteria

- [ ] Both named tests report `1 passed` alone. Today each reports `0 passed`.
- [ ] `cargo test --lib executor::sbx` reports `0 failed`; no existing test edited.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` exits 0;
      `cargo fmt --all --check` exits 0.
- [ ] `cargo test` reports `0 failed` and exactly `passed=2300` (today 2298,
      plus the two new tests).
- [ ] The Update Log entry contains the § End-to-end verification output,
      every line as printed, and a line `PASTE MATCH: yes`.

## End-to-end verification

```sh
for t in script_invocation_accepts_an_absolute_path_under_scripts_dir script_invocation_rejects_an_absolute_path_outside_scripts_dir; do cargo test --lib "$t" 2>&1 | grep -E '^test result' | sed "s/^/$t: /"; done
cargo test --lib executor::sbx 2>&1 | grep -E '^test result'
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1
cargo fmt --all --check; echo "fmt_exit=$?"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
git diff --stat
```

Each line runs **once**. Expect **only** `test result:` summary lines from
the `cargo test` lines; rexyMCP's output filter removes the individual
`test … ok` lines.

## Authorizations

- [x] May edit `src/daemon/executor/sbx.rs`, `src/scripts.rs`,
      `src/daemon/executor/foreground.rs` and
      `src/daemon/executor/bg_shell.rs` as the chosen fix needs.

## Out of scope

- The live re-run (the architect repeats the ghost check after this lands).

## Update Log

<!-- entries appended below this line -->

### Update — 2026-10-08 15:01 (started, executor: claude)

Executor run started. Flipped Status todo → in-progress; README row mirrored.
Plan: re-read the resolve/staging call path (`policy.resolve_command` in
`run_background`, then `sbx::script_invocation` → `parse_script_invocation`),
choose where the absolute-path fix lives, add the two named tests, keep the
remote ghost path and all existing tests untouched, then run the § End-to-end
verification block verbatim and paste it as this phase's end-to-end entry.

### Notes for executor — 2026-10-08 (refined re-dispatch after a NoProgressStall)

The previous run made 120 read-only calls exploring `policy.rs`,
`executor/mod.rs` and `session.rs`, and edited nothing. The spec left the
fix's location open, and that was the gap. **It is now decided**:

- **Fix only `script_invocation` in `src/daemon/executor/sbx.rs`** (line
  ~362). Before it calls `parse_script_invocation`, if the command's first
  word is an absolute path whose **parent directory equals
  `crate::config::scripts_dir()`**, replace that first word with its file
  name. Any other absolute path is left as it is, and
  `parse_script_invocation` rejects it as today.
- Keep the signature unchanged. Both callers (`foreground.rs` ~1246, the
  ghost path; `bg_shell.rs` ~116, chat `wait: false`) then get the fix with
  no edit.
- Do **not** touch `src/scripts.rs`, `policy.rs` or `executor/mod.rs`.
  There is nothing to investigate there.
- Add the two named tests next to `script_invocation_skips_sudo_and_unknown_scripts`.
  Build the absolute path from `crate::config::scripts_dir()`.

Finish condition: `passed=2300`, exactly two more than today.

### Update — 2026-10-08 (escalation)

**Chosen lever:** refined re-dispatch
**Rationale:** no code was written, so there is nothing to resume. The stall
came from the spec leaving the fix's location open, and pinning that one
decision removes it.

### Update — 2026-10-08 (end-to-end verification)

Fix landed in `src/daemon/executor/sbx.rs` only, per the re-dispatch note:
`script_invocation` now runs its first word through
`canonicalize_scripts_dir_path`, which rewrites
`<crate::config::scripts_dir()>/<name>` to the bare `name` (keeping the
verbatim argument tail) before `parse_script_invocation` sees it; any other
absolute path passes through untouched and stays `None` there. Both callers
(`foreground.rs` ghost path, `bg_shell.rs` chat `wait: false`) pick the fix up
unmodified. `src/scripts.rs`, `policy.rs`, `executor/mod.rs` untouched; no
existing test edited.

```
$ for t in script_invocation_accepts_an_absolute_path_under_scripts_dir script_invocation_rejects_an_absolute_path_outside_scripts_dir; do cargo test --lib "$t" 2>&1 | grep -E '^test result' | sed "s/^/$t: /"; done
script_invocation_accepts_an_absolute_path_under_scripts_dir: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 2245 filtered out; finished in 0.00s
script_invocation_rejects_an_absolute_path_outside_scripts_dir: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 2245 filtered out; finished in 0.00s
$ cargo test --lib executor::sbx 2>&1 | grep -E '^test result'
test result: ok. 66 passed; 0 failed; 0 ignored; 0 measured; 2180 filtered out; finished in 0.00s
$ cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 10.10s
$ cargo fmt --all --check; echo "fmt_exit=$?"
fmt_exit=0
$ cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
passed=2300 failed=0
$ git diff --stat
 src/daemon/executor/sbx.rs | 46 +++++++++++++++++++++++++++++++++++++++++++++-
 1 file changed, 45 insertions(+), 1 deletion(-)
```

PASTE MATCH: yes
