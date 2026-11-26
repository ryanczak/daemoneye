# Phase 10 — masking

**Milestone:** M40 — Security hardening
**Status:** todo
**Drafted:** 2026-11-26
**Depends on:** 09b (done)
**Estimated diff:** ~250 lines
**Tags:** language=rust, kind=bugfix, size=m

## Goal

This phase closes audit rows **S13** and **S14**. The architect probed both
on 2026-11-26.

1. **S13 — the JSON secret rule emits `"$1"`.**
   - **The defect:** `mask_sensitive` (`src/ai/filter.rs`) replaces through
     a closure, `re.replace_all(&result, |_| { n += 1; rep_str })`. A
     closure's returned string is used **literally**; `$1` is never
     expanded. The JSON-quoted secret rule's replacement is
     `"$1": "<REDACTED>"`.
   - **The probe:** run against the same regex crate version,
     `{"password": "hunter2"}` became `{"$1": "<REDACTED>"}`. The key name
     is lost.
   - **Why the tests miss it:** the test helper `mask` in the same file
     calls `replace_all(&result, rep.as_str())`, which **does** expand.
     So `secrets_mask_json_quoted` tests a path production never takes.
2. **S14 — credentials outlive their use, and `Debug` prints secrets.**
   - `credential_round_trip` (`src/daemon/executor/shell_exec_auth.rs`)
     wraps the credential in `Zeroizing`, then copies it into a plain
     `Vec<u8>` with `as_bytes().to_vec()` and `push(b'\n')`. That `Vec` is
     returned in `PromptReply::Send(Vec<u8>)` and is never zeroed.
     `push` on a full `Vec` reallocates, leaving a second unzeroed copy.
   - `PromptReply` (`src/shell/client.rs`) derives `Debug`, so `{:?}` on
     `Send` prints the password bytes.
   - `Request` (`src/ipc.rs`) derives `Debug` over
     `CredentialResponse { credential: String }`.
   - `ModelEntry` (`src/config/models.rs`) derives `Debug` over `api_key`.
   - `WebhookConfig` (`src/config/types.rs`) derives `Debug` over `secret`.
   - `Config`'s derived `Debug` prints all of these.

## Statement of work

1. **One replacement path (S13).**
   - Factor the per-pattern replacement in `mask_sensitive` into one
     private function. It expands captures in the replacement
     (`Captures::expand`) and counts matches.
   - The test helper `mask` calls that same function, so tests and
     production can no longer diverge.
   - The output of every other built-in pattern is unchanged: none of
     their replacements contains a `$`.
2. **The answer stays zeroizing (S14).**
   - `PromptReply::Send` carries `zeroize::Zeroizing<Vec<u8>>`.
   - `credential_round_trip` builds that buffer with its capacity reserved
     **before** it copies in the credential and the newline, so no
     reallocation leaves a copy behind.
   - `write_answer` (`src/shell/client_read.rs`) zeroizes what it
     produces from the answer: the `ShellRequest::Input` bytes and the
     encoded wire string. It uses each before zeroizing it.
   - The two test call sites in `src/shell/client_tests.rs` adapt to the
     new payload type.
3. **Redacting `Debug` (S14).**
   - `PromptReply`'s `Debug` prints `Send(<N bytes redacted>)`, never the
     bytes. Keep `PartialEq` / `Eq`.
   - `Request::CredentialResponse.credential` becomes a newtype. It is
     `#[serde(transparent)]` over `String`, so the wire bytes are
     unchanged. Its `Debug` prints `<redacted>`, it implements
     `From<String>`, and it exposes the inner `&str`.
     - Every construction site uses `.into()`.
     - The daemon reads it into `Zeroizing` as today.
   - `ModelEntry` and `WebhookConfig` replace their derived `Debug` with a
     hand-written one. It prints every field except the secret
     (`api_key`, `secret`), which shows as `<redacted>`, or as `""` when
     the secret is empty.

**Constraint: `src/ipc.rs` is at the 800-production-line cap**
(`tests/hygiene.rs`).
- Define the credential newtype in `src/ipc_types.rs`. That is the child
  module `ipc.rs` glob re-exports.
- Change only the field's type in `ipc.rs`, adding no lines.
- Do not edit `tests/hygiene_allowlist.txt`.

**Closed file list:**
- `src/ai/filter.rs`;
- `src/daemon/executor/shell_exec_auth.rs`;
- `src/shell/client.rs`, `src/shell/client_read.rs`,
  `src/shell/client_tests.rs`;
- `src/ipc.rs`, `src/ipc_types.rs`, `src/ipc_tests.rs`;
- `src/cli/commands/stream_ask.rs`, `src/cli/commands/ask.rs`;
- the test files that build a `CredentialResponse`, found with:
  `grep -rln 'CredentialResponse {' src --include='*_tests.rs'`. Today
  that finds five files, `src/ipc_tests.rs` among them;
- `src/config/models.rs`, `src/config/types.rs`.

**Edit order:** each step ends with `cargo build --tests`.
1. **Your first tool call is an edit to `src/ai/filter.rs`:** the red test
   `json_secret_rule_keeps_its_key`.
2. The S13 fix.
3. The `PromptReply` payload and `Debug`, with their tests.
4. The credential newtype and its call sites.
5. `ModelEntry` and `WebhookConfig`.

**Read limit:** read at most **3** files before your first edit.

**Budget:** you have **1200 turns**; use them efficiently.

## First red test

`json_secret_rule_keeps_its_key`, in `src/ai/filter.rs`'s tests. It goes
through **`mask_sensitive`**, not the test helper.

- **Asserts:**
  - `{"password": "hunter2"}` becomes exactly `{"password": "<REDACTED>"}`;
  - `{"API_KEY":"abc"}` becomes exactly `{"API_KEY": "<REDACTED>"}`, with
    the key's case kept;
  - neither output contains `$1`.
- **Today:** it is red, because the output is `{"$1": "<REDACTED>"}`.

Paste the red run.

## Acceptance criteria

- [ ] **S13:** `json_secret_rule_keeps_its_key` passes. Paste the red run.
      `secrets_mask_json_quoted` and every other `filter.rs` test pass
      unchanged.
- [ ] **One path:** the test helper `mask` and `mask_sensitive` call the
      same replacement function.
- [ ] **PromptReply:** `prompt_reply_debug_hides_the_answer` passes.
      - **Setup:** `format!("{:?}", …)` of a `Send` built from
        `b"hunter2\n"`.
      - **Asserts:** the text does not contain `hunter2`; it does contain
        `8 bytes`.
- [ ] **Round trip:**
      `credential_round_trip_returns_a_zeroizing_answer` passes.
      - **Setup:** a `Vec<u8>` writer, and a `Cursor` reader holding one
        `CredentialResponse` line for `"pw"`.
      - **Asserts:** the reply is `Send`; its payload is typed
        `Zeroizing<Vec<u8>>`, held through a binding annotated with that
        type; it equals `b"pw\n"`.
- [ ] **Request:** `credential_response_debug_hides_the_credential`
      passes. `{:?}` of a `CredentialResponse` holding `hunter2` does not
      contain `hunter2`.
- [ ] **Wire unchanged:**
      `credential_response_wire_bytes_are_unchanged` passes. The
      serialized JSON is
      `{"CredentialResponse":{"id":"p","credential":"pw"}}`, and it
      round-trips.
- [ ] **Config:**
      `model_entry_and_webhook_debug_hide_their_secrets` passes.
      - **Hidden:** `{:?}` of a `ModelEntry` with `api_key = "sk-xyz"`, of
        a `WebhookConfig` with `secret = "s3cret"`, and of a `Config`
        holding both contains neither value.
      - **Still shown:** each output still shows the `model` / `port`
        field.
- [ ] **Mutation run, pasted as output:**
      1. Make the replacement closure return the raw replacement string
         again.
      2. Run `cargo test --lib json_secret_rule_keeps_its_key` once. It
         must fail.
      3. Restore by undoing that one edit, never by copying a backup file.
- [ ] **No test removed:** run this once. It prints **0**:
      `git diff 0b2a5ca8 -- src tests | grep -cE '^-\s*#\[(tokio::)?test\]'`
- [ ] **Gates:** all four pass, each run once. `tests/hygiene.rs` passes
      with the allowlist untouched.

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
- You may change existing call sites and test fixtures to the new
  credential and `PromptReply` types.

## Out of scope

- **Zeroizing the credential on the client side**, in the CLI prompt and
  `stream_ask`. Only the call sites change, to `.into()`.
- **Redacting other `Debug` derives.** Only the four named types change.
- **New masking patterns.**

## Update Log

<!-- entries appended below this line -->
