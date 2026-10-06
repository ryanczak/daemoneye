# Phase 02b: backend-flake — make the redaction-marker test model a real password prompt

**Milestone:** M26 — Agent isolation on Docker Sandboxes (sbx)
**Status:** todo
**Depends on:** phase-02
**Estimated diff:** ~30 lines, one test in one file
**Tags:** language=rust, kind=bugfix, size=s

## Goal

`shell::backend::tests::input_after_a_prompt_logs_a_redaction_marker` fails
on roughly half of full `cargo test` runs on pinky. Every phase has had to
carry a "run the suite at most twice" rule because of it. This phase fixes
the **test**. The product code is correct and must not change.

## Architecture references

- `docs/dev/milestones/M26-sbx-isolation/README.md` § "Flaky test on pinky".
- `docs/design/daemoneye-2.0.md` § 3 (security deltas): shell logs are raw;
  only `"i"` (input) events typed while a password prompt shows are
  scrubbed.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read this entire phase doc before touching any code.
3. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

### What fails, measured 2026-10-06 on pinky

The failing assertion is **not** the timeout. It is the credential check at
`src/shell/backend.rs:611`, `the credential must never reach the cast file`.
The cast from a failing run (abridged):

```
[0.071347, "i", "printf '[de-sudo-prompt-77] Password: redaction-e2e-probe\\n'\n"]
[0.073279, "o", "[de-sudo-prompt-77] Password: redaction-e2e-probe\r\n"]
[0.073482, "o", "...matt@pinky:~$ "]
[0.081485, "i", "[redacted]"]
[0.084565, "o", "hunter2-secret-test"]
```

### Root cause: two defects in the test, none in the product

1. **The fixture is not a password prompt.** The test fakes one with
   `printf`, so the shell is still at an ordinary bash prompt when the
   credential arrives, and readline **echoes the keystrokes back** as an `"o"`
   event. The input scrub works: the `"i"` event is `[redacted]`. A real
   `sudo`/`ssh` prompt turns terminal echo off, so the echo never happens.
   Echoed output is meant to be raw in the cast; only input is scrubbed.
2. **The cast is read once, before the polling loop.** The loop then re-tests
   the same stale string:

   ```rust
   let cast = cast_output(&backend, &dir);
   loop {
       if cast.contains("[redacted]") {
           break;
       }
       …
   ```

   So the test passes only when that single read lands **before** the echo is
   written. Under full-suite load the echo usually wins, and the test fails.

### The fixture to copy is already in this file

The sibling test `the_flag_clears_once_the_answer_is_submitted`
(`src/shell/backend.rs`, around line 655) does this correctly. Its comment
explains why:

```rust
// `read -s` is the fixture that silences terminal echo *around the
// answer*, the same window real sudo owns for its prompt — without
// it readline's prompt-redraw flush reflects the answer back as an
// "o" event and the whole-cast assertion below can never hold. …
// Both needles in the prompt are
// split with `''` — the idiom `pty.rs::wrap_command` uses for the
// marker — so the echoed command line matches neither `password` nor
// `de-sudo-prompt-`; the flag can then arm only on the prompt that
// `read -rs` itself emits, which it prints *after* echo is off.
let echo = "read -rs -p '[de-sudo-prompt''-78] Pass''word: ' v; printf '\\n'\n";
```

The `''` split matters. The scrub arms on any output chunk containing
`password` or `de-sudo-prompt-` (`looks_like_secret_prompt`,
`src/shell/backend.rs:50`). Without the split, the **echoed command line**
would arm it before `read` has turned echo off.

### The fix, measured

Applying exactly the diff in Task 1 on this tree gave:

- the test passing solo;
- **4 of 4** full `cargo test --lib` runs at `2248 passed; 0 failed` (it failed
  about half the time before);
- **red** when the scrub is broken. Replacing `b"[redacted]"` with `bytes` in
  `PtyBackend::input` makes it time out with `` `[redacted]` and `got 19`
  never both reached the cast``.

## Spec

### Task 1 — Rewrite the test body

In `src/shell/backend.rs`, test `input_after_a_prompt_logs_a_redaction_marker`,
apply this change. It is the measured diff; reproduce it exactly:

```diff
-        let prompt = "printf '[de-sudo-prompt-77] Password: redaction-e2e-probe\\n'\n";
+        let prompt = "read -rs -p '[de-sudo-prompt''-77] Pass''word: ' de_secret; printf '\\n'; echo \"got ${#de_secret}\"\n";
         rt.block_on(backend.input(prompt.as_bytes()))
             .expect("emit prompt");
@@
         // The answer reaching the shell must never reach the cast file.
-        rt.block_on(backend.input(CREDENTIAL.as_bytes().to_vec().leak()))
+        rt.block_on(backend.input(format!("{CREDENTIAL}\n").as_bytes()))
             .expect("submit the credential");
+        let done = format!("got {}", CREDENTIAL.len());
         let deadline = std::time::Instant::now() + Duration::from_secs(10);
-        let cast = cast_output(&backend, &dir);
-        loop {
-            if cast.contains("[redacted]") {
-                break;
+        let cast = loop {
+            let cast = cast_output(&backend, &dir);
+            if cast.contains("[redacted]") && cast_has_o_text(&cast, &done) {
+                break cast;
             }
             assert!(
                 std::time::Instant::now() < deadline,
-                "`[redacted]` never reached the cast: {cast}"
+                "`[redacted]` and `{done}` never both reached the cast: {cast}"
             );
             std::thread::sleep(Duration::from_millis(20));
-        }
+        };
```

Why each piece is there:

- **`read -rs -p`**: `-s` turns echo off, as sudo does. `-r` keeps backslashes
  literal. The prompt is the sentinel shape, split with `''`.
- **`{CREDENTIAL}\n`**: the newline submits the answer. That ends the
  `read`, and it is also what disarms the scrub (`ends_secret_entry`).
- **`echo "got ${#de_secret}"`**: `got 19` in the output proves the credential
  reached the shell, so the test waits for a **settled** cast instead of
  racing the shell. `CREDENTIAL` is 19 bytes, but the code uses
  `CREDENTIAL.len()`, so do not hard-code 19.
- **`let cast = loop { let cast = cast_output(…); … break cast; };`**: the cast
  is re-read on every iteration, and the loop yields the settled string to
  the assertions below it. Note the `;` after the loop.
- `cast_has_o_text` is an existing helper in this test module; use it as is.

Keep the arming poll (`for _ in 0..500 { … awaiting_secret … }`) and the two
final assertions (`[redacted]` present, `CREDENTIAL` absent) unchanged.

### Task 2 — Rewrite the comment above `let prompt`

The comment block above `let prompt` (from `// Emit a prompt-shaped line so
the pump's chunk arm sets the flag.` to `// armed output looks like at the
ordering boundary.`) describes the old `printf` fixture. Replace it with a
3–6 line comment saying:

- `read -rs` owns the echo window, as a real password prompt does;
- the `''` splits keep the echoed command line from arming the scrub, so it
  arms on `read`'s own prompt;
- `got N` marks the point where the cast has settled.

Point to `the_flag_clears_once_the_answer_is_submitted` for the full reasoning
rather than repeating it.

### Task 3 — Capture the end-to-end evidence

Run the block in § End-to-end verification verbatim, **once**, and paste its
output into a new Update Log entry.

## Acceptance criteria

- [ ] `grep -c "Pass''word: ' de_secret" src/shell/backend.rs` prints `1`.
- [ ] `grep -c "redaction-e2e-probe" src/shell/backend.rs` prints `0` (the
      old `printf` fixture is gone).
- [ ] `cargo test --lib input_after_a_prompt_logs_a_redaction_marker` reports
      `1 passed; 0 failed`.
- [ ] The three full library runs in § End-to-end verification each report
      `0 failed`. **Run that loop exactly once.** If any run fails, paste the
      failure and stop: do not re-run to see whether it passes. A failure here
      means the fix did not work, which is a finding for the reviewer.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` exits 0.
- [ ] `cargo fmt --all --check` exits 0.
- [ ] `git diff --stat` shows only `src/shell/backend.rs` among source
      files, and the diff touches only the body and comment of
      `input_after_a_prompt_logs_a_redaction_marker`. **No non-test code
      changes**: `PtyBackend`, `looks_like_secret_prompt` and
      `ends_secret_entry` are correct.
- [ ] The Update Log entry contains the § End-to-end verification output and
      a line `PASTE MATCH: yes`, meaning the pasted block equals the terminal
      output.

## Test plan

No new tests. The changed test is the deliverable, and the three full
library runs are its evidence. The mutation that proves it still guards the
scrub (swap `b"[redacted]"` for `bytes` in `PtyBackend::input`) was run at
drafting, and the reviewer re-runs it. **The executor does not need to run
it.**

## End-to-end verification

```sh
cargo test --lib input_after_a_prompt_logs_a_redaction_marker 2>&1 | grep -E '^test result'
for i in 1 2 3; do cargo test --lib 2>&1 | grep -E '^test result' | sed "s/^/lib run $i: /"; done
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1
cargo fmt --all --check; echo "fmt_exit=$?"
git diff --stat
```

This takes about 3½ minutes. Each line runs **once**. Expected: `1 passed`;
three lines `lib run N: test result: ok. 2248 passed; 0 failed`; clippy
`Finished`; `fmt_exit=0`; only `src/shell/backend.rs` among source files.

## Authorizations

None.

## Out of scope

- Any change outside this one test: no product code, no other test, no
  helper.
- Running the full suite more than the three runs above.
- The sbx work (rows 03+).

## Update Log

<!-- entries appended below this line -->
