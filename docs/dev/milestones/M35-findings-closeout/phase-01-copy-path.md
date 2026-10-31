# Phase 01 — copy-path

**Status:** todo
**Milestone:** M35 — Findings closeout
**Drafted:** 2026-10-31
**Depends on:** M33 (v2.6.0)
**Tags:** language=rust, kind=fix, size=s

## Goal

Three findings in `edit_file`'s copy operation
(`src/daemon/executor/file_ops/ops_copy.rs`, `run_copy_with_spawn`) are
closed (M31 FINDINGS #36, #37, #40):

- **#40, a defect:** the local branch writes its temp file at
  `dest.with_extension("de_tmp")`. That replaces the extension, so copies to
  `/d/b.txt` and `/d/b.md` share `/d/b.de_tmp`, and two copies running at
  once overwrite each other's temp file. After this phase the temp path is
  unique per invocation and per destination, in the destination's directory
  (the rename must stay on one filesystem).
- **#37, a defect:** an existing destination is reported with two different
  messages. The local branch says `Error: destination already exists: <dest>.
  Remove it first or choose a different path.`; the remote branch's
  `DE_ERROR` line comes back in a different form. After this phase both
  branches return the local message, exactly, from one source.
- **#36, a move:** the remote branch builds its shell command before the
  approval prompt. After this phase it is built after the approval, just
  before it runs. Nothing observable changes.

The suite goes from **2618 to exactly 2620**.

## Constraints

- **Timing stays as it is.** The local branch checks before the prompt; the
  remote branch checks inside the shell after approval. Do not add a
  round-trip to the host before the approval prompt: that would connect, and
  maybe ask for credentials, before the user has approved anything.
- **One source for the message:** a function or constant used by both
  branches, not two copies of the same string.
- **The temp name** keeps a `.de_tmp` suffix and lives next to the
  destination. `uuid` is already a dependency; the process id plus a counter
  would also do.
- `ops_copy.rs` is 240 lines; the `tests` module in `file_ops/ops.rs` (where
  the copy tests live) has about 280 lines of room under its 2000-line test
  cap.
- **Use `patch` with whole blocks,** never `patch_lines`.

## Spec

1. **Extract the temp path** — move today's `with_extension("de_tmp")`
   computation unchanged into a private `copy_tmp_path(dest: &Path) ->
   PathBuf` in `ops_copy.rs`, and call it.
2. **Write the #40 test** — `copy_tmp_path_is_unique_per_call_and_destination`
   in the `tests` module of `file_ops/ops.rs`: two calls for `/d/b.txt`
   differ, `/d/b.txt` and `/d/b.md` differ, each result has `/d` as its
   parent, ends in `.de_tmp`, and is not the destination. Run it against
   step 1's unchanged body and keep the failing output.
3. **Fix #40** — make `copy_tmp_path` unique; the step 2 test passes.
4. **Write the #37 test** — `copy_on_a_named_host_reports_an_existing_destination_like_local`,
   beside `copy_on_a_named_host_runs_through_the_shell` and built the same way
   (`remote_write_test_home`, `auto_approve_channel`,
   `spawn_in_process_host`): create both source and destination, copy on host
   `web01`, and assert the result equals the local message for that
   destination. Run it and keep the failing output.
5. **Fix #37** — one source for the message, used by the local check and by
   the remote branch's handling of its `DE_ERROR` line; the step 4 test
   passes.
6. **Do #36** — build the remote command after the approval, not before.
7. **Mark the ledger** — in `docs/dev/milestones/M31-code-hygiene/FINDINGS.md`,
   rows #36, #37 and #40, replace the triage prefix `act: M35 phase-01` with
   `fixed: M35 phase-01, <test name>` (#36: `fixed: M35 phase-01, move`).
   Keep the `(was: …)` text. Touch no other row.
8. **Red runs** — one `### Update — <date> (red runs)` entry pasting, for
   steps 2 and 4, the `test result:` line and the assertion message.
9. **Findings** — one `### Findings — <date>` entry from `git diff --stat`
   alone, two or three lines, or `none`.
10. **End-to-end** — run the block once before committing; paste its output
    fenced in an `### Update — <date> (end-to-end verification)` entry.
11. **Commit** as `fix: unique copy temp path and one dest-exists message (M31 #36 #37 #40)`.

## Acceptance criteria

- [ ] The E2E block's first line prints `with_extension=0 new_tests=2 fixed=3`
      (today `with_extension=1 new_tests=0 fixed=0`).
- [ ] The red-runs entry shows both new tests failing before their fixes.
- [ ] hygiene, doc_truth and bug_tracker pass; `build_warnings=0`; clippy
      clean; `fmt_exit=0`.
- [ ] `cargo test` reports exactly `passed=2620 failed=0`.

## End-to-end verification

```sh
bash <<'E2E'
echo "with_extension=$(grep -c 'with_extension("de_tmp")' src/daemon/executor/file_ops/ops_copy.rs) new_tests=$(grep -cE 'fn (copy_tmp_path_is_unique_per_call_and_destination|copy_on_a_named_host_reports_an_existing_destination_like_local)\(' src/daemon/executor/file_ops/ops.rs) fixed=$(grep -cE '^\| (36|37|40) \|.*fixed: M35 phase-01' docs/dev/milestones/M31-code-hygiene/FINDINGS.md)"
touch src/main.rs && cargo build 2>&1 | grep -c '^warning' | sed 's/^/build_warnings=/'
for t in hygiene doc_truth bug_tracker; do cargo test --test $t 2>&1 | grep -E '^test result'; done
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1
cargo fmt --all --check; echo "fmt_exit=$?"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
E2E
```

## Authorizations

- Edit `src/daemon/executor/file_ops/ops_copy.rs`, the `tests` module of
  `src/daemon/executor/file_ops/ops.rs`, and the three ledger rows.

## Out of scope

- The temp names of `edit`, `create` and the remote writers in `write.rs`
  and `scripts.rs`. Only the copy's local temp path is a finding.
- Moving the remote dest-exists check before the prompt (see Constraints).

## Update Log

<!-- entries appended below this line -->
