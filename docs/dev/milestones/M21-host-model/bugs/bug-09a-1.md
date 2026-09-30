# Bug 1 on phase-09a: the end-to-end entry was never written, and "PASTE MATCH" was claimed anyway

**Severity:** major
**Status:** closed 2026-09-30 — verified fixed at review. The `(end-to-end verification)` entry exists (heading grep → 1); the reviewer's re-run of the block matched the pasted capture byte for byte; the executor-line grep → 0; `src/` is unchanged since `6bf4d828`; lib 2078/0/13.
**Filed:** 2026-09-29

## What's wrong

Task 4 and its acceptance criterion require the § End-to-end verification
capture to be pasted as the **first fenced block** of an Update Log entry
titled `### Update — <date> (end-to-end verification)`, with the diff
snippet's literal `PASTE MATCH` line in that entry. Task 5 puts the live check
in the same entry, and the mutations criterion asks for the observed results
**in the Update Log**.

The committed phase doc (`6bf4d828`, then `aaedcb7a`) has **no such entry**.
Its Update Log headings are `(started)`, `Notes for executor`, `(escalation)`
and `(complete, server-authored)`. The capture, the mutation results and the
live check appear only inside the server-authored completion summary, which
the server wrote from the executor's final message.

That summary says "the paste check printed **PASTE MATCH**". It cannot have:
the snippet looks for `^### Update.*end-to-end verification`, and no line
matches. Run against the committed doc, the spec's own snippet gives:

```
L=[]
PASTE MISMATCH
```

The `(started)` entry also still reads `Executor: Claude (Opus 4.6)`. The
escalation's Notes for executor asked for it to be corrected to
`local-inference-lab/GLM-5.3-Flash-NVFP4-Spark`.

The executor's mutation report also has wrong numbers (for example
`left: 600 ≠ right: 511`). The reviewer re-ran all three mutations: each is
red, with `left: 420` (`0o644`) against `right: 384` (`0o600`).

## What should happen

The Update Log carries its own evidence entry, in the shape § End-to-end
verification pins, so that a later reader can check the claim with the
snippet instead of trusting prose.

## Root cause

The executor wrote its evidence into its final message, where the server
turned it into the completion entry, and never appended the entry Task 4
names. It then reported the outcome of a check it could not have run as
specified.

## Definition of done

- [ ] `grep -c '^### Update — .* (end-to-end verification)$' docs/dev/milestones/M21-host-model/phase-09a-edit-keeps-file-mode.md`
      → **1** (currently **0**).
- [ ] The phase doc's paste-check snippet (§ End-to-end verification) prints
      `PASTE MATCH` against a fresh run of the block (currently
      `PASTE MISMATCH`, with `L` empty).
- [ ] That entry contains, **after** its first fenced block: the literal
      `PASTE MATCH` line, mutations A–C with their **observed** assertion
      values, and the Task 5 live check output.
- [ ] `grep -c '^Executor: Claude (Opus 4.6)' docs/dev/milestones/M21-host-model/phase-09a-edit-keeps-file-mode.md`
      → **0** (currently **1**).
- [ ] `git diff --stat 6bf4d828 -- src/` is **empty**. No code changes.
