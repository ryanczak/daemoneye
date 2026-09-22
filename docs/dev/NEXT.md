# NEXT

**Active phase:** **none** — M21 phase-01c is `done` (2026-09-22,
`approved_after_2`). The test gate it repaired is green: `cargo test --lib` is
**1935 passed / 0 failed / 5 ignored**, five consecutive full-suite runs,
independently re-verified at review. **04c — run-command-host is unblocked**;
run `/rexymcp:architect next` to advance the pointer to it.

The account below is the drafting-time record of why 01c was inserted ahead of
04c. It is kept for the history; its "is red" claims describe the tree at
`73bc8586`, not today's.

**This is not the row that was queued, and the reason matters.** Drafting 04c
began with measuring the tree, and the measurement found that **`cargo test` is
deterministically red on master**. One test fails every full-suite run and
passes every run in isolation:

```
$ for i in 1 2 3 4 5; do cargo test --lib | tail -1; done
FAILED. 1934 passed; 1 failed; 5 ignored      (five for five)

$ cargo test --lib -- --test-threads=1
ok. 1935 passed; 0 failed; 5 ignored
```

No phase can meet the Definition of Done while that is true, because
STANDARDS §1 requires the suite to pass. So 01c is dispatched first and
**04c is unchanged, just queued behind it.**

**It was a flake this morning and is a blocker now.** At 04b's drafting it
failed roughly one run in four; 04b then added eight tests, and the extra
parallel load turned it deterministic. The review that approved 04b saw it
fail once and pass on re-run, which is exactly what the spec told the
executor to expect. That tolerance is what expired.

**Root cause, measured not guessed.** The secret detector arms on bash
echoing the fixture's own command line, because that line contains the literal
prompt text. The flag is therefore armed roughly 60 ms before `read -rs` ever
turns echo off, and a fixed 50 ms sleep was the only thing holding the answer
inside the real echo-off window. The scrub itself is working correctly — the
credential's input event is `[redacted]`, and it reaches the cast as an
**output** event, the shell echoing it back. This is the same bug
`wrap_command` already solved for the marker protocol by splitting the
`DE_''BEG` literal.

**Two candidate fixes were prototyped at drafting and both failed**, and the
spec records them so the executor does not repeat them. Reordering the fixture
to disable echo before printing the prompt made it fail in isolation too;
splitting the prompt literal while deleting the sleep also failed in
isolation. The conclusion handed to the executor is that the split literal and
the wait condition have to be fixed together, with the wait on a real
condition rather than a duration. The spec states the symptom, the root cause
and the finish condition, and deliberately does not prescribe the edit.

**The security property is protected by a mutation.** A "fix" that relaxes the
assertion, drops the whole-file negative or adds `#[ignore]` is a regression,
so the phase pins a mutation that defeats the scrub and must turn the test
red, plus a criterion that the `#[ignore]` count in that file stays zero.

**Next after 01c: 04c — run-command-host.** `run_terminal_command { host? }`,
the one-shot remote form, plus host-keyed reuse in `shell_for_owner` (04b left
it local-only because nothing could ask for a remote shell yet). Measurement
for it is already done and points at a likely split: `AiEvent::ToolCall` is a
six-field positional tuple whose `shell` and `host` would sit adjacent as two
`Option<String>`, a silent-transposition hazard worth converting to a struct
variant; and `ShellTarget` needs a host, which is nine sites in one file.
Row 04c must also fix the `open_shell_test_seam` flaw the 04b review recorded,
before it adds tests that would trip over it.

Fourteen rows in M21; live host `pinky`; `max_turns` 1000.
