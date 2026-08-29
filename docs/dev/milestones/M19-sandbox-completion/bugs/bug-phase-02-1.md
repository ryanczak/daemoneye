# Bug 1 on phase-02: the end-to-end entry has no `PASTE MATCH` verdict line

**Severity:** minor
**Status:** open
**Filed:** 2026-08-29

## What's wrong

The code is correct — the source diff of commit `8223ac8` is **byte-identical**
to the architect's prototype, all four gates are green on independent re-run
(1464 passed / 0 failed / 4 ignored), every structural criterion reads its
pinned value, and a reviewer mutation (dropping the `/de/scripts/` prefix from
`staged_script_command`) fails exactly `sandbox_staging_rewrites_to_the_staged_path`.

The **evidence artifact** is short one required line. The entry
`### Update — 2026-08-29 18:25 (end-to-end verification)` in
`docs/dev/milestones/M19-sandbox-completion/phase-02-staging-integration.md`
ends at its closing fence (line 779) and is followed directly by the
server-authored `(complete)` entry (line 781). Measured at review:

```
$ grep -c '^PASTE MATCH$' docs/dev/milestones/M19-sandbox-completion/phase-02-staging-integration.md
0
```

The only `PASTE MATCH` outside the spec's own text is `- PASTE MATCH ✓` at
line 808 — inside the executor's completion-summary checklist, which the
server copies into the `(complete)` entry. That entry never satisfies an
evidence requirement (`STANDARDS.md` § 1, `WORKFLOW.md` § "End-to-end
verification"), and a tick in a summary is a claim, not the verdict line the
self-check prints.

## What should happen

The phase doc's acceptance criterion, verbatim: *"The § End-to-end entry
contains the literal line `PASTE MATCH` (bare, with no surrounding
backticks)."* § End-to-end verification says where it goes: *"run the
self-check and paste its verdict line into the same entry **bare, on its own
line, with no backticks**."*

`WORKFLOW.md` § "End-to-end verification", mechanic 2, is the rule this
criterion enforces: *"The literal `PASTE MATCH` line goes **into** the entry.
Twice an artifact verified byte-exact at review while the verdict line was
absent from the entry — which moves the check back to the reviewer, silently."*
That is exactly what happened here: the reviewer re-ran the self-check and it
printed `PASTE MATCH`, but the entry does not carry it.

## Root cause

The self-check was run and its verdict transcribed into the executor's final
message rather than appended to the Update Log entry. The transcript paste
itself is byte-exact — the failure is the last step of Task 9, not the
capture.

## Definition of done

Per `WORKFLOW.md` § "One entry per dispatch, not per phase": the round that
fixes this needs its **own** end-to-end entry. Do not edit the existing
18:25 entry.

- [ ] `rm -f /tmp/e2e-02.txt` first, so the new artifact holds only this
      round's output; then Tasks 7, 8 and 9 re-run in full — mutation markers,
      the § End-to-end block verbatim, the paste, the self-check.
- [ ] A new `### Update — <date> (end-to-end verification)` entry exists
      **after** the server-authored `(complete)` entry, holding the new
      artifact as a fenced block followed by the verdict line, bare.
- [ ] `grep -c '^PASTE MATCH$' docs/dev/milestones/M19-sandbox-completion/phase-02-staging-integration.md`
      prints `1` (**measured 0 on the tree under review**).
- [ ] The self-check recipe in § End-to-end verification, run by the
      reviewer against the last entry, prints `PASTE MATCH`.
- [ ] No source file changes. `git diff 8223ac8 -- src/` is empty.
