# Bug 3 on phase-07c: the ordering-guard test passes with the ordering reversed

**Severity:** major
**Status:** fixed
**Filed:** 2026-09-08

## What's wrong

`shell_exec_pipeline_is_annotate_then_mask_then_normalize`
(`src/daemon/executor/shell_exec.rs:276`) is named by the phase doc's test plan
as "the regression guard for the ordering". It is not one. Measured at review:
with `format_for_model` (`shell_exec.rs:27`) mutated to the **reverse** order —

```rust
crate::tmux::annotate_output(&normalize_output(&mask_sensitive(raw)))
```

— the whole module stays green:

```
test daemon::executor::shell_exec::tests::shell_exec_pipeline_is_annotate_then_mask_then_normalize ... ok
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 1636 filtered out
```

The test's input is the F3 byte string
`"AKIAIOSFODNN7EXAMPLE\r\n\x1b[31mred line\x1b[0m\r\n"`, in which the secret and
the escapes never overlap. Mask-first still masks the secret, and annotate-last
still turns the colour into `[ERROR: …]`, so both orderings produce the same
string. The one assertion in the module that mentions ordering cannot observe it.

This is the second test-that-cannot-fail on this phase (see bug-07c-2).

## What should happen

The ordering is not cosmetic — it is the phase's own security contract. From
`format_for_model`'s doc comment (`shell_exec.rs:24-26`) and F3: *"the
annotation step must strip control bytes before masking, or an escape embedded
mid-token could split a secret past its regex."*

Measured at review, both orderings, on an input where they differ — a secret
with an escape **inside** it:

```
raw = "AKIAIOSFOD\x1b[0mNN7EXAMPLE\r\n"

annotate → mask (shipped)  PROBE out="<AWS_KEY>"
mask → annotate (reversed) PROBE out="AKIAIOSFODNN7EXAMPLE"
```

The reversed pipeline emits the AWS key **verbatim into the tool result**. That
is exactly the M20 masking exit criterion failing, and the phase's guard test
sits green through it. A guard for this contract must use an input on which the
two orderings disagree.

The shipped code is correct. Nothing about `format_for_model`'s body needs to
change — this bug is about the coverage that is supposed to keep it correct.

## Root cause

The phase doc's test plan prescribed the exact input (`"feed the exact F3 byte
string"`), and the F3 string was chosen to demonstrate the *transformations*,
not to discriminate the *order*. The test was written to the letter of the spec
and inherited its blind spot. Architect-side in origin; the missing coverage is
still real and is fixable only here.

## Definition of done

Each was run against the current tree and returns the "now" value shown.

- [ ] A test in `src/daemon/executor/shell_exec.rs` feeds `format_for_model` a
      secret with an escape sequence embedded mid-token and asserts the result
      is masked: `grep -c 'AKIAIOSFOD.x1b' src/daemon/executor/shell_exec.rs`
      → **1 or more** (now `0`).
- [ ] `cargo test --lib daemon::executor::shell_exec::` reports **7 passed;
      0 failed** (now `6 passed`).
- [ ] **The new test is red against the reversed pipeline.** Substitute
      `format_for_model`'s body with
      `crate::tmux::annotate_output(&normalize_output(&mask_sensitive(raw)))`,
      run `cargo test --lib daemon::executor::shell_exec::`, and paste the
      failing `test result:` line and the failing test's name into the Update
      Log; then restore the body and paste the green run. A mutation that
      leaves the module green does not close this bug.
- [ ] `shell_exec_pipeline_is_annotate_then_mask_then_normalize` still exists
      and still passes — it is a fine transformation test, it is just not the
      ordering guard. Do not delete it.
- [ ] All four gates pass; the twelve preserved module counts in § Acceptance
      criteria are unchanged; `cargo test --lib` reports `0 failed` on three
      consecutive runs.
