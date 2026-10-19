# Phase 02: small-fixes — masker `hostname`, results name their shell, the `sbx.rs` flake

**Milestone:** M29 — Post-2.0 fixes
**Status:** todo
**Depends on:** phase-01
**Estimated diff:** ~120 lines
**Tags:** language=rust, kind=fix, size=s

## Goal

Three independent defects deferred from M28.

- **O3, masker false positive.** The ssh-config rule in `src/ai/filter.rs`
  (the `<SSH_HOST>` pattern) is case-insensitive and uses `\s+`, which
  crosses a newline. So the output of the `hostname` command (`hostname`
  on one line, `pinky` on the next) is masked, and so is prose such as
  `the hostname is pinky` or a command line such as `hostname -f`.
- **O4, results don't name their shell.** A successful waited
  `run_terminal_command` returns only the output. The model then has to
  guess which shell the command ran in.
- **The `sbx.rs` flake.** `script_invocation_accepts_an_absolute_path_under_scripts_dir`
  reads `HOME` ambiently, through `crate::config::scripts_dir()`, without
  holding the test-`HOME` lock. Another test's `HOME` change can land
  between its two reads. It flaked in two M28 phases.

## Spec

**Files:** `src/ai/filter.rs`, `src/daemon/executor/shell_exec.rs`,
`src/daemon/executor/sbx.rs`.

1. **The masker matches ssh-config lines only.** The `<SSH_HOST>` rule
   matches `HostName` or `IdentityFile` (still case-insensitive, as ssh
   config keywords are) only:
   - at the start of a line, after optional spaces or tabs;
   - followed by spaces or tabs, **not** a newline;
   - then a value that does not start with `-`.

   It must mask:
   - `HostName 10.0.0.5`;
   - `  HostName 10.0.0.5` in a `Host web` block;
   - `\tIdentityFile ~/.ssh/id_ed25519`;
   - the existing cases `hostname 192.168.1.1` and
     `HOSTNAME server.internal`.

   It must **not** change:
   - `hostname\npinky`;
   - `$ hostname\npinky`;
   - `hostname -f`;
   - `the hostname is pinky`.
2. **A waited result names its shell.** `finish_run` in `shell_exec.rs` is
   the epilogue shared by the plain and sudo paths. It appends one final
   line, `[ran in shell <id>]`, after any exit-status note. Every waited
   `run_terminal_command` result then ends with it, success or failure.
   The interactive-launch text, refusals and background completions are
   unchanged. Existing tests that compare a waited result exactly will
   fail. The compiler and the test run name them; update their
   expectations to include the line.
3. **The flaky test holds the guard.** Every test in `sbx.rs` that
   reaches `crate::config::scripts_dir()`, directly or through
   `script_invocation`, holds `crate::test_home_guard()` for its whole
   body. No new test.
4. **Capture the end-to-end evidence.** Run § End-to-end verification
   verbatim. Paste `/tmp/e2e-m29-02.txt` into a new Update Log entry
   headed `### Update — <date> (end-to-end verification)`, with the
   `PASTE MATCH` line after the fence.

**Gotchas.**
- `regex` needs the multi-line flag for `^` to mean the start of a line.
  `[ \t]` is the horizontal-whitespace class.
- The shell id is appended **after** masking, so it is never masked.
- For an end-to-end waited command in a test, follow
  `run_terminal_command_targets_the_named_shell` (`shell_exec.rs` tests).
- Find anchors with `grep -n`; read windows, not single lines.

## Tests

- `the_hostname_command_and_prose_are_not_masked` (`filter.rs` tests): each
  must-not string from task 1 comes back from `mask_sensitive` unchanged.
- `ssh_config_lines_are_still_masked` (`filter.rs` tests): each must-mask
  string from task 1 contains `<SSH_HOST>` and not the original value.
- `a_waited_result_names_its_shell` (`shell_exec.rs` tests): a waited
  `echo hi` ends with `[ran in shell <id>]`, where `<id>` is the shell it
  ran in. A command that exits 3 has the exit-status note **before** that
  line.

## Acceptance criteria

- [ ] The three new tests ⇒ `test result: ok. 1 passed` each.
- [ ] `script_invocation_accepts_an_absolute_path_under_scripts_dir` holds
      `test_home_guard()` (end-to-end line `guarded=1`).
- [ ] `cargo test` ⇒ about `passed=2415 failed=0` (2412 at `9b328b56`
      + 3). An extra test is fine: name it and report; do not re-run the
      suite to chase a number.
- [ ] `cargo test --test doc_truth` ⇒ `failed=0`. Clippy (`-D warnings`)
      and `cargo fmt --all --check` exit 0. No new `#[allow]`.
- [ ] The end-to-end Update Log entry holds the fence **and** the
      `PASTE MATCH` line.

## End-to-end verification

```sh
A=/tmp/e2e-m29-02.txt; : > "$A"
for t in the_hostname_command_and_prose_are_not_masked ssh_config_lines_are_still_masked a_waited_result_names_its_shell ssh_config_hostname_case_insensitive script_invocation_accepts_an_absolute_path_under_scripts_dir; do cargo test --lib "$t" 2>&1 | grep -E '^test result' | sed "s/^/$t: /; s/[0-9]* filtered out/<n> filtered out/; s/finished in [0-9.]*s/finished in <t>s/" >> "$A"; done
echo "guarded=$(grep -A4 'fn script_invocation_accepts_an_absolute_path_under_scripts_dir' src/daemon/executor/sbx.rs | grep -c 'test_home_guard')" >> "$A"
cargo test --test doc_truth 2>&1 | grep -E '^test result' | sed 's/^/doc_truth: /; s/finished in [0-9.]*s/finished in <t>s/' >> "$A"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1 | sed 's/in [0-9.]*s/in <t>s/' >> "$A"
cargo fmt --all --check; echo "fmt_exit=$?" >> "$A"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}' >> "$A"
cat "$A"
```

Run `cargo fmt --all` **before** this block. Run each line **once**. Then
run the self-check, and put its line in the entry after the fence:

```sh
D=docs/dev/milestones/M29-post-2.0-fixes/phase-02-small-fixes.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-m29-02.txt
diff /tmp/pasted-m29-02.txt /tmp/e2e-m29-02.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

## Authorizations

None.

## Out of scope

- The other masker patterns.
- Background-completion messages and the interactive-launch text, which
  already name their shell.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->
