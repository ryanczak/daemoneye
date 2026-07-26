# Bug 1 on phase-04f: two production lock sites left unconverted — the grep criterion cannot see them

**Severity:** major
**Status:** open
**Filed:** 2026-07-26

**Root cause is an architect spec defect, not executor error.** The spec's site
inventory listed 2 production sites; the file has **4**. The two it missed write
`sessions` and `.lock()` on separate lines, so `grep -c "sessions\.lock()"` —
which every acceptance criterion in this phase and its predecessors used — is
blind to them. The executor implemented tasks 1–3 exactly as written and every
criterion it was given passed. Telemetry records this as `spec_bug`.

## What's wrong

### Defect 1 — `run_compaction` still holds two raw locks (the reason for the bounce)

`src/daemon/context/background.rs:118-124`:

```rust
    let Some(tail_start) = tail_start else {
        // No viable cut — discard.
        if let Some(entry) = sessions
            .lock()
            .unwrap_or_log()
            .get_mut(&snapshot.session_id)
        {
            entry.compaction_in_flight = false;
        }
        return Ok(());
    };
```

`src/daemon/context/background.rs:137-145`:

```rust
    if let Some(last_prior) = prior.last()
        && dropped_last_turn > 0
        && last_prior.turn_end >= dropped_last_turn
    {
        if let Some(entry) = sessions
            .lock()
            .unwrap_or_log()
            .get_mut(&snapshot.session_id)
        {
            entry.compaction_in_flight = false;
        }
        return Ok(());
    }
```

Both are genuine `SessionStore` acquisitions in `run_compaction`, both clear
`compaction_in_flight` on an early-discard path, and both survived the phase.

The phase's stated Finish condition — "0 raw `sessions.lock()` in the production
region" — is therefore **not met in substance**. It only appeared met because the
measuring instrument was a single-line grep.

Verified with a multi-line scan (`sessions` at end of line, `.lock()` on the
next):

```
MULTI-LINE MISSED: src/daemon/context/background.rs:118
MULTI-LINE MISSED: src/daemon/context/background.rs:137
```

### Defect 2 — the named regression test does not guard the flag ordering

The spec's Test plan asserted that `background_swap_discards_on_new_turn` is the
discriminator for the stale branch's flag-clear-before-return ordering, and asked
the executor to confirm it by reading the assertions. The Update Log reports
"The stale-path test in the test module exercises the flag-clearing order and
passes."

**It does not.** Proven by mutation — deleting
`entry.compaction_in_flight = false;` from the stale branch and running the test:

```
$ cargo test --lib background_swap_discards_on_new_turn
test daemon::context::background::tests::background_swap_discards_on_new_turn ... ok
```

The assertion at `background.rs:434` (`assert!(!entry.compaction_in_flight)`) is
**vacuous**: `make_test_entry()` builds the entry with `compaction_in_flight:
false` and the test calls `run_compaction` directly rather than going through
`try_snapshot`, so the flag is never `true` to begin with. The assertion cannot
fail. The tree was restored after the mutation check.

The production code's ordering is **correct** — `entry.compaction_in_flight =
false;` precedes `return None;` at `background.rs:237-238`, confirmed by reading.
Only the regression net is missing.

## What should happen

1. All **four** production `SessionStore` acquisitions in
   `src/daemon/context/background.rs` go through `with_sessions`, leaving zero raw
   acquisitions in the production region **as measured by a scan that sees
   multi-line calls**.
2. The stale-path flag clearing is guarded by a test that actually fails when the
   clear is removed.

## How to fix

### 1. Convert `background.rs:118-124`

```rust
    let Some(tail_start) = tail_start else {
        // No viable cut — discard.
        with_sessions(sessions, |store| {
            if let Some(entry) = store.get_mut(&snapshot.session_id) {
                entry.compaction_in_flight = false;
            }
        });
        return Ok(());
    };
```

### 2. Convert `background.rs:137-145`

```rust
    if let Some(last_prior) = prior.last()
        && dropped_last_turn > 0
        && last_prior.turn_end >= dropped_last_turn
    {
        with_sessions(sessions, |store| {
            if let Some(entry) = store.get_mut(&snapshot.session_id) {
                entry.compaction_in_flight = false;
            }
        });
        return Ok(());
    }
```

Both `return Ok(())` statements stay **outside** the closure, exactly as in the
step-2 conversion this phase already landed. Do not move them inside.

After these two, `with_sessions(` in this file is **4**.

**`use crate::util::UnpoisonExt;` (line 13) becomes unused by production code**
once these are converted — lines 119 and 138 were its last two production users.
The test module still uses `unwrap_or_log`, so the import is used under
`cfg(test)` but **not** under a plain `cargo build`, which will warn and fail
`-D warnings`. Move the import into the test module:

- delete `use crate::util::UnpoisonExt;` from the file header, and
- add `use crate::util::UnpoisonExt;` inside `mod tests` (beside its existing
  `use super::*;`).

Verify with `cargo build` **and** `cargo clippy --all-targets`, since the two
disagree about whether a test-only import is used. This is the trap in this fix —
do not skip it.

### 3. Make the flag-ordering test real

Amend `background_swap_discards_on_new_turn` so the flag is `true` before the
call, which is what production does via `try_snapshot`:

- after inserting the entry and before calling `run_compaction`, set
  `compaction_in_flight = true` on the stored entry;
- keep the existing `assert!(!entry.compaction_in_flight)` — it now has meaning.

**This phase's "write no new tests" instruction is amended: you may modify this
one existing test.** The lib-unit count stays **915** — you are changing a test,
not adding one. Do not add any other test.

Confirm the fix discriminates: with the amendment in place, temporarily delete
`entry.compaction_in_flight = false;` from the stale branch, observe the test
**fail**, restore it, observe it pass. Quote both outcomes in the Update Log.

## Verification

- [ ] Multi-line-aware scan reports zero production raw acquisitions:
      `python3 -c "$(cat <<'PY'
import pathlib,re
p=pathlib.Path('src/daemon/context/background.rs'); L=p.read_text().splitlines()
tb=next(i for i,l in enumerate(L,1) if l.strip().startswith('#[cfg(test)]'))
n=0
for i,l in enumerate(L,1):
    if i>=tb: break
    if 'sessions.lock()' in l: n+=1
    elif re.search(r'\bsessions\s*$',l) and L[i].strip().startswith('.lock()'): n+=1
print(n)
PY
)"` prints **0**.
- [ ] `grep -c "with_sessions(" src/daemon/context/background.rs` returns **4**.
- [ ] `grep -c "sessions\.lock()" src/daemon/context/background.rs` still returns
      **11** — the test module stays untouched.
- [ ] `grep -c "use crate::util::UnpoisonExt" src/daemon/context/background.rs`
      returns **1**, and it is inside `mod tests`.
- [ ] `cargo build` succeeds with zero warnings.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` passes.
- [ ] `cargo fmt --all` passes.
- [ ] `cargo test` passes with **915** lib-unit tests — unchanged.
- [ ] `background_swap_discards_on_new_turn` **fails** when
      `entry.compaction_in_flight = false;` is deleted from the stale branch, and
      passes when restored. Both quoted in the Update Log.
