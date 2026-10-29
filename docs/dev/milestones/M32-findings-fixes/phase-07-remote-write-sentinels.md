# Phase 07 — Remote delete and copy read their result, not a substring

**Status:** in-progress
**Milestone:** M32 — Findings fixes
**Drafted:** 2026-10-29
**Finding:** M31 `FINDINGS.md` #38 (#36, #37 and #40 move to the
function milestone)

## Goal

`edit_file`'s remote `delete` and `copy` (`file_ops/ops_delete.rs`,
`file_ops/ops_copy.rs`) run a shell command that prints a result line,
`DE_OK: …` or `DE_ERROR: …`, and then a `__DE_DONE__` line. Each reads
the result by walking the captured output from the bottom and taking
the first line that *contains* `DE_OK:` or `DE_ERROR:`. `__DE_DONE__`
is never checked. That misreads two cases as a success:
- an error whose path contains `DE_OK:`, for example
  `DE_ERROR: file not found: /tmp/DE_OK:x`;
- output with no result line at all. The echoed command itself contains
  both sentinels as text, so the walk finds `DE_OK:` in it.

After this phase, both use one parser,
`remote_write_outcome(snap) -> Option<Result<String, String>>` in
`file_ops/mod.rs`. It reads only the lines above the last line that is
exactly `__DE_DONE__`, and a result line must *start* with its
sentinel. Anything else is `None`, which the call sites already report
as "result was unclear". The messages the model sees do not change.

**Moved, not fixed here:** #36 (copy's shell string is built before
approval), #37 (local and remote dest-exists checks differ) and #40 (a
non-unique temp path). All three are seams of `run_copy_with_spawn`
(213 lines, allowlisted), and they belong to the phase that splits it.

## Architecture references

- M32 README, decisions 1 and 6.

## Spec

1. **Add the parser as today's logic, and the tests, then run them red.**
   In `src/daemon/executor/file_ops/mod.rs`, directly above
   `fn sq_escape`, add `remote_write_outcome` with **today's** scan as
   its body:
   - walk `snap.lines().rev()`;
   - return `Some(Ok(line.trim().to_string()))` for the first line that
     contains `"DE_OK:"`, and `Some(Err(…))` likewise for `"DE_ERROR:"`;
   - otherwise return `None`.

   Append the test module below to the end of `mod.rs`, exactly as
   written, and run `cargo test --lib remote_write_outcome_tests`.
   Three must fail and `a_success_line_is_ok` must pass. The echoed
   command and the missing-marker cases fail with `left: Some(Ok(…))`
   against `right: None`. The `DE_OK:`-in-path case fails with
   `Some(Ok(…))` against `Some(Err(…))`. Paste that output into the
   Update Log.
2. **The fix.** Replace the parser's body and doc comment with the
   version below. Run the tests again; all four must pass.
3. **Use it at both call sites.** In `ops_delete.rs` and `ops_copy.rs`,
   replace the `for line in snap.lines().rev() { … }` loop with a
   `match super::super::remote_write_outcome(&snap)`, shown below for
   delete:
   - `Some(Ok(_))` runs the old `DE_OK:` branch, unchanged;
   - `Some(Err(line))` runs the old `DE_ERROR:` branch with `line` in
     place of `line.trim()`;
   - `None => {}` falls through to the existing "result was unclear"
     return.

   Copy is converted the same way.
4. **Findings** go in their own `### Findings — <date>` entry, from the
   two call sites and the parser only, within five tool calls, or the
   single line `none`.
5. **Commit** as `fix:`.

**Do not look further.** Do not change `ops.rs`; its edit and create
paths have the same scan, and that is out of scope below. The architect
dry-ran this exact change on `HEAD`: red at step 1 (three of four),
green at step 2, clippy and fmt clean, hygiene passing, and the full
suite at `passed=2604 failed=0`.

**Gotcha:** `daemon::utils::event_log::tests::for_each_skips_unparseable_lines`
failed once in the dry run and passed on three re-runs. It is a flake,
unrelated to this phase. If it fails, re-run once, as for the known
flake below.

The fixed parser (step 2):

```rust
/// The outcome a remote write's shell reported: `Ok` with the `DE_OK:` line,
/// `Err` with the `DE_ERROR:` line, or `None` when it reported neither. Only
/// lines before the last line that is exactly `__DE_DONE__` count, and a
/// result line must start with its sentinel. That keeps the echoed command,
/// which contains both sentinels as text, and any path that happens to
/// contain one, from being read as the result.
fn remote_write_outcome(snap: &str) -> Option<Result<String, String>> {
    let lines: Vec<&str> = snap.lines().map(str::trim).collect();
    let done = lines.iter().rposition(|l| *l == "__DE_DONE__")?;
    lines[..done].iter().rev().find_map(|l| {
        if l.starts_with("DE_OK:") {
            Some(Ok(l.to_string()))
        } else if l.starts_with("DE_ERROR:") {
            Some(Err(l.to_string()))
        } else {
            None
        }
    })
}
```

The tests (step 1):

```rust
#[cfg(test)]
mod remote_write_outcome_tests {
    use super::remote_write_outcome;

    /// The echoed command line, as the PTY shows it before the output.
    const ECHO: &str = "if [ -e '/tmp/a' ]; then rm -- '/tmp/a' && echo 'DE_OK: Deleted /tmp/a'; \
                        else echo 'DE_ERROR: file not found: /tmp/a'; fi; echo '__DE_DONE__'";

    #[test]
    fn a_success_line_is_ok() {
        let snap = format!("{ECHO}\nDE_OK: Deleted /tmp/a\n__DE_DONE__\n");
        assert_eq!(
            remote_write_outcome(&snap),
            Some(Ok("DE_OK: Deleted /tmp/a".to_string()))
        );
    }

    #[test]
    fn an_error_whose_path_contains_the_ok_sentinel_is_an_error() {
        let snap = format!("{ECHO}\nDE_ERROR: file not found: /tmp/DE_OK:x\n__DE_DONE__\n");
        assert_eq!(
            remote_write_outcome(&snap),
            Some(Err("DE_ERROR: file not found: /tmp/DE_OK:x".to_string()))
        );
    }

    #[test]
    fn the_echoed_command_alone_is_no_outcome() {
        assert_eq!(remote_write_outcome(&format!("{ECHO}\n")), None);
    }

    #[test]
    fn a_result_without_the_done_marker_is_no_outcome() {
        let snap = format!("{ECHO}\nDE_OK: Deleted /tmp/a\n");
        assert_eq!(remote_write_outcome(&snap), None);
    }
}
```

The delete call site after step 3:

```diff
@@ -83,8 +83,8 @@ where
                     return Ok(ToolCallOutcome::Result(format!("Error: {}", e)));
                 }
             };
-            for line in snap.lines().rev() {
-                if line.contains("DE_OK:") {
+            match super::super::remote_write_outcome(&snap) {
+                Some(Ok(_)) => {
                     crate::daemon::stats::finish_command(cmd_id, 0);
                     crate::daemon::utils::log_event(
                         "file_delete",
@@ -95,14 +95,14 @@ where
                         path, name
                     )));
                 }
-                if line.contains("DE_ERROR:") {
+                Some(Err(line)) => {
                     crate::daemon::stats::finish_command(cmd_id, 1);
                     return Ok(ToolCallOutcome::Result(format!(
                         "Error deleting {}: {}",
-                        path,
-                        line.trim()
+                        path, line
                     )));
                 }
+                None => {}
             }
             crate::daemon::stats::finish_command(cmd_id, 1);
             return Ok(ToolCallOutcome::Result(format!(
```

## Acceptance criteria

- [ ] `grep -c 'contains("DE_OK:")' src/daemon/executor/file_ops/ops_delete.rs src/daemon/executor/file_ops/ops_copy.rs`
      prints `0` for both files (today: `1` each).
- [ ] `grep -c 'remote_write_outcome(&snap)'` prints `1` in each of the
      two files.
- [ ] The four `remote_write_outcome_tests` pass, and the Update Log
      shows three failing at step 1.
- [ ] `grep -c '^### Findings — '` on this doc prints `1`.
- [ ] `cargo test` reports **exactly** `passed=2604 failed=0` (2600 + 4).
      `tests/hygiene.rs` and `cargo test --test bug_tracker` pass. fmt,
      build and clippy are clean.

## End-to-end verification

```sh
bash <<'E2E'
D=src/daemon/executor/file_ops
echo "substring=$(cat $D/ops_delete.rs $D/ops_copy.rs | grep -c 'contains(\"DE_OK:\")') parser_calls=$(cat $D/ops_delete.rs $D/ops_copy.rs | grep -c 'remote_write_outcome(&snap)')"
cargo test --lib remote_write_outcome_tests 2>&1 | grep -E '^test result'
cargo test --test hygiene 2>&1 | grep -E '^test result'
cargo test --test bug_tracker 2>&1 | grep -E '^test result'
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
E2E
```

Expected: `substring=0 parser_calls=2`, `test result: ok. 4 passed`,
hygiene `ok. 16 passed`, bug tracker `ok. 6 passed`, clippy clean,
`passed=2604 failed=0`. Paste the output into an
`### Update — <date> (end-to-end verification)` entry. **Known flakes:**
`read_shell_screen_lines_tails_the_visible_rows` (M31 #65) and
`for_each_skips_unparseable_lines` (above). If the block shows
`passed=2603 failed=1`, run it once more and paste both outputs.

## Authorizations

- Edit `src/daemon/executor/file_ops/mod.rs`, `ops_delete.rs` and
  `ops_copy.rs`. No other source file.

## Out of scope

- `ops.rs`. Its remote `edit` and `create` read `DE_OK:` the same
  substring way, but they sit inside allowlisted functions
  (`run_edit_with_spawn` 271, `run_create_with_spawn` 164). They move to
  the parser when those functions are split.
- #36, #37, #40 (above).

## Update Log

<!-- entries appended below this line -->

### Update — 2026-10-29 20:33 (progress)

Step 1 done and red as predicted: `a_success_line_is_ok` passed, the other
three failed (`Some(Ok(…))` against `None` for the echoed command and the
missing-marker cases, `Some(Ok(…))` against `Some(Err(…))` for the
`DE_OK:`-in-path case). Step 2 swapped the parser body for the
`__DE_DONE__`-aware version; all four green. Step 3 converted both call
sites to `match super::super::remote_write_outcome(&snap)` with
`None => {}` falling through to the existing "result was unclear"
return; the `DE_OK:`/`DE_ERROR:` message strings are unchanged.

### Findings — 2026-10-29

- `ops_delete.rs` · `remote_delete_with_spawn`: the old failure arm
  formatted `line.trim()`; the parser already trims each line, so the
  call site now formats `line` directly and the trim belongs to the
  parser alone. Nothing else surprising at either site.
- `mod.rs` · `remote_write_outcome`: `lines.iter().rposition(…)`, not
  `rev().position(…)` — both would work, but `rposition` on the forward
  iterator keeps `done` an index into the forward slice, which the
  `lines[..done]` arithmetic relies on; easy to "simplify" into a bug.

### Update — 2026-10-29 20:47 (end-to-end verification)

Ran the phase doc's E2E block verbatim, redirected to
`/tmp/phase07-e2e.txt`; the file's contents follow, unedited.

```
substring=0 parser_calls=2
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 2523 filtered out; finished in 0.00s
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.13s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.17s
passed=2604 failed=0
```

Matches the expected values exactly: `substring=0 parser_calls=2`, the
four parser tests green, hygiene `16 passed`, bug tracker `6 passed`,
clippy clean, and the full suite `passed=2604 failed=0` (2600 + 4). No
flake appeared in this run; both named known flakes passed on the first
try.

