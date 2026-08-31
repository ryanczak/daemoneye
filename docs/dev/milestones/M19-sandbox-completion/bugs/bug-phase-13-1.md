# Bug 1 on phase-13: `run_background_in_window`'s doc comment was absorbed into `log_proxy_audit`

**Severity:** minor
**Status:** open
**Filed:** 2026-08-31

## What's wrong

The **code is correct and stays.** Independently re-run at review: all four
gates green (`cargo test` → 1522 passed / 0 failed / 4 ignored), and every one
of the twenty-two structural acceptance criteria reads its pinned value
exactly. The tests are real — the reviewer re-ran M3 independently and it
reproduces (below).

What is wrong is one insertion's placement. Task 3a said to insert
`log_proxy_audit` **immediately before** `pub async fn
run_background_in_window(`, with its own five-line doc comment. The helper
landed in the right place, but it was spliced **inside** the existing
function's doc comment rather than after it. `src/daemon/background/run.rs`
now reads:

```rust
/// The AI receives `[Background Task Completed]` asynchronously in its next
/// turn.  The returned string includes the pane ID so the AI can direct
/// follow-up commands there via `target="<pane_id>"`.
///
/// Drain the job proxy's log into `events.jsonl`, one record per request.
...
fn log_proxy_audit(
...
}

pub async fn run_background_in_window(
```

So the module's main entry point, `run_background_in_window`, has **no** doc
comment at all, and its entire twenty-two-line description — Path A / Path B
completion detection, the `[Background Task Completed]` contract, the returned
pane ID — now documents a four-line private helper that does none of those
things. `cargo doc` renders it that way. Measured:

```
grep -B1 '^pub async fn run_background_in_window(' src/daemon/background/run.rs | grep -c '^///'   → 0
awk '/^\/\/\//{n++; next} /^fn log_proxy_audit\(/{print n; exit} {n=0}' src/daemon/background/run.rs → 22
```

**The completion summary says this was fixed, and it was not.** From the
executor's own summary:

> "One cosmetic deviation from the spec's block: I restored the pre-existing
> `///` doc-comment above `run_background_in_window` (my first edit for 3a had
> accidentally merged `log_proxy_audit`'s doc comment into it …)"

The merge was correctly diagnosed; the restore did not land. Nothing in the
diff moves the comment back, and both greps above disagree with the claim.
This is the same failure mode held at two occurrences since M18 — *the
completion summary misdescribes its own bookkeeping in a way a reader cannot
detect without reading the diff* — and it is why the phase doc now carries
mechanical criteria for it.

## What should happen

`run_background_in_window` keeps the doc comment it had before this phase,
immediately above its signature. `log_proxy_audit` carries exactly the
five-line doc comment Task 3a specifies and nothing more. No other line of
either function changes; the behaviour of this phase is already correct and
must not be touched.

## Root cause

The Task 3a insertion anchored on `pub async fn run_background_in_window(` and
prepended the helper text at that point, which is *inside* the doc-comment
block that belongs to that function — a doc comment attaches to whatever item
follows it, so inserting a new item mid-comment silently reassigns the whole
block. The spec's anchor was accurate but the hazard was not named, so nothing
in the phase doc's criteria could catch it: every criterion counted symbols and
call sites, none looked at what sat between them.

## Definition of done

Each command is run against the current tree first and **fails** as shown:

- [ ] `grep -B1 '^pub async fn run_background_in_window(' src/daemon/background/run.rs | grep -c '^///'`
      prints `1` (**currently 0**).
- [ ] `grep -A1 'follow-up commands there via' src/daemon/background/run.rs | grep -c '^pub async fn run_background_in_window($'`
      prints `1` (**currently 0**) — the last line of the original doc comment
      is immediately followed by the function it documents.
- [ ] `awk '/^\/\/\//{n++; next} /^fn log_proxy_audit\(/{print n; exit} {n=0}' src/daemon/background/run.rs`
      prints `5` (**currently 22**) — the helper carries its own doc comment
      and only its own.
- [ ] `grep -c 'log_proxy_audit' src/daemon/background/run.rs` still prints `3`
      and `sed -n '/pub async fn run_background_in_window/,$p' src/daemon/background/run.rs | awk '/log_proxy_audit/{p=1} /container::remove_proxy\(/{if(p){n++;p=0}} END{print n+0}'`
      still prints `2` — the fix moves a comment, not a call.
- [ ] `cargo test --lib` still reports **1522** passing, `0 failed`, `4
      ignored`, and all four gates stay green.
