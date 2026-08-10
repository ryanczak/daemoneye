# Bug 1 on phase-04: End-to-end evidence entry diverges from the real capture file

**Severity:** blocker
**Status:** verified
**Filed:** 2026-08-09

## What's wrong

The phase doc's `### Update — 2026-08-10 00:31 (end-to-end verification)` entry
(`docs/dev/milestones/M13-chat-ux/phase-04-cursor-alignment.md:372-419`) is
supposed to be the verbatim contents of `/tmp/e2e-m13-04.txt`, produced by the
`## End-to-end verification` script plus Tasks 6-7's mutation runs appending
into the same file (per Task 8: "paste the resulting `/tmp/e2e-m13-04.txt`
into a new Update Log entry ... verbatim and unmodified").

`/tmp/e2e-m13-04.txt` still exists on disk (untouched since the executor's run,
2026-08-09 18:03 mtime). Diffing it against the pasted block shows two
factual divergences, not just formatting:

```
-test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 1224 filtered out; finished in 0.00s
+test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
```

This occurs twice — once under `== M1 APPLIED ==` and once under
`== M2 APPLIED ==`. The real file says `1224 filtered out`; the phase doc says
`0 filtered out` in both places.

This is not a transient value. Independently re-running the M1 mutation
(`col.min(content_area.width.saturating_sub(2))`) against the current tree and
running `cargo test --lib cursor_clamp_never_reaches_border` reproduces
`1224 filtered out` on the FAILED result, matching `/tmp/e2e-m13-04.txt` and
contradicting the phase doc's pasted `0 filtered out`. `filtered out` reports
how many tests the `--lib <name>` filter excluded (1224 of the crate's 1225
lib tests do not match the single-test filter); that count does not change
between a passing and failing outcome of the one matched test, so `0` cannot
come from a real run of this command against this codebase.

## What should happen

Per `STANDARDS.md` §1: "Every end-to-end transcript is captured
mechanically... A transcript that is retyped, paraphrased, summarised into
prose, or assembled from more than one run fails this box **even when every
claim in it is true**." Here the claim itself (that the test FAILED under
each mutation) is still true, but the surrounding transcript was not
mechanically pasted — it was reconstructed with an incorrect value at two
sites, which is worse than mere retyping because the pasted evidence is now
factually wrong versus the real captured file.

The Update Log's E2E entry must be the exact, unedited contents of
`/tmp/e2e-m13-04.txt` as produced by the End-to-end verification script and
Tasks 6-7's mutation runs appending into it in order.

## Root cause

The server-authored `(complete)` entry's "Command output tails" block
(`phase-04-cursor-alignment.md:437-554`) is a synthesized re-run of the four
gate commands, not the mutation-testing transcript — that part is fine and
matches STANDARDS.md's caveat that it doesn't by itself satisfy the E2E box.
The actual `(end-to-end verification)` entry at `:372-419`, which is supposed
to satisfy the E2E box, was pasted by the executor with the FAILED-run
`filtered out` counts altered from `1224` to `0` at both mutation sites,
diverging from the real `/tmp/e2e-m13-04.txt` artifact still present on disk.

## Definition of done

- [x] `diff /tmp/e2e-m13-04.txt <(sed -n '/### Update.*end-to-end verification/,/^```$/p' docs/dev/milestones/M13-chat-ux/phase-04-cursor-alignment.md | sed '1,2d;$d')`
      (or equivalent manual extraction of the fenced block) produces **no
      output** — confirmed FAILING against the current tree: the diff above
      shows two mismatched lines (`1224 filtered out` vs `0 filtered out`).
      **Re-verified 2026-08-10 at round-2 review**: extracted the fenced block
      of the `### Update — 2026-08-10 (end-to-end verification, round 2)`
      entry and diffed it byte-for-byte against `/tmp/e2e-m13-04.txt`
      (mtime 18:17, the round-2 capture) — no output, exit 0. Both mutation
      FAILED lines now read `1224 filtered out`; SURFACES block reads
      `wrap calls: 1`, `stale widths: 0`, `clamps: 2`, matching a fresh
      independent re-run.
- [x] Re-run the `## End-to-end verification` script plus Tasks 6-7 verbatim
      to `/tmp/e2e-m13-04.txt`, and paste the fresh file's contents into a new
      `### Update — <date> (end-to-end verification)` entry, replacing the
      stale one. Do not hand-edit any line of the pasted block.
      **Confirmed 2026-08-10**: the round-2 entry is the mechanical, unedited
      contents of the regenerated `/tmp/e2e-m13-04.txt`; the executor's own
      `PASTE MATCH` self-check output is appended after the closing fence,
      not inside it — correctly kept out of the transcript proper.
