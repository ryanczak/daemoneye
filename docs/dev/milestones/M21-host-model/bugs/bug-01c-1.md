# Bug 1 on phase-01c: the recorded `PASTE MATCH` is false — the check fails against the tree

**Severity:** major
**Status:** closed 2026-09-22 — both findings verified fixed at review. `PASTE MATCH` now prints against the repair-round entry (anchored at its first fenced block) and the review independently re-ran the whole § End-to-end verification block, reproducing all eight captured values byte for byte. Production region restored byte-identical to `6e81cef9` — `PROD UNCHANGED`. **Residual, waived:** the repair entry says the restored comment was "re-point[ed]" at Mutation A; it was in fact restored verbatim, still reading "Mutation B" (it is phase-01b's comment, and restoring it verbatim is what `PROD UNCHANGED` requires — the two cannot both be true). DoD item 3 is therefore unmet on a single descriptive sentence; corrected in the review verdict rather than bounced a third time.
**Filed:** 2026-09-21

## What's wrong

The code fix is correct and every gate is green — this bug is entirely about
the phase's **evidence**, and about two Update Log claims that are not true.

**1. `PASTE MATCH` is recorded, but the spec's own check prints
`PASTE MISMATCH`.**

`### Update — 2026-09-22 02:05 (end-to-end verification)` ends with the literal
line `PASTE MATCH`. Running the § End-to-end verification block's second
snippet verbatim against the committed tree:

```
$ D=docs/dev/milestones/M21-host-model/phase-01c-prompt-echo-race.md
$ L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
$ tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-01c.txt
$ diff /tmp/pasted-01c.txt /tmp/e2e-01c.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
1,2c1,8
< the credential from the prompt phase must be absent from the whole cast
< test result: FAILED. 0 passed; 1 failed
---
> full1=ok
...
PASTE MISMATCH
```

The extraction pulls the entry's **first** fenced block, which is the Mutation A
output. The captured `full1=ok …` values are that entry's **third** fenced
block. The spec is explicit: paste the file's contents "**as that entry's first
fenced block**."

The captured values themselves are honest — `/tmp/e2e-01c.txt` exists and its
eight lines match the third block byte for byte, and this review independently
re-ran all of them green. **The defect is that a mechanical self-check was
reported as passing when it fails.** That check is the only thing standing
between a real transcript and a retyped one, and a reviewer who trusts the
recorded `PASTE MATCH` learns nothing from it.

**2. "Production `src/shell/backend.rs` is untouched" is not true.**

The same entry states: "Production `src/shell/backend.rs` is untouched — both
edits are inside the test." Commit `f86b8eab` makes **three** edits; the third
deletes a two-line comment in `PtyBackend::input`, at `src/shell/backend.rs:144-145`
(`mod tests` begins at line 244, so this is the production region):

```
-        // Mutation B in the Update Log deletes this branch and shows the same
-        // test going red on finding the credential in the cast.
```

The deletion was neither authorized by § Authorizations (which permits a
production edit "**only if** § Spec Task 1's last bullet applies and you say so
in the Update Log") nor declared. The comment it removed pointed at the
mutation that proves the scrub — the same property this phase's Mutation A
re-verifies.

## What should happen

1. The § End-to-end verification block's captured output is the
   `(end-to-end verification)` entry's **first** fenced block, and the diff
   snippet, run verbatim, prints `PASTE MATCH` — because it matched, not
   because the line was typed.
2. The Update Log's account of what the commit changed matches the commit.
   Either the production region is genuinely untouched, or the production edit
   is named in the Update Log with its reason.

`STANDARDS.md` §1 requires end-to-end evidence "captured mechanically" and
notes a transcript fails that box "**even when every claim in it is true**."
`WORKFLOW.md` § E2E carries the `PASTE MATCH` recipe for exactly this reason.

## Root cause

The `(end-to-end verification)` entry was written as a narrative that opens
with the mutation results and reaches the captured values only in its third
fenced block. The spec's extraction is positional — `awk ... c==1` takes the
first block after the heading — so the entry's structure, not its content,
is what breaks the check. The `PASTE MATCH` line was then written from the
expectation that the evidence was correct rather than from the command's
output; the evidence *is* correct, which is why the discrepancy produced no
visible symptom.

The production-comment deletion is a plausible tidy-up — the comment's
"Mutation B" reference is stale under this phase's numbering, where the same
mutation is A — that was made silently and then contradicted in prose.

## Definition of done

- [ ] The `(end-to-end verification)` entry is restructured so the captured
      `full1=ok … ignore_count=0` values are its **first** fenced block (the
      mutation results may follow it), and re-running the § End-to-end
      verification block — both snippets, capturing to `/tmp/e2e-01c.txt`
      freshly — prints the literal `PASTE MATCH`. Confirmed **FAILING** at
      filing: prints `PASTE MISMATCH`.
- [ ] The production region of `src/shell/backend.rs` is either restored or
      its change is declared. Checked with:
      ```sh
      git show 6e81cef9:src/shell/backend.rs | sed -n '1,/^mod tests {/p' > /tmp/prod-before.txt
      sed -n '1,/^mod tests {/p' src/shell/backend.rs > /tmp/prod-after.txt
      diff /tmp/prod-before.txt /tmp/prod-after.txt && echo "PROD UNCHANGED" || echo "PROD CHANGED"
      ```
      Either this prints `PROD UNCHANGED`, **or** it prints `PROD CHANGED` and
      an Update Log entry names the deleted comment and why. Confirmed
      **FAILING** at filing: prints `PROD CHANGED` with no such entry.
- [ ] No claim in the Update Log contradicts the commit's diff.
- [ ] All four gates stay green and `cargo test --lib` stays at
      **1935 passed / 0 failed / 5 ignored** — the code fix is correct and
      must not be disturbed by this repair.
