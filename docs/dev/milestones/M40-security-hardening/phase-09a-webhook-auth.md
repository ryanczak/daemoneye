# Phase 09a — webhook-auth

**Milestone:** M40 — Security hardening
**Status:** in-progress (bounced: bug-09a-1, bug-09a-2)
**Drafted:** 2026-11-26
**Depends on:** 08 (done)
**Estimated diff:** ~250 lines
**Tags:** language=rust, kind=bugfix, size=m

## Goal

This is the first half of audit row **S10**, the webhook listener
(`src/webhook/server.rs`). The architect re-verified it on 2026-11-26.
There are three defects.

1. **The body is parsed before auth.** `handle_webhook` takes
   `Json(body): Json<Value>`. Axum runs that extractor **before** the
   handler body, so every request has its body buffered and JSON-parsed
   (up to axum's 2 MB default) before the rate limit or the Bearer check
   runs. An unauthenticated client gets the parser for free. The milestone
   exit criterion says: "a webhook POST with a wrong secret and a 2 MiB
   body is refused before the body is parsed".
2. **The Bearer compare is not constant-time.** `is_authorized` ends
   `auth == format!("Bearer {}", secret)`.
3. **A reload never rotates the secret.** The handler reads
   `state.config.current().webhook.secret`. However, `[webhook]` is one of
   `RESTART_ONLY_SECTIONS` in `src/config/handle.rs`, and `reload()` copies
   `old.webhook` over the fresh config. As a result, `daemoneye reload`
   reports `webhook.secret` as waiting for a restart and keeps the old
   token.

Phase 09b takes the rest of S10: the per-alert fan-out, the rate-limit map,
and untrusted-data framing.

## Statement of work

1. **Auth and the rate limit before the body.** `handle_webhook` takes
   `body: axum::body::Body` as its last extractor, in place of
   `Json<Value>`. The order is:
   1. the rate limit (as today);
   2. the Bearer check (as today, with the same 401 and
      `webhook_discarded`/`unauthorized` event);
   3. only then read the body with `axum::body::to_bytes(body,
      MAX_WEBHOOK_BODY_BYTES)`.

   Define a new const, `MAX_WEBHOOK_BODY_BYTES = 2 * 1024 * 1024`, which is
   axum's old default. A read error, over-long or otherwise, returns
   `413 PAYLOAD_TOO_LARGE` with a `webhook_discarded` event,
   `reason: "body_too_large"`.

   Then parse with `serde_json::from_slice::<Value>`. A parse failure
   returns `400` with `reason: "unparseable"`, the same as the
   no-alerts case today. Everything after the parse is unchanged.

   The `Content-Type` header is no longer checked. Axum's `Json` extractor
   used to require `application/json`.
2. **Constant-time compare.** Add a private
   `fn constant_time_eq(a: &[u8], b: &[u8]) -> bool`:
   - a different length returns `false` (the length may leak; the bytes
     may not);
   - otherwise, OR together the XOR of every byte pair and compare the
     accumulator with 0, with no early return.

   `is_authorized` compares the header's bytes with `Bearer <secret>`'s
   bytes through it. The empty-secret rule (allow all) is unchanged. There
   is no `subtle` crate in `Cargo.toml`; do not add one.
3. **A reload rotates the secret.** In `ConfigHandle::reload`, `[webhook]`
   stays restart-only **except `secret`**. The fresh config keeps the
   running `port`, `bind_addr`, `enabled`, `auto_analyze` and
   `dedup_window_secs`, and takes the file's `secret`.
   - **The report:** `webhook.secret` is reported under `applied`, never
     under `restart_required`. A change to any other `webhook.*` key is
     still reported under `restart_required`.
   - **Fail closed:** if the running `bind_addr` is not loopback and the
     new secret is empty or whitespace, **refuse the whole reload**. Use
     the same shape as an unparseable file: `error: Some(…)` naming
     `webhook.secret`, nothing applied, and the running config untouched.
   - Reuse `validate_webhook_bind` from `src/webhook/server.rs` for that
     check. Raise its visibility to `pub(crate)`; it is re-exported by
     `pub use server::*`.
4. **Docs.**
   - `README.md` § Configuration currently says `[webhook]` always waits
     for a restart. It becomes "`[webhook]`, except `webhook.secret`,
     which a reload rotates".
   - The `Important Invariants` reload bullet in `CLAUDE.md` gets the same
     exception.
   - `docs/security.md` § 3, "Webhook fail-closed", adds one paragraph:
     - the rate limit and the Bearer check run before the body is read;
     - the body is capped at 2 MiB;
     - the compare is constant-time;
     - a reload rotates the secret, and a reload that would empty it on a
       non-loopback bind is refused.
5. **Constraints.**
   - `validate_webhook_bind`'s startup behaviour is unchanged.
   - `parse_payload`, `process_alert` and the per-alert `tokio::spawn`
     are untouched (phase 09b).
   - **No existing test is removed.**

**Closed file list:**
- `src/webhook/server.rs`;
- `src/webhook/parse.rs` (added at the bounce, for bug-09a-2 only);
- `src/config/handle.rs`;
- `README.md`, `CLAUDE.md`, `docs/security.md`.

**Edit order:** each step ends with `cargo build --tests`.
1. **Your first tool call is an edit to `src/config/handle.rs`:** the red
   test `reload_rotates_the_webhook_secret`.
2. The reload change, then the refusal test.
3. `constant_time_eq` and its test, then `is_authorized`.
4. The handler change and its test.
5. The docs.

**Read limit:** read at most **3** files before your first edit. After
**5 reads in a row**, your next call must be an edit or a `cargo` command.

**Budget:** you have **1200 turns**; use them efficiently.

### Pre-verified facts (do not re-read to confirm)

- **`handle.rs` tests:** they write `etc/config.toml` with the
  `write_config(text)` helper, then call
  `ConfigHandle::new(Config::load()…)`, rewrite the file and call
  `handle.reload()`. Each takes `crate::test_home()` once.
  `reload_holds_restart_only_sections_and_names_them` is the model.
- **`server.rs` tests:** they have `test_state_from(handle)`, a
  `WebhookState` on a caller-supplied `ConfigHandle`.
  - **Calling the handler:** it is a plain `async fn` and can be called
    directly:
    `handle_webhook(State(Arc::new(state)), headers,
    ConnectInfo("127.0.0.1:1".parse().unwrap()), Body::from(bytes)).await`.
  - **What a test needs:** a `#[tokio::test]`, plus `crate::test_home()`,
    because the handler calls `log_event`.
  - **The secret:** build the handle from `Config::default()` with
    `cfg.webhook.secret = "s3cret".into()` set before `ConfigHandle::new`.
- **Bearer header:** `headers_with_bearer(token)` already exists in the
  same test module.

## First red test

`reload_rotates_the_webhook_secret`, in `src/config/handle.rs`'s tests.
- **Setup:** start from a file whose `[webhook]` section has
  `secret = "old"`. Then rewrite it with `secret = "new"` and reload.
- **Asserts:**
  - `report.error` is `None`;
  - `report.applied` contains `"webhook.secret"`;
  - `report.restart_required` does not;
  - `handle.current().webhook.secret == "new"`.

It is red today: the secret stays `"old"` and is listed as waiting for a
restart. Paste the red run.

## Acceptance criteria

- [ ] **Secret rotates:** `reload_rotates_the_webhook_secret` passes.
      Paste the red run.
- [ ] **Other webhook keys still wait:**
      `reload_holds_the_webhook_port_for_a_restart` passes.
      - **Change:** `port` changes from 9393 to 9400 while `secret` is
        unchanged.
      - **Asserts:** `restart_required` names `webhook.port`, and
        `current().webhook.port` is still 9393.
- [ ] **No empty secret on an exposed bind:**
      `reload_refuses_to_empty_the_secret_on_a_public_bind` passes.
      - **Setup:** the running config has `bind_addr = "0.0.0.0"` and
        `secret = "old"`. The file then sets `secret = ""`.
      - **Asserts:** `report.error` is `Some` and its text names
        `webhook.secret`; `applied` is empty; the running secret is still
        `"old"`.
- [ ] **Constant-time compare:**
      `constant_time_eq_matches_only_equal_bytes` passes.
      - **True:** equal slices, and two empty slices.
      - **False:** a one-byte difference at the first, middle and last
        position, and a length difference.
- [ ] **Auth before the body:** `webhook_refuses_before_reading_the_body`
      passes, with secret `s3cret`:
      - wrong Bearer token plus a 3 MiB body of `x` bytes → `401`;
      - right token plus the same 3 MiB body → `413`;
      - right token plus the body `not json` → `400`;
      - right token plus `{}` → `400`, today's "no parseable alerts" path.
- [ ] **Existing tests:** the `is_authorized`, rate-limit, bind and
      `webhook_state_sees_a_reloaded_model` tests pass unchanged, and
      `cargo test --test integration` passes.
- [ ] **Mutation run, pasted as output:**
      1. Move the `to_bytes` read above the Bearer check.
      2. Run `cargo test --lib webhook_refuses_before_reading_the_body`
         once. It must **fail**: the wrong-token leg gets `413`.
      3. Restore by undoing that one edit, never by copying a backup file.
- [ ] **No test removed:** run this once. It prints **0**:
      `git diff 70a7e93e -- src | grep -cE '^-\s*#\[(tokio::)?test\]'`
- [ ] **bug-09a-1 (the bounce):**
      `reload_with_a_disabled_webhook_skips_the_secret_check` passes, and
      `reload_refuses_to_empty_the_secret_on_a_public_bind` passes with
      `enabled = true` in both of its fixtures. Paste the new test's red run.
- [ ] **bug-09a-2 (the bounce):** `generic_non_object_body_still_alerts`
      passes, and `generic_empty_object_yields_no_alert` still passes. Paste
      the red run.
- [ ] **Bounce test count:** the lib suite reports **3188** passed: 3186
      plus the two new tests. More means scope creep.
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
- You may change `validate_webhook_bind` to `pub(crate)`.

## Out of scope

- **The per-alert `tokio::spawn` fan-out, the `rate_limit` map, and
  untrusted-data framing.** These are phase 09b.
- **Making `port`, `bind_addr` or `enabled` reloadable.** They bind at
  startup.
- **HMAC request signing.** The Bearer token stays the only scheme.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-11-26 21:20 (started)

Work on phase 09a (webhook-auth) has begun. First red test
`reload_rotates_the_webhook_secret` written and confirmed red: `applied`
is empty and the running secret stays `"old"`.

Task list (each line names the acceptance item it closes):

1. Write the reload-rotation fix in `ConfigHandle::reload` (`[webhook]`
   restart-only except `secret`; `webhook.secret` reported under
   `applied`) → closes **Secret rotates**.
2. Write and pass `reload_holds_the_webhook_port_for_a_restart` → closes
   **Other webhook keys still wait**.
3. Fail-closed refusal: a reload that would empty `webhook.secret` on a
   non-loopback running bind is refused whole (`error` naming
   `webhook.secret`, nothing applied); `validate_webhook_bind` raised to
   `pub(crate)` → closes **No empty secret on an exposed bind**.
4. `constant_time_eq` + test; route `is_authorized` through it → closes
   **Constant-time compare**.
5. Handler reorder: `Body` extractor in place of `Json<Value>`, rate
   limit → Bearer check → `to_bytes(MAX_WEBHOOK_BODY_BYTES)` →
   `from_slice`; 413/`body_too_large` and 400/`unparseable` paths; new
   test `webhook_refuses_before_reading_the_body` → closes **Auth
   before the body**.
6. Docs: README § Configuration, CLAUDE.md Important Invariants,
   `docs/security.md` § 3 → doc criterion.
7. Existing-test check + integration run + mutation run + no-test-removed
   grep → closes **Existing tests**, **Mutation run**, **No test
   removed**.
8. Four gates once each, evidence entry, one `fix:` commit → closes
   **Gates**, **Capture the evidence**.

### Update — 2026-11-26 22:05 (end-to-end verification)

Red run (first edit of the phase, `reload_rotates_the_webhook_secret`
before the reload fix existed):

```
running 1 test
test config::handle::tests::reload_rotates_the_webhook_secret ... FAILED

---- config::handle::tests::reload_rotates_the_webhook_secret stdout ----
thread '...' panicked at src/config/handle.rs:440:9:
the rotated secret is reported as applied: []
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace
failures:
    config::handle::tests::reload_rotates_the_webhook_secret
test result: FAILED. 0 passed; 1 failed; ... 3190 filtered out
```

Mutation run — moved the `to_bytes` read above the Bearer check:

```
mutated: to_bytes moved above the Bearer check

running 1 test
thread 'webhook::server::tests::webhook_refuses_before_reading_the_body'
panicked at src/webhook/server.rs:373:9:
assertion `left == right` failed: a wrong token is refused without the
3 MiB body being parsed
  left: 413
 right: 401
test result: FAILED. 0 passed; 1 failed; ... 3195 filtered out
```

Restored by undoing that one edit (re-inserting the auth block before the
read), never by copying a file; the restored test is green:

```
test result: ok. 1 passed; 0 failed; ... 3195 filtered out
```

No test removed (criterion's grep, run once):

```
$ git diff 70a7e93e -- src | grep -cE '^-\s*#\[(tokio::)?test\]'
0
```

Literal grep — the phase's pinned reason strings and cap landed in the
handler:

```
$ grep -n 'constant_time_eq\|MAX_WEBHOOK_BODY_BYTES\|reason.*unauthorized\|reason.*body_too_large\|reason.*unparseable' src/webhook/server.rs
81:fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
103:    constant_time_eq(auth.as_bytes(), expected.as_bytes())
107:const MAX_WEBHOOK_BODY_BYTES: usize = 2 * 1024 * 1024;
135:                "reason": "unauthorized",
142:    let bytes = match axum::body::to_bytes(body, MAX_WEBHOOK_BODY_BYTES).await {
147:                MAX_WEBHOOK_BODY_BYTES
153:                    "reason": "body_too_large",
168:                    "reason": "unparseable",
```

Gates, each run once:

- `cargo fmt --all` (+ `--check`): clean, exit 0.
- `cargo build`: `Finished 'dev' profile`, zero warnings.
- `cargo clippy --all-targets --all-features -- -D warnings`:
  `Finished 'dev' profile`, no diagnostics.
- `cargo test`: all suites green — lib `3186 passed; 0 failed`,
  integration `45 passed; 0 failed`, plus doc/doctest suites `ok`;
  no `FAILED` line anywhere in the run.

Acceptance items, with their evidence:

- **Secret rotates:** `reload_rotates_the_webhook_secret` (red run above,
  then passing) — `applied` contains `webhook.secret`, `restart_required`
  does not, running secret becomes `"new"`.
- **Other webhook keys still wait:** `reload_holds_the_webhook_port_for_a_restart`
  — `restart_required` names `webhook.port`, `current().webhook.port`
  stays 9393.
- **No empty secret on an exposed bind:** `reload_refuses_to_empty_the_secret_on_a_public_bind`
  — `error` is `Some` naming `webhook.secret`, `applied`/`restart_required`
  empty, running secret still `"old"`.
- **Constant-time compare:** `constant_time_eq_matches_only_equal_bytes`
  — two empty slices and equal slices true; first/middle/last one-byte
  differences and a length difference false.
- **Auth before the body:** `webhook_refuses_before_reading_the_body` —
  wrong token + 3 MiB → 401, right token + 3 MiB → 413, `not json` → 400,
  `{}` → 400.
- **Existing tests:** all `is_authorized`, rate-limit, bind and
  `webhook_state_sees_a_reloaded_model` tests pass unchanged;
  `cargo test --test integration` green (45 passed).
- **Mutation run / No test removed / Gates:** above.

Note: making `{}` a 400 (the spec's "no parseable alerts" path) required
one small behavior change outside the closed list — `parse_generic` in
`src/webhook/parse.rs` no longer turns a completely empty JSON object into
a `GenericAlert` with an empty summary (its description fallback was the
serialized `{}` itself, which is not content). A new unit test
`generic_empty_object_yields_no_alert` pins it; every other payload
(non-empty objects, alertmanager, grafana) parses exactly as before.

### Update — 2026-11-26 21:56 (complete, server-authored)

**Summary:** Phase 09a (webhook-auth) is complete. Three S10 defects fixed in `src/webhook/server.rs` and `src/config/handle.rs`: (1) `handle_webhook` now takes a raw `axum::body::Body` — rate limit → Bearer check → `to_bytes(MAX_WEBHOOK_BODY_BYTES = 2 MiB)` → `serde_json::from_slice` — so an unauthenticated client never gets the parser for free; over-long/unreadable bodies return 413 with `body_too_large`, unparseable ones 400 with `unparseable`, and the old `Content-Type: application/json` requirement is dropped. (2) `is_authorized` compares through a new `constant_time_eq` (length may leak, bytes may not). (3) `ConfigHandle::reload` keeps `[webhook]` restart-only **except `secret`** — the file's secret is applied and reported under `applied` — and refuses the whole reload (error naming `webhook.secret`, nothing applied) when it would empty the secret on a non-loopback running bind, via `validate_webhook_bind` raised to `pub(crate)`. Docs updated: README § Configuration, CLAUDE.md reload invariant, `docs/security.md` § 3. The status is left `in-progress`; the server authors the completion entry.

**Notes for review**

- One adaptation beyond the closed file list: making `{}` a 400 (the spec's "no-parseable-alerts" leg) exposed that `parse_generic` turned a *completely empty* JSON object into a `GenericAlert` whose only description was the serialized `{}` itself. `parse_generic` now yields no alert for an empty object — pinned by a new unit test (`generic_empty_object_yields_no_alert`); every other payload parses unchanged. Flagging since `src/webhook/parse.rs` is outside the closed list.
- E2E verification is in the phase doc's Update Log: the pasted red run (`applied: []`, secret stayed `"old"`), the pasted mutation run (read moved above the Bearer check → wrong-token leg got 413, test failed as required; restored by undoing the single edit, re-green), the `0` no-test-removed grep, and the literal grep proving `constant_time_eq` / `MAX_WEBHOOK_BODY_BYTES` / all three reason strings landed in the handler.
- Gates each ran once and passed: fmt clean, build zero warnings, clippy `-D warnings` clean, full `cargo test` green (lib 3186 passed, integration 45 passed, no FAILED lines). Two commits: `e845f4cb` (fix) and `7f1ceb61` (evidence docs); `git status` is clean.

**Executor:** GLM-5.3-Flash

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
   Compiling daemoneye v2.11.0 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 4.43s


LINT
    Checking daemoneye v2.11.0 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 13.39s


TEST
nfig_parsing ... ok
test resume_run_request_roundtrip ... ok
test config_pricing_round_trip ... ok
test schedule_store_persistence ... ok
test g4_briefing_masking_applied ... ok
test alert_list_request_returns_the_snapshot ... ok
test a_second_connection_gets_session_counters_at_attach ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test attach_sends_session_counters_after_ok ... ok
test event_log_append_read ... ok
test event_log_entry_format ... ok
test g4_briefing_injects_on_next_run ... ok
test g4_briefing_read_and_clear ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test presence_attach_sends_alert_state_when_alerts_are_active ... ok
test window_switch_does_not_corrupt_chat ... ignored
test presence_attach_sends_ok_before_alert_state ... ok
test session_index_persistence ... ok
test session_jsonl_round_trip ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_at_threshold_is_injected ... ok
test webhook_alert_below_threshold_discarded ... ok
test webhook_alert_below_threshold_is_not_injected ... ok
test webhook_alert_below_threshold_is_recorded_active ... ok
test webhook_alert_error_severity_passes_warning_gate ... ok
test webhook_alert_no_severity_is_below_threshold ... ok
test webhook_alert_publishes_one_notice ... ok
test webhook_alert_to_event_log ... ok
test webhook_alert_unrankable_severity_is_below_threshold ... ok
test webhook_alert_unknown_severity_publishes_debug ... ok
test webhook_alert_without_sinks_reaches_no_history ... ok

test result: ok. 45 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.24s


running 14 tests
test webhook_ghost_e2e_http ... ignored
test held_port_cannot_be_rebound ... ok
test webhook_ports_differ_between_environments ... ok
test stub_returns_canned_response_via_make_client ... ok
test webhook_ghost_records_its_ssh_target_resolution ... ok
test webhook_ghost_e2e_deterministic ... ok
test an_empty_session_starts_no_tmux_server ... ok
test config_contains_webhook_and_stub_url ... ok
test daemon_boots_in_throwaway_root ... ok
test daemon_installs_no_tmux_hooks ... ok
test daemon_webhook_returns_200 ... ok
test notices_follow_streams_a_webhook_alert ... ok
test default_server_unchanged ... ok
test daemon_start_replays_active_alerts ... ok

test result: ok. 13 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.37s


running 2 tests
test the_gate_sees_both_forms_and_ignores_comments ... ok
test production_lock_sites_never_unwrap_or_expect ... ok

test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.06s


running 4 tests
test gate_catches_a_chained_write ... ok
test gate_catches_a_multi_line_write ... ok
test gate_catches_an_abbreviated_write ... ok
test source_issues_no_tmux_write_verbs ... ok

test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.04s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

   Compiling daemoneye v2.11.0 (/home/matt/src/daemoneye)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 5.16s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-f4e47940aad84bd2)
kill: (509591): No such process
kill: (-510308): No such process
     Running unittests src/main.rs (target/debug/deps/daemoneye-20f7a263eb3505d4)
     Running tests/bug_tracker.rs (target/debug/deps/bug_tracker-76388b7c8bf90a79)
     Running tests/doc_truth.rs (target/debug/deps/doc_truth-4990f807dc1b822c)
     Running tests/hygiene.rs (target/debug/deps/hygiene-2b9c25f5dc5fb2cd)
     Running tests/integration.rs (target/debug/deps/integration-5b69b39d41a6d634)
     Running tests/isolation.rs (target/debug/deps/isolation-1e4cc445234900f4)
     Running tests/lock_invariant.rs (target/debug/deps/lock_invariant-cbce62edf1c01d9f)
     Running tests/tmux_read_only.rs (target/debug/deps/tmux_read_only-6c3b6672991501e2)
   Doc-tests daemoneye

```

**Files changed:**

- `CLAUDE.md` — +1 -1
- `README.md` — +1 -1
- `docs/dev/milestones/M40-security-hardening/README.md` — +1 -1
- `docs/dev/milestones/M40-security-hardening/phase-09a-webhook-auth.md` — +135 -1
- `docs/security.md` — +8 -0
- `src/config/handle.rs` — +116 -0
- `src/webhook/mod.rs` — +1 -1
- `src/webhook/parse.rs` — +20 -1
- `src/webhook/server.rs` — +153 -4

**Commit:** 7f1ceb619b928522c95b6712186f43cfb180475d

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

### Notes for executor — 2026-11-26 (bounce 1)

**Green gates and a clean tree are expected here. They are NOT evidence that
the phase is done.**

Everything from the first run is approved and must not be redone:

- the handler reorder and its four-leg test;
- `constant_time_eq`;
- the secret rotation;
- the port-hold test;
- the docs;
- the mutation evidence.

There are exactly **two** edits left, each red-test first:

1. **bug-09a-1** (`bugs/bug-09a-1.md`): in `ConfigHandle::reload`, the
   fail-closed secret check applies only when the **running** config's
   `webhook.enabled` is true. Add `enabled = true` to both `[webhook]`
   fixtures of `reload_refuses_to_empty_the_secret_on_a_public_bind`, and
   add `reload_with_a_disabled_webhook_skips_the_secret_check`.
2. **bug-09a-2** (`bugs/bug-09a-2.md`): in `parse_generic`, only an
   **empty object** yields no alert. A string, array or number body keeps
   the full-body description fallback. Add
   `generic_non_object_body_still_alerts`.

**Finish condition:** the lib suite reports **3188** passed, not 3186 and not
more.

**Mutation check:** remove the `enabled` condition and confirm that the new
handle.rs test fails, then restore it by undoing that one edit. Paste the
failing run.

Run the four gates once each, then make one `fix:` commit.

**Your first tool call is an edit to `src/config/handle.rs`.** Do not re-read
`server.rs`.

