# Phase 05: residency — a session is resident from the moment a chat attaches

**Milestone:** M29 — Post-2.0 fixes
**Status:** in-progress
**Depends on:** phase-04
**Estimated diff:** ~350 lines, about half of it test rewrites
**Tags:** language=rust, kind=fix, size=m

## Goal

The daemon has three views of "a session": the resident `SessionStore`,
the presence registry (`CHAT_PRESENCE`), and the disk history plus
`NON_RESIDENT_DETACHES`. A session becomes resident only on its first
`Ask` after a daemon start, so a reconnected chat that has not asked yet
is known to presence but not to the store. Each M27 fix reconciled one
pair of views (F5, F8, F11, bug-07e-1), and the gap kept returning.

**PE decision:** a presence attach makes the session resident, loading
its history and meta from disk. One store then serves injection, the
catch-up brief and `handle_ask`. `NON_RESIDENT_DETACHES` is deleted; its
job is done by the entry's own `last_detach` / `detach_time_utc` /
`messages_at_detach`, which `record_chat_detach` already stamps on an
entry that exists.

## Spec

**Files:** `src/daemon/session.rs`, `src/daemon/server/ask.rs`,
`src/daemon/server/chat_presence.rs`, `src/webhook/process.rs`.

1. **One constructor for a resident entry.** Add
   `impl SessionEntry { pub fn from_disk(id: &str) -> SessionEntry }` in
   `session.rs`. It builds the entry the way `handle_ask`'s
   `or_insert_with` closure does today, restoring `started_at`,
   `turn_count`, `last_prompt_tokens`, `token_scale`,
   `tool_calls_this_session` and `saved_name` from `read_session_meta`,
   **and** sets `messages` to `read_session_file(id, None)`. A missing
   file or meta gives empty history and defaults, as now.
   `handle_ask`'s closure becomes a call to it, so the two creation sites
   cannot drift.
2. **Attach makes the session resident.** In `handle_chat_attach`, after
   `presence_register` and before `attach_brief_for`:
   `with_sessions(&sessions, |store| { store.entry(id).or_insert_with(|| SessionEntry::from_disk(&id)); })`.
   An entry that already exists is untouched, including its detach
   fields.
3. **The brief has one branch.** `attach_brief_for` keeps its resident
   branch and loses the non-resident fallback. It no longer calls
   `take_non_resident_detach`.
4. **Delete the registry.** Remove `NON_RESIDENT_DETACHES`,
   `NonResidentDetach`, `non_resident_detach_map`,
   `record_non_resident_detach`, `take_non_resident_detach` and
   `non_resident_detach_ids` from `session.rs`; the
   `record_non_resident_detach` call in `record_chat_detach`; and the
   `ids.extend(non_resident_detach_ids())` line in
   `inject_into_sessions`. The rest of `inject_into_sessions` is
   unchanged: store keys plus presence ids, disk append for every id,
   in-memory push for resident entries.
5. **Tests that encoded the old model.** The test run names them. The
   expected changes, and only these:
   - `attach_brief_creates_no_store_entry_for_a_non_resident_session`
     (`chat_presence.rs`) inverts. Rename it
     `attach_makes_the_session_resident_with_its_disk_history`: after
     attach, the entry exists and its `messages` equal the disk file.
   - `catchup_brief_reaches_a_session_that_is_not_resident` and
     `catchup_brief_names_an_alert_posted_after_the_chat_left`
     (`chat_presence.rs`) back-date the entry's `last_detach` and
     `detach_time_utc` instead of a registry record, the way
     `reattach_after_30s_returns_catchup_brief` does, and drop their
     "must not make the session resident" assertions.
   - `record_chat_detach_stamps_only_when_the_entry_exists`
     (`session.rs`) keeps its assertion that an unknown id creates no
     entry; `record_chat_detach` still creates nothing.
   - `inject_reaches_a_presence_attached_session_that_is_not_resident`
     (`process.rs`) registers presence directly, never attaches, and is
     **unchanged**.
6. **Capture the end-to-end evidence.** Run `cargo fmt --all` first. Then
   run § End-to-end verification verbatim, **once**. Paste the **output**
   (`/tmp/e2e-m29-05.txt`) as the **first** fenced block of a new Update
   Log entry headed `### Update — <date> (end-to-end verification)`, then
   the `PASTE MATCH` line. Do not paste the command block.

**Gotchas.**
- `with_sessions` asserts if called re-entrantly. Build the entry
  (`from_disk` reads disk) **outside** or inside a single
  `with_sessions` closure, never by nesting two.
- `handle_ask` reads a resident entry's `messages` instead of disk. With
  task 1 that is correct, because the entry now carries the disk history.
  Do not add a second disk read there.
- Test fixtures: `with_test_home` for a throwaway `HOME`, `write_session_file`
  / `append_session_message` to seed `<id>.jsonl`, an in-memory duplex to
  drive `handle_chat_attach`, as `reattach_after_30s_returns_catchup_brief`
  does. Hold `test_home_guard` for the whole test.
- `bg_shell.rs` drops a background completion for a session the store
  does not hold. Leave it: an attached session is now always held.
- Find anchors with `grep -n`; read windows, not single lines.

## Tests

The three sequences from the milestone's exit criteria, in
`chat_presence.rs` tests, each on a fresh `SessionStore` (the restart)
with a seeded disk history:

- `restart_reconnect_alert_ask_sees_the_alert`: attach, then
  `inject_into_sessions` with an alert. The store entry's `messages`
  contain the alert, and so does the disk file. (`handle_ask` reads the
  resident entry's `messages`, so this is what the next ask sees.)
- `restart_reconnect_quit_alert_resume_briefs_the_alert`: attach, close
  the connection (the last one), back-date the entry's `last_detach` by
  60 s, inject an alert, attach again. The brief names the alert.
- `fresh_resume_of_a_never_loaded_session_carries_history_and_meta`:
  seed two messages and a meta with `turn_count = 5`, attach. The entry
  has two messages and `turn_count == 5`.

Plus the renamed `attach_makes_the_session_resident_with_its_disk_history`.

## Acceptance criteria

- [ ] The three sequence tests and the renamed test ⇒
      `test result: ok. 1 passed` each.
- [ ] `grep -rc 'NON_RESIDENT_DETACHES\|non_resident_detach' src | grep -v ':0'`
      prints nothing (end-to-end line `registry_refs=0`).
- [ ] `cargo test` ⇒ about `passed=2425 failed=0` (2422 at `b89c1d03`
      + 4 new, with the renamed test replacing one). An extra test is
      fine: name it and report; do not re-run the suite to chase a number.
- [ ] `cargo test --test doc_truth` ⇒ `failed=0`. Clippy (`-D warnings`)
      and `cargo fmt --all --check` exit 0. No new `#[allow]`.
- [ ] The end-to-end Update Log entry holds the output fence **first**,
      then the `PASTE MATCH` line.

## End-to-end verification

```sh
A=/tmp/e2e-m29-05.txt; : > "$A"
for t in restart_reconnect_alert_ask_sees_the_alert restart_reconnect_quit_alert_resume_briefs_the_alert fresh_resume_of_a_never_loaded_session_carries_history_and_meta attach_makes_the_session_resident_with_its_disk_history catchup_brief_reaches_a_session_that_is_not_resident catchup_brief_names_an_alert_posted_after_the_chat_left inject_reaches_a_presence_attached_session_that_is_not_resident; do cargo test --lib "$t" 2>&1 | grep -E '^test result' | sed "s/^/$t: /; s/[0-9]* filtered out/<n> filtered out/; s/finished in [0-9.]*s/finished in <t>s/" >> "$A"; done
echo "registry_refs=$(grep -rc 'NON_RESIDENT_DETACHES\|non_resident_detach' src | grep -v ':0' | wc -l)" >> "$A"
cargo test --test doc_truth 2>&1 | grep -E '^test result' | sed 's/^/doc_truth: /; s/finished in [0-9.]*s/finished in <t>s/' >> "$A"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1 | sed 's/in [0-9.]*s/in <t>s/' >> "$A"
cargo fmt --all --check; echo "fmt_exit=$?" >> "$A"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}' >> "$A"
cat "$A"
```

Then run the self-check, and put its line in the entry after the output
fence:

```sh
D=docs/dev/milestones/M29-post-2.0-fixes/phase-05-residency.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-m29-05.txt
diff /tmp/pasted-m29-05.txt /tmp/e2e-m29-05.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

## Authorizations

- Renaming and rewriting the four tests named in task 5.

## Out of scope

- Eviction (`cleanup_pass`): an idle resident entry with no presence is
  still evicted after `idle_after`, and its detach state goes with it, as
  before this phase.
- The live checks of the three sequences, which belong to the live close.
- `bg_shell.rs`, `notify_chat_panes` and the historical M27 docs that
  describe the registry.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-10-19 09:53 (progress)

Started. Status flipped to `in-progress`. Plan: add `SessionEntry::from_disk`,
make `handle_ask`'s closure call it, make `handle_chat_attach` insert the entry
after `presence_register`, strip the non-resident branch from
`attach_brief_for`, delete the `NON_RESIDENT_DETACHES` registry and its
`record_chat_detach` / `inject_into_sessions` hooks, then rewrite the four
tests the phase names and add the three sequence tests.

### Update — 2026-10-19 11:23 (end-to-end verification)

The phase's § End-to-end verification, run verbatim and once, `cargo fmt
--all` first. The output was redirected to `/tmp/e2e-m29-05.txt` and pasted
below unedited.

````
restart_reconnect_alert_ask_sees_the_alert: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
restart_reconnect_quit_alert_resume_briefs_the_alert: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
fresh_resume_of_a_never_loaded_session_carries_history_and_meta: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
attach_makes_the_session_resident_with_its_disk_history: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
catchup_brief_reaches_a_session_that_is_not_resident: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
catchup_brief_names_an_alert_posted_after_the_chat_left: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
inject_reaches_a_presence_attached_session_that_is_not_resident: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; <n> filtered out; finished in <t>s
registry_refs=0
doc_truth: test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in <t>s
    Finished `dev` profile [unoptimized + debuginfo] target(s) in <t>s
fmt_exit=0
passed=2425 failed=0
````

PASTE MATCH

- The four sequence tests and the renamed test each print `test result: ok.
  1 passed` — the three new sequences (`restart_reconnect_alert_ask_sees_the_alert`,
  `restart_reconnect_quit_alert_resume_briefs_the_alert`,
  `fresh_resume_of_a_never_loaded_session_carries_history_and_meta`) and
  `attach_makes_the_session_resident_with_its_disk_history`, with the two
  rewritten catch-up tests and the untouched
  `inject_reaches_a_presence_attached_session_that_is_not_resident`.
- `registry_refs=0` — `grep -rc 'NON_RESIDENT_DETACHES\|non_resident_detach' src
  | grep -v ':0'` printed nothing.
- `doc_truth: 16 passed; 0 failed`, clippy `-D warnings` finished clean,
  `fmt_exit=0`, full suite `passed=2425 failed=0`.
