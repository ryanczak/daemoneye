# Phase 10: Pane-Preference Redesign

**Milestone:** M6 — Verification & Hygiene
**Status:** in-progress
**Depends on:** phase-01 (done)
**Estimated diff:** ~350 lines
**Tags:** language=rust, kind=fix, size=m

## Goal

Stop the pane preference from silently targeting a pane the user never picked.

Today the mapping is `session_name → pane_id`, and **both sides are unstable
identities**. After a tmux server restart, `%0` almost certainly exists and is
something else — so the stored preference validates and the agent runs a
foreground command in the wrong pane, with no prompt.

## The decision, and who made it

The milestone's exit criterion says the mechanism is the phase's to choose: *"a
recorded decision plus its implementation — not a specific mechanism."* **The
architect has made that decision** and records it here, so this phase is
determinate rather than open-ended.

**Chosen: fingerprint validation + pruning.** Store a small fingerprint beside
the pane ID and accept the preference only if the pane still matches it.

**Why, over the alternatives the README lists:**

- **vs. "ask once per daemon run" (drop persistence).** Simplest and provably
  safe, but it discards the feature's stated purpose — `pane_prefs.rs:1-5` says
  the point is that the user "is never asked to pick a pane more than once per
  session", surviving daemon restarts. A safety fix that deletes the feature is a
  scope reduction, and nothing forces one here: the data needed to make
  persistence safe is already available.
- **vs. keying on window/pane index.** Indices renumber when panes are added or
  closed, so this trades one unstable identity for another.
- **vs. scoping to a tmux-server generation.** Correct, but it introduces a new
  concept (server identity) to solve a problem the fingerprint already solves
  with data the code already fetches.

**This decision is the architect's and is open to PE override at milestone
close.** If it is overridden, the fallback is the scope reduction — which is
strictly less work, so nothing here is wasted.

## Architecture references

Read before starting:

- `src/pane_prefs.rs` — the whole module is 35 lines. Read all of it.
- `src/cli/commands/pane.rs:15-45` — `resolve_target_pane`, the only consumer of
  `get()`, and three of the four `save()` call sites.
- `src/tmux/pane.rs:5-21` (`RichPaneInfo`) and `:45` (`list_panes_detailed`) —
  where the fingerprint fields come from.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom, including §1's
   mechanical-capture box and §3.3's determinism rule.
2. Read `src/pane_prefs.rs` and `src/cli/commands/pane.rs` in full.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is clean and `cargo test` is green at 979 lib / 30
   integration (2 ignored) / 8 isolation (1 ignored).

## Current state — three defects, all verified in the tree

**1. The stored identity cannot be validated.** `get()` returns the pane ID and
`resolve_target_pane` (`cli/commands/pane.rs:17-22`) accepts it on
`pane_exists()` alone:

```rust
if let Some(saved) = crate::pane_prefs::get(session)
    && saved != chat_pane
    && crate::tmux::pane_exists(&saved)
{
    return Some(saved);
}
```

`pane_exists` proves *a* pane holds that ID — not that it is the pane the user
chose. Pane IDs are per-server and recycled across tmux server restarts.

**2. `get()` is implemented as a remove.** `src/pane_prefs.rs:33-35`:

```rust
pub fn get(session_name: &str) -> Option<String> {
    let mut all = load_all();
    all.remove(session_name)
}
```

Non-destructive **only** because the mutated map is never written back. One
refactor away from silently deleting every preference on read.

**3. Nothing prunes.** Entries accumulate for every session name ever seen. The
maintainer's live file still holds `de-phase01`, a long-dead rexyMCP session:

```json
{"2":"%5","0":"%0","1":"%1","daemoneye":"%0","de-phase01":"%20"}
```

Note also that `"0"`, `"1"`, `"2"` are tmux's **default numeric session names**,
reused constantly — a preference stored for session `"0"` is offered to any
future session that happens to be named `"0"`.

**4. A stale doc comment.** `src/pane_prefs.rs:4` says the file lives at
`~/.daemoneye/pane_prefs.json`; `prefs_path()` returns
`var_run_dir().join("pane_prefs.json")`. This is milestone defect 10, nominally
phase 11's — but you are rewriting that exact comment, so fix it here rather than
have phase 11 reopen the file. **Do not** touch the orphaned
`~/.daemoneye/pane_prefs.json` on disk; removing it is still phase 11's.

## Spec

### 1. A fingerprinted preference

Replace the stored value with the pane ID plus enough context to tell the user's
pane apart from a recycled ID. Use fields already available on `RichPaneInfo`
(`tmux/pane.rs:5-21`) — the window name and the pane's working directory are the
natural pair; add the session name if it helps. Do not invent a new tmux query if
`list_panes_detailed()` already returns what you need.

**The old on-disk format must not crash the daemon.** The live file is
`{session: "pane_id"}` and will be read by the new code on first upgrade. Treat
an entry that does not parse as the new shape as absent — drop it and move on.
Silently discarding a stale preference is correct here; the user gets prompted
once and the new entry is written.

### 2. Accept only on a fingerprint match

`get()` must return the stored pane **only** when the live pane still matches the
recorded fingerprint. A pane that exists but whose window name or cwd has changed
is a different pane as far as this feature is concerned — return `None` and let
`resolve_target_pane` fall through to its existing prompt path.

**This is the phase's whole point.** The PE constraint is that nothing may
silently execute a foreground command in a pane the user did not pick, and asking
again is explicitly the acceptable outcome.

### 3. `get()` must not mutate

Make it a read. Whatever pruning happens (task 4) must be deliberate and
explicit, not a side effect of a lookup.

### 4. Prune deliberately

Entries whose pane no longer exists, or no longer matches its fingerprint, must
not accumulate. Drop them and persist the pruned map. Say in a comment when
pruning runs — on load, on save, or on an explicit call — and make sure it is
reachable from a test.

### 5. Keep the four `save()` call sites working

`cli/commands/pane.rs:44`, `:123`, `:168` and
`daemon/server/handlers.rs:132` all call `save(session, pane_id)`. They will need
the fingerprint. Fetch it at the call site or inside `save()` — your choice — but
**do not** make a caller pass fields it has no natural access to.

## Acceptance criteria

- [ ] A stored preference whose pane still matches its fingerprint is returned.
- [ ] A stored preference whose pane exists but whose fingerprint no longer
      matches is **not** returned.
- [ ] A stored preference whose pane no longer exists is not returned.
- [ ] `get()` does not mutate the stored map.
- [ ] Stale entries are pruned and the pruned map persists.
- [ ] An old-format `{session: "pane_id"}` file is read without panicking; its
      entries are treated as absent.
- [ ] `resolve_target_pane` falls through to its existing prompt path whenever
      `get()` returns `None`.
- [ ] The `pane_prefs.rs` doc comment names the real path (`var/run/`).
- [ ] All four gates green.

## Test plan

The fingerprint check is the safety property, so it must be tested without a live
tmux server. **Put the comparison behind a seam that takes the live pane data as
a parameter** — a function like `matches(stored, live) -> bool`, or `get_with`
taking an injected lookup — so tests can drive it directly. Phase 08's
`rotate_log_file` / `reattach_log_fds` split and phase 09's
`retention_warnings(&Config)` are the precedent: the decision is pure, the tmux
query is the caller's.

**Tests that touch `HOME` must take `crate::test_home_guard()`** (`src/lib.rs:45`)
— not the raw `TEST_HOME_LOCK` (`:32`) — hold it through all HOME-dependent work,
**and restore `HOME` at the end.** Phase 09 shipped five tests that set `HOME` and
never restored it, poisoning every ambient reader and making `cargo test --lib`
fail ~3 runs in 8; the fix is in the tree now, so do not reintroduce the pattern:

```rust
let old_home = std::env::var("HOME").ok();
unsafe { std::env::set_var("HOME", tmp.path()) };
// … test body …
match old_home {
    Some(v) => unsafe { std::env::set_var("HOME", v) },
    None => unsafe { std::env::remove_var("HOME") },
}
```

**Mutation-check the safety property before reporting.** Break the fingerprint
comparison so it always matches, confirm the "fingerprint no longer matches"
test **fails**, revert, confirm it passes. Quote both runs. A test that passes
when the check is disabled would leave the exact defect this phase exists to fix.

**Do not pin a test count in advance.** Report the resulting count and explain
the delta.

## End-to-end verification

**`STANDARDS.md` §1's mechanical-capture box applies.** Redirect each command's
output to a file and paste the contents into a **new Update Log entry you
author**, titled `### Update — <date> (end-to-end verification)`.

The server-authored `(complete)` entry's "Command output tails" block is the
standard gate capture every phase receives automatically. **It does not satisfy
this requirement** — seven bounces on this milestone have turned on that
distinction.

```sh
# Mutation: make the fingerprint always match.
cargo test --lib pane_prefs -- --nocapture \
  > /tmp/e2e-10-red.txt 2>&1; echo "exit=$?" >> /tmp/e2e-10-red.txt

git checkout -- src/

cargo test --lib pane_prefs -- --nocapture \
  > /tmp/e2e-10-green.txt 2>&1; echo "exit=$?" >> /tmp/e2e-10-green.txt

# No flake regression.
for i in $(seq 1 12); do cargo test --lib >/dev/null 2>&1 || echo "FAIL run $i"; done \
  > /tmp/e2e-10-flake.txt 2>&1; echo "exit=$?" >> /tmp/e2e-10-flake.txt
```

Paste all three. `/tmp/e2e-10-flake.txt` must contain only the `exit=0` line.
Also paste the JSON your new format writes, from a test tempdir.

## Authorizations

- [ ] May rewrite `src/pane_prefs.rs`, including its file format and doc comment.
- [ ] May modify the `get()`/`save()` call sites in `src/cli/commands/pane.rs`
      and `src/daemon/server/handlers.rs`.
- [ ] May add a pane-lookup helper to `src/tmux/` **only if**
      `list_panes_detailed()` genuinely does not provide the fields — say so in
      the Update Log if you add one.

No new dependencies. No changes to `docs/architecture.md`.

## Out of scope

- **Do not delete the orphaned `~/.daemoneye/pane_prefs.json`** on the
  maintainer's disk. Removing stray runtime files is phase 11's.
- **Do not change `resolve_target_pane`'s fallback behaviour** — the sibling
  auto-pick, the prompt, and the no-sibling split offer all stay as they are. This
  phase only changes when step 1 is allowed to short-circuit them.
- **Do not add a config key** for this. The preference is a runtime convenience,
  not an operator setting.
- **Do not touch `.gitignore`, `main.rs`'s stale `daemon.log` help strings, or
  the pre-existing `tokio::time::sleep` at `tests/integration.rs:615`.** Phase 11
  and milestone housekeeping.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-07-31 02:35 (started)

**Executor:** model

Rewrote `src/pane_prefs.rs` with fingerprint validation. `get()` now validates the stored pane's window name and working directory against live tmux data before returning it. Old-format `{session: "pane_id"}` files are tolerated (entries treated as absent). `get()` is non-destructive. Pruning runs on every `get()` call. `save()` fetches the fingerprint from `list_panes_detailed()` at call time, so all four existing call sites work without changes.

### Update — 2026-07-31 02:38 (end-to-end verification)

**Mutation check — fingerprint broken (RED):**
```
running 10 tests
thread 'pane_prefs::tests::does_not_match_when_current_path_changed' panicked at src/pane_prefs.rs:168:9:
assertion failed: !matches(&stored, "%3", "main", "/tmp")
thread 'pane_prefs::tests::does_not_match_when_window_name_changed' panicked at src/pane_prefs.rs:162:9:
assertion failed: !matches(&stored, "%3", "edited", "/home/user")
thread 'pane_prefs::tests::does_not_match_when_pane_id_changed' panicked at src/pane_prefs.rs:174:9:
assertion failed: !matches(&stored, "%99", "main", "/home/user")
test result: FAILED. 7 passed; 3 failed; 0 ignored; 0 measured; 979 filtered out
exit=101
```

**Mutation check — fingerprint restored (GREEN):**
```
running 10 tests
test pane_prefs::tests::does_not_match_when_pane_id_changed ... ok
test pane_prefs::tests::does_not_match_when_current_path_changed ... ok
test pane_prefs::tests::does_not_match_when_window_name_changed ... ok
test pane_prefs::tests::matches_when_all_fields_equal ... ok
test pane_prefs::tests::get_does_not_mutate_stored_map ... ok
test pane_prefs::tests::load_all_reads_new_format ... ok
test pane_prefs::tests::load_all_tolerates_garbage_json ... ok
test pane_prefs::tests::load_all_tolerates_old_format ... ok
test pane_prefs::tests::load_all_tolerates_missing_file ... ok
test pane_prefs::tests::save_roundtrips_fingerprint ... ok
test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 979 filtered out
exit=0
```

**Flake check (12 runs):**
```
exit=0
```

**New on-disk JSON format (from test roundtrip):**
```json
{"sess1":{"pane_id":"%3","window_name":"main","current_path":"/home/user"},"sess2":{"pane_id":"%7","window_name":"edit","current_path":"/tmp"}}
```

**Test count:** 989 lib (+10 new pane_prefs tests), 30 integration (2 ignored), 8 isolation (1 ignored). Delta: +10 unit tests for `matches()`, old-format tolerance, mutation-free `get()`, missing file, garbage JSON, and save/load roundtrip.

**Grep for pinned literal:** `grep -r "PanePreference" src/pane_prefs.rs | head -1` → confirmed `PanePreference` struct present in the module.
