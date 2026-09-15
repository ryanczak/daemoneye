# Bug 1 on phase-06a: the producer is ~10% unreliable, and its core property has no discriminating test

**Severity:** major
**Status:** closed (fixed 2026-09-15 at architect takeover)
**Filed:** 2026-09-15

Two findings. Both were reproduced at review; both were disclosed in good
faith by the run, which is why this is a bounce and not a rewrite.

---

## Finding 1 — the deliverable fails about one run in ten

`read_shell_returns_clean_output_after_a_real_command` — the test that exists
to prove this phase's whole point — fails intermittently. Reproduced at
review, **2 failures in 20 consecutive runs**:

```
panicked at src/daemon/executor/shell_exec.rs:747:9:
the command's output is recorded and readable: Error: shell s0 (chat:read-shell-e2e)
has a cast log but its command index …/s0-1789479115.meta.json is missing or unreadable.
```

The run disclosed this at 3-in-20 and could not attribute it. Note the
message: the cast log **exists**, so `before` was `Some` and the
`no cast file before the command` branch is **not** the path. The settle loop
ran and broke `None` — `marked_inner_range` did not find both markers inside
`[before, after)` before the deadline.

**One hypothesis is ruled out.** The run's Update Log proposes that
`run_in_shell_with` snapshots `before` *"once per shell session (per-owner
call), so a second command can inherit a stale `before`"*. That is **not what
the code does**: `before` is read at `src/daemon/executor/shell_exec.rs:120`,
immediately before this command's `run_command`, so it is per command. Do not
spend the run chasing it.

**One hypothesis is plausible and unverified** — offered as a lead, not as
the answer. `marked_inner_range` decodes and searches **one cast line at a
time**:

```rust
    for line in String::from_utf8_lossy(cast_slice).lines() {
        let mut decoded = Vec::new();
        decode_payload(Some(line.to_owned()), &mut decoded);
        if find_subslice(&decoded, &begin).is_some() {
```

A PTY read can **split** a marker across two chunks, and therefore across two
cast event lines; neither line's decoded payload then contains the whole
needle and the search misses. The function's own comment anticipates markers
being *bundled* with output, but not *split* across events. The latent
`windowed_payload` panic the run found (`range end index 427 out of range for
slice of length 61`) is independent evidence that payloads do get cut
mid-marker. **Verify before fixing** — the DoD below is the flake going away,
not this mechanism being adopted.

---

## Finding 2 — mutation 1 still has no test that catches it

The run disclosed that mutation 1 (producer stores the raw `[before, after)`
window instead of the clean inner range) only reddened **1 of 6** runs,
because the reader's `windowed_payload` strips framing per payload regardless
of the stored range. It added a boundary assertion to
`a_run_command_appends_a_meta_index_record` to make the property
reader-independent. **That assertion does not fire.** Re-run at review against
the same mutant, **6 of 6 green**:

```
--- mutant (raw range): 6 runs of the RECORD test ---
test result: ok. 1 passed; 0 failed      (x6)
```

The reason is the same raw-vs-decoded confusion as the escape defect the run
itself diagnosed. The assertion reads **raw cast bytes**:

```rust
        assert_ne!(
            bytes.get(start),
            Some(&0x1f),
            "the stored range must not begin on a marker's framing byte (raw): {record:?}"
        );
```

but with the raw range, `start == before`, which is a **cast line boundary** —
the byte there is `[`, the opening bracket of a JSON event line, never `0x1f`.
Likewise `"BEG"` is not in the first 8 raw bytes, because the line begins
`[1.234, "o", "…`. The assertion is checking raw newline-delimited JSON for
patterns that only exist **after** decoding.

So the phase's central property — *the producer stores the clean inner range,
not the raw window* — is currently pinned by nothing deterministic.

---

## What should happen

- The deliverable test passes reliably, not nine times in ten.
- Mutation 1 reddens a named test **every** run.

The second is the one to design for: an assertion over the **decoded** payload
of the stored range, or over `read_shell`'s output, that a raw range cannot
satisfy. Whatever you choose, the DoD is that the mutant goes red 6/6 — not
that a particular assertion exists.

## Root cause

Both are the raw-cast-bytes-versus-decoded-payload boundary, the same seam
that produced the escape defect this phase already corrected. Finding 1 is a
search that decodes per line and so cannot see across a line; finding 2 is an
assertion that does not decode at all. Architect-side contribution: § Spec
task 4 described the finder as operating on "the cast bytes recorded while one
command ran" without saying that a marker may be split across events, and the
adopted correction in the escalation entry did not either.

## Definition of done

- [ ] **The flake is gone:** `read_shell_returns_clean_output_after_a_real_command`
      passes **30 consecutive runs**. Show the loop and its verdict. Against
      the tree today it fails roughly 1 in 10 (measured 2/20 at review).
- [ ] **Mutation 1 reddens deterministically.** With the producer storing
      `(before, after)` instead of the clean inner range, a named test must go
      **red in 6 of 6 runs**. Capture the six verdicts. Today the record test
      is green 6/6 under that mutant.
- [ ] Revert the mutant and show the same test green; the shipped producer
      must still store the clean inner range.
- [ ] `cargo test --lib 'daemon::executor::shell_exec'` → **12 or more**,
      `0 failed`.
- [ ] `cargo test --lib` → **1877 or more**, `0 failed`, on three consecutive
      runs.
- [ ] `cargo test --lib 'knowledge::shell'` → **44**, unchanged;
      `cargo test --test doc_truth` → **10**, unchanged.
- [ ] All four gates pass, and no `#[allow(...)]` is added:
      `git diff a8f93ef2..HEAD -- 'src/*.rs' | grep -cE '^\+.*#\[allow'` → **0**.

## How to fix — finding 1 (run at review, admissible)

Applied to the tree left by run `511b0b6d` (whose `marked_inner_range`
rewrite is correct and should be kept), this single change eliminates the
flake — **0 failures in 30 consecutive runs** of
`read_shell_returns_clean_output_after_a_real_command`, record test 5/5,
`shell::log` 21, clippy clean:

```rust
// src/shell/log.rs, windowed_payload, END branch — `p` is already relative to
// `from = &data[start..]`, and the caller slices `from[..stop]`.
-                Some(start + p)
+                Some(p)
```

With that, the `.min(from.len())` clamp is no longer load-bearing (it was
masking this overshoot); it may stay or go. The "latent panic" recorded by the
earlier run was this same overshoot, not hostile content — remove that
follow-up from the milestone README when this lands.

Finding 2 is **not** addressed by this and still needs its own test.

## Not in scope for this bug

- **The `windowed_payload` panic** the run flagged — **now known to be the same off-by-`start` fixed under finding 1, not hostile content; closed with it.** Original note:
  (`src/shell/log.rs`, END branch: `stop` can exceed `from.len()`, so
  `from[..stop]` panics on a payload truncated mid-marker). It is real and
  reachable from hostile cast content. It is **recorded as a follow-up in the
  milestone README**, not fixed here — unless your fix for finding 1 happens
  to land in that function, in which case say so in the Update Log.
- The `pub(crate)` widening of `extend_output_line` plus the `#[cfg(test)]`
  re-export in `src/shell/mod.rs`. Both files were authorized, the change is
  test-only, and it was disclosed. **Ruled scope-consistent at review — keep
  it.**


## Resolution — 2026-09-15 (architect takeover)

- **Finding 1:** the `Some(start + p)` → `Some(p)` fix landed; deliverable
  test 0/30 on the final tree. The cancelled run's split-marker search rewrite
  was kept. The "latent panic" was this overshoot.
- **Finding 2:** the original mutation was withdrawn as not observable (PTY
  chunk bundling; the reader re-windows the accumulated buffer and keeps only
  the last BEGIN..END). Replaced by two deterministic mutations, each 6/6 red
  with green revert, and a new index-level test
  `read_shell_since_command_excludes_the_previous_commands_output`. The
  non-firing raw-byte assertions were removed.
- `shell_exec` 13, lib 1878, all gates green.
