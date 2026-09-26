# NEXT

**Active phase:** **none** — M21 phase-07a is `done` (2026-09-26,
`approved_first_try`). `read_file { host? }` reads through a shell on the
host, verified live against `pinky`. **Row 07b — edit-file-host is next**;
run `/rexymcp:architect next` to draft it. 07b, 08, 08b and 09 remain.

## MUST FIX IN 07b's SPEC — this architect shipped a false premise in 07a

07a's § Current state 4 and § Out of scope both claimed the four write ops
still need `local_read_via_buffer` and `build_local_buffer_read_cmd`, and
forbade deleting them. **They do not.** Measured at review:

```
grep -c 'local_read_via_buffer\|build_local_buffer_read_cmd' src/daemon/executor/file_ops/ops.rs
→ 0
```

`local_read_via_buffer` now has **zero** callers. Obeying the instruction left
two dead functions, and with `clippy -D warnings` the executor's only options
were `#[allow(dead_code)]` or deleting them — which the spec forbade. It chose
the allow and documented the real reason on both. Correct response to a
contradictory spec.

**`remote_run_and_capture` was pinned correctly** — four live callers at
`ops.rs:58, 268, 417, 578`, the four write branches.

**07b must therefore:** delete all three helpers and the two
`#[allow(dead_code)]` with them, and state in its § Current state that the
two read-path helpers were *already* dead at 07a's close — not repeat 07a's
claim.

**Calibration:** the premise came from reading `read.rs`'s branch and assuming
the write ops shared its helpers, instead of grepping `ops.rs`. **When a spec
says "X still uses Y, do not delete Y", grep for Y's callers before writing
the sentence.** The measure-don't-infer rule applies to claims about the call
graph, not only to system facts.

**Next after 07a: 07b — edit-file-host**, then 08, 08b, 09.

Nineteen rows in M21; live host `pinky` (sudo requires a password since
2026-09-22); `max_turns` 1000.
