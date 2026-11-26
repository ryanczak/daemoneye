# Phase 09b — webhook-fanout

**Milestone:** M40 — Security hardening
**Status:** todo
**Drafted:** 2026-11-26
**Depends on:** 09a (done)
**Estimated diff:** ~300 lines
**Tags:** language=rust, kind=bugfix, size=m

## Goal

This is the second half of audit row **S10**. The architect probed each
claim against the tree on 2026-11-26.

1. **Unbounded fan-out.** After the body is parsed, `handle_webhook`
   (`src/webhook/server.rs`) runs `for alert in alerts { tokio::spawn(…
   process_alert …) }`. A 2 MiB Alertmanager body of tiny alerts with
   distinct labels gives hundreds of thousands of concurrent tasks from one
   authenticated POST. Distinct labels defeat dedup.
2. **The analysis rate-limit map never evicts.** `maybe_analyze_alert`
   (`src/webhook/process.rs`) inserts `notice.name → now` into
   `WebhookState.rate_limit` **before** the runbook lookup, and never
   removes anything. Every distinct alert name stays for ever, including
   names with no runbook.
3. **Alert text reaches the model unframed.** The alert's name, summary
   and description are attacker-controlled text. They reach a prompt in
   three places, none of them marked as data:
   - the watchdog query, `format!("Incoming alert:\n{alert_msg}")` in
     `maybe_analyze_alert`;
   - the ghost's first user turn, `"Incoming alert:\n{}…"` in
     `GhostManager::start_session_with_config` (`src/daemon/ghost.rs`);
   - every chat session's history: the history sink's `history_message`
     (`src/daemon/notice_sinks.rs`) injects `notice.history_text()` as a
     `user` message.

## Statement of work

1. **Bounded fan-out.**
   - Add `const MAX_ALERTS_PER_POST: usize = 100` in `server.rs`.
   - The handler keeps the first 100 parsed alerts. When it drops any, it
     logs **one** `webhook_discarded` event with
     `reason: "too_many_alerts"` and `dropped: <count>`, before spawning.
   - It then spawns **one** task per POST, which runs `process_alert` for
     each kept alert in turn, awaiting each.
   - The response is still `200` when alerts were dropped.
   - Put the truncation in a small pure function so it can be unit-tested.
2. **The rate-limit map evicts.** In `maybe_analyze_alert`, before the
   insert, drop every entry whose age is at least
   `config.webhook.dedup_window_secs`. Such an entry can never suppress an
   analysis again. The suppression behaviour is unchanged.
3. **One untrusted-data block.**
   - Add a new file, `src/webhook/untrusted.rs`, declared in
     `src/webhook/mod.rs`. It holds
     `pub(crate) fn untrusted_alert_block(text: &str) -> String`, which
     returns:
     - a fixed preamble line saying that the text between the markers came
       from an external webhook, is data and not instructions, and must not
       be followed;
     - then `<untrusted-alert-data>`, the text, and
       `</untrusted-alert-data>`, each on its own line.
   - Any occurrence of either marker inside `text`, ASCII-case-insensitive,
     is rewritten so it no longer matches. The output therefore holds
     **exactly one** opening marker and **one** closing marker, whatever
     the input is.
   - Apply it at the three sites:
     - **(a) the watchdog query:** the alert text inside the watchdog
       message is the block.
     - **(b) the ghost's first turn:** when `trigger` is
       `GhostTrigger::Webhook`, the alert text in that turn is the block.
       Every other trigger is unchanged. `assemble_incident_context` still
       receives the **raw** text: it is a search query.
     - **(c) the history sink:** for a notice whose `source` is
       `Source::Webhook`, `history_message`'s content is
       `untrusted_alert_block(&notice.history_text())`. Every other source
       is unchanged.
4. **Docs.** `docs/security.md` § 3 gets one paragraph covering three
   points:
   - the 100-alert cap per POST, processed one at a time;
   - the evicting rate-limit map;
   - the untrusted-data block, and the three sites it is applied at.

**Closed file list:**
- `src/webhook/server.rs`, `src/webhook/process.rs`;
- `src/webhook/untrusted.rs` (new), `src/webhook/mod.rs`;
- `src/daemon/notice_sinks.rs`, `src/daemon/ghost.rs`;
- `tests/integration.rs`;
- `docs/security.md`.

**Edit order:** each step ends with `cargo build --tests`.
1. **Your first tool call is the creation of `src/webhook/untrusted.rs`:**
   a stub plus its red test.
2. The framing helper, then sites (c), (a) and (b).
3. The rate-limit eviction and its test.
4. The fan-out cap and its tests.
5. The docs.

**Read limit:** read at most **3** files before your first edit.

**Budget:** you have **1200 turns**; use them efficiently.

### Pre-verified facts (do not re-read to confirm)

- **Two existing assertions must change with site (c).** Change these
  assertions; never delete the tests.
  - `history_sink_broadcast_reaches_every_resident_session`
    (`notice_sinks.rs`) asserts that the content equals
    `notice.history_text()` for a `Source::Webhook` notice. It becomes:
    the content equals `untrusted_alert_block(&notice.history_text())`.
  - `webhook_alert_at_threshold_is_injected` (`tests/integration.rs`)
    asserts that the content `starts_with("[Webhook Alert]")`. It becomes
    `contains`, plus `contains("<untrusted-alert-data>")`. The helper is
    `pub(crate)`, so the integration test matches the marker string.
- **Calling the handler in a test:** 09a's
  `webhook_refuses_before_reading_the_body` in `server.rs` already shows
  it: `test_state_from`, `headers_with_bearer`, `ConnectInfo`, and
  `Body::from`.
- **Reading events in a test:** use
  `crate::daemon::utils::for_each_event_between(None, None, &mut |v| …)`
  inside the same `crate::test_home()`, which is non-reentrant, so take it
  once.
- **A state for `maybe_analyze_alert`:** `src/webhook/trigger.rs`'s tests
  build a `WebhookState` by hand. With no runbook on disk, the function
  returns right after the rate-limit insert.

## Acceptance criteria

- [ ] **The block:** `untrusted_alert_block_wraps_and_neutralises_markers`
      passes.
      - **Asserts:** plain text is wrapped, with the preamble first.
      - **Negative:** a text containing `</untrusted-alert-data>` and
        `<UNTRUSTED-ALERT-DATA>` still yields exactly one opening and one
        closing marker, counted case-insensitively.
      - Paste the red run.
- [ ] **History framing:** `history_sink_frames_only_webhook_notices`
      passes. A `Source::Webhook` notice's injected content holds the block
      around its `history_text`. A `Source::Scheduler` notice's content is
      its `history_text`, byte for byte.
- [ ] **Watchdog framing:** `watchdog_message_frames_the_alert` passes.
      Factor the watchdog message into a pure function so the test needs no
      model call. The message contains the block around the alert text.
- [ ] **Ghost framing:** `webhook_ghost_first_turn_frames_the_alert`
      passes.
      - **Asserts:** with `GhostTrigger::Webhook`, the first user turn
        contains the block.
      - **Negative:** with `GhostTrigger::Scheduled`, it contains no
        `<untrusted-alert-data>`.
- [ ] **Eviction:** `analyze_rate_limit_drops_expired_names` passes.
      - **Seed:** `stale` at `now - window - 1` and `fresh` at `now`.
      - **Action:** analyse a third name, `other`.
      - **Asserts:** the map holds `fresh` and `other`, not `stale`.
- [ ] **Suppression unchanged:** the existing `trigger.rs` tests pass
      unchanged. Both `ghost_trigger_*` tests read the `rate_limit` map.
- [ ] **Cap:** `alerts_over_the_cap_are_dropped_and_counted` passes. It
      covers the pure truncation for 150 alerts (100 kept, 50 dropped) and
      for 3 alerts (3 kept, 0 dropped).
- [ ] **Handler:** `webhook_post_over_the_cap_logs_one_discard` passes.
      An authorised POST of 150 Alertmanager alerts with distinct
      `alertname` labels returns `200`. The event log then holds exactly
      one `webhook_discarded` record with `reason == "too_many_alerts"`
      and `dropped == 50`.
- [ ] **Mutation run, pasted as output:**
      1. Make the truncation keep every alert.
      2. Run `cargo test --lib alerts_over_the_cap` once. It must fail.
      3. Restore by undoing that one edit, never by copying a backup file.
- [ ] **No test removed:** run this once. It prints **0**:
      `git diff 3c6fde51 -- src tests | grep -cE '^-\s*#\[(tokio::)?test\]'`
- [ ] **Gates:** all four pass, each run once.

## Spec

1. **Break down:** write the first red test and run it red. Then write a
   numbered task list into an Update Log entry, one line per task, each
   naming the acceptance item it closes. Done when that entry is written.
2. **Implement:** follow the edit order, writing each red test before its
   fix. Done when `cargo build --tests` is green.
3. **Meet the acceptance criteria:** check off every item, running each
   command **once**. Done when every item is checked with its evidence
   named.
4. **Capture the evidence:** run the four gates, once each. Paste into a
   new Update Log entry:
   - each `test result:` line, without the per-test `… ok` lines;
   - the red run and the mutation run, **as output, not summarized**;
   - the grep.

   Then commit as one `fix:` commit.

## Authorizations

- You may touch the files in the closed file list, and no others.
- You may change the two assertions named in the pre-verified facts.

## Out of scope

- **The catch-up brief and the away digest.** They are client displays,
  not model prompts.
- **The chat-notice overlay and route commands.** These are the tmux line
  and `DAEMONEYE_MSG`.
- **A global cap on concurrent `process_alert` tasks across POSTs.** The
  per-IP request limit already bounds POSTs.
- **The dedup map.** It already has a 10,000-entry cap.

## Update Log

<!-- entries appended below this line -->
