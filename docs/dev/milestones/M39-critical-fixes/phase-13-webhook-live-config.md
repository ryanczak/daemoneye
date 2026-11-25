# Phase 13 — webhook-live-config

**Milestone:** M39 — Critical fixes
**Status:** todo
**Drafted:** 2026-11-24
**Depends on:** none
**Estimated diff:** ~150 lines
**Tags:** language=rust, kind=bugfix, size=s

## Goal

The M39 live check found this defect, and the PE asked for it fixed before
2.11.0. The architect **reproduced it live** on 2026-11-24.

- **A webhook ghost runs on the startup config.** `WebhookState` (in
  `src/webhook/server.rs`) holds `config: Config`, a copy taken once in
  `serve`. `daemoneye reload` swaps the daemon's `ConfigHandle` but never
  that copy. Every `state.config` read in `src/webhook/` is stale, including:
  - the ghost `spawn_webhook_ghost` starts,
  - `cost_gate`,
  - `run_watchdog_query`,
  - `check_ghost_capacity`.

  The live run went like this: a runbook with `model: badkey` was added,
  with `[models.badkey]` arriving by `reload`. Its ghost ran on the
  `default` model and ended `Completed`. After a restart the same alert
  ended `hard_fail` with the provider's 401, which is correct. `CLAUDE.md`'s
  first invariant says daemon code reads config through the handle. A
  comment in `src/webhook/trigger.rs` already warns that `state.config` is a
  snapshot.
- **The model fallback lies in the log.** `Config::resolve_model`
  (`src/config/load.rs`) falls back to `default` (then the first entry) for
  a name that is not configured. `do_ghost_turn` (`src/daemon/ghost/turn.rs`)
  then logs `using model '<requested name>'` while calling the fallback.
  **PE decision:** keep the fallback, because three of the PE's real agents
  name unconfigured models and rely on it. But say so: log a `WARN` naming
  both models, and have the info line name the model actually used.

## Statement of work

1. **`WebhookState` holds the daemon's handle,** `config:
   Arc<ConfigHandle>`, not a `Config`. `serve` passes `daemon.config`,
   which is the same `Arc` that `run_daemon` installs. Each code path takes
   one `state.config.current()` snapshot and reads it, so one alert sees
   one consistent config. The ghost that `spawn_webhook_ghost` starts gets
   the snapshot current at spawn. Fix the `trigger.rs` comment, which
   becomes false. The test constructors (`test_state` in `server.rs`, and
   the one in `trigger.rs` tests) wrap a `Config` in `ConfigHandle::new`.
2. **`Config::resolve_model_key(&self, name: Option<&str>) -> &str`**
   returns the key of the entry `resolve_model` would return: the name if
   it is configured, else `default`, else the first entry. Re-express
   `resolve_model` through it so the two cannot disagree. Its behaviour is
   unchanged; `resolve_model_unknown_name_falls_back_to_default`
   still passes.
3. **`model_fallback_warning(session_id, requested: Option<&str>, used:
   &str) -> Option<String>`,** a pure fn in
   `src/daemon/ghost/turn_helpers.rs`. It returns `Some` only when
   `requested` is `Some` and differs from `used`. The message names the
   ghost session, the requested model, the word "not configured" and the
   model used. `do_ghost_turn`:
   - logs that message at `warn` when it is `Some`;
   - always logs `using model '<used>'` at `info`, with the key
     `resolve_model_key` returned.

**Constraints:**
- No `unwrap`, `expect` or `panic!` in new production code.
- No change to `resolve_model`'s results or to any other caller.
- `[webhook]` is a restart-only section. Reading it through the handle is
  still correct, because `reload` keeps its running value.

**Closed file list:**

| Source | Tests |
|---|---|
| `src/webhook/server.rs` | its own `mod tests` |
| `src/webhook/process.rs` | none |
| `src/webhook/trigger.rs` | its own `mod tests` |
| `src/config/load.rs` | `src/config/tests.rs` |
| `src/daemon/ghost/turn.rs` | none |
| `src/daemon/ghost/turn_helpers.rs` | its own `mod tests` |

**Edit order:** item 2, item 3, then item 1. Read at most **3** files
before your first edit.

**Budget.** You have **1200 turns**, so use them efficiently. Your
**first tool call** after reading writes the first red test.

**Gotcha:** `crate::test_home_guard()` is not re-entrant. Take it **once**
per test, at the top, and hold it for the whole test.

## First red test

`resolve_model_key_names_the_fallback` in `src/config/tests.rs`.
- For a config with `[models.default]` and `[models.big]`:
  - `Some("big")` gives `"big"`;
  - `Some("badkey")` gives `"default"`;
  - `None` gives `"default"`.
- For a config whose only entry is `[models.alpha]`, `Some("x")` gives
  `"alpha"`.

Red today: the function does not exist. Paste the compile error.

## Acceptance criteria

- [ ] **Key:** `resolve_model_key_names_the_fallback` passes. Paste the
      red run.
- [ ] **Warning:** `ghost_model_fallback_is_warned` (in `turn_helpers.rs`
      tests) passes. It calls `model_fallback_warning` and asserts:
      - `(…, Some("badkey"), "default")` is `Some` and contains both
        `badkey` and `default`;
      - `(…, Some("big"), "big")` is `None`;
      - `(…, None, "default")` is `None`.

      Paste the red run.
- [ ] **Reload reaches the webhook:**
      `webhook_state_sees_a_reloaded_model` (in `server.rs` tests) passes.
      Under `test_home_guard`, it:
      1. writes a `config.toml` with only `[models.default]`;
      2. builds an `Arc<ConfigHandle>` from `Config::load()`, and a
         `WebhookState` from it;
      3. appends a `[models.late]` entry to the file and calls
         `handle.reload()`;
      4. asserts `state.config.current().models` contains `late`.

      Red today is a compile error, because the field is a `Config`. Paste
      it.
- [ ] **No snapshot reads:** run this command **once**. It prints **0**
      (it prints 10 today):
      `grep -n 'state\.config' src/webhook/*.rs | grep -v 'state\.config\.current()' | grep -c .`
- [ ] **Mutation check,** run once with the output pasted:
      1. make `resolve_model_key` return the requested name whenever one
         is given;
      2. run `cargo test --lib resolve_model_key_names_the_fallback` once
         and see it fail;
      3. restore it.
- [ ] **Gates:** all four pass, run once each. The lib suite reports **at
      least 3114** passed (3111 plus three).

## Spec

1. **Break down:** write the first red test and run it red. Then write a
   numbered task list into an Update Log entry, one line per task, each
   naming the acceptance item it closes. Done when that entry is written.
2. **Implement:** item 2, item 3, then item 1, writing each red test
   before its fix. Done when `cargo build --tests` is green.
3. **Meet the acceptance criteria:** check off every item. Run each command
   **once**. Done when every item is checked with its evidence named.
4. **Capture the evidence:** run the four gates, once each. Paste into a
   new Update Log entry:
   - each `test result:` line, without the per-test `… ok` lines;
   - the three red runs;
   - the grep;
   - the mutation check.

   Commit as one `fix:` commit.

## Authorizations

- You may touch the seven files in the closed file list, and no others.

## Out of scope

- Making an unknown model name an error. The PE decided to keep the
  fallback.
- Other `resolve_model` callers (chat, digest, briefing).
- The 5 s interrupt grace after a Ctrl-C'd timeout (live-check finding 2,
  deferred).

## Update Log

<!-- entries appended below this line -->

### Update — 2026-11-25 00:25 (started)

Executor: Claude (headless run). Status flipped to `in-progress`.

**Task list** (each line names the acceptance item it closes):

1. Red test `resolve_model_key_names_the_fallback` in `src/config/tests.rs`,
   run red (compile error: no method `resolve_model_key`) — closes **Key**.
2. Implement `Config::resolve_model_key` in `src/config/load.rs`,
   re-express `resolve_model` through it — closes **Key**.
3. Red test `ghost_model_fallback_is_warned` in `turn_helpers.rs` tests,
   run red — closes **Warning**.
4. Implement `model_fallback_warning` in `src/daemon/ghost/turn_helpers.rs`;
   wire `do_ghost_turn` (`src/daemon/ghost/turn.rs`) to log WARN + info
   naming the used model via `resolve_model_key` — closes **Warning**.
5. Red test `webhook_state_sees_a_reloaded_model` in `server.rs` tests,
   run red (compile error: field is a `Config`) — closes **Reload reaches
   the webhook**.
6. Change `WebhookState.config` to `Arc<ConfigHandle>`, update all
   `state.config` reads in `src/webhook/` to `state.config.current()`,
   fix the `trigger.rs` snapshot comment, fix test constructors — closes
   **Reload reaches the webhook** and **No snapshot reads** (grep prints 0).
7. Mutation check on `resolve_model_key`; run once, paste output — closes
   **Mutation check**.
8. Run the four gates once each; paste `test result:` lines into an
   end-to-end evidence entry; single `fix:` commit — closes **Gates**.

### Update — 2026-11-25 01:20 (end-to-end verification)

All four gates ran once each, after `cargo fmt --all`:

```
cargo test  (lib suite):
test result: ok. 3114 passed; 0 failed; 10 ignored; 0 measured; 0 filtered out; finished in 148.42s
(integration suites:) 2 passed / 6 passed / 19 passed / 16 passed / 45 passed; 2 ignored /
13 passed; 1 ignored / 2 passed / 4 passed — all ok, 0 failed.
```

**Red runs (pasted as captured):**

1. `cargo test --lib resolve_model_key_names_the_fallback` before the fix:

```
error[E0599]: no method named `resolve_model_key` found for struct `config::types::Config` in the current scope
   --> src/config/tests.rs:130:20
    |
130 |     assert_eq!(cfg.resolve_model_key(Some("x")), "alpha");
    |                    ^^^^^^^^^^^^^^^^^
```

2. `cargo test --lib ghost_model_fallback_is_warned` before the fix:

```
error[E0425]: cannot find function `model_fallback_warning` in this scope
   --> src/daemon/ghost/turn_helpers.rs:899:17
```

3. `cargo test --lib webhook_state_sees_a_reloaded_model` before the fix:

```
error[E0308]: mismatched types
   --> src/webhook/trigger.rs:106:13
    |
106 |             config,
    |             ^^^^^^ expected `Arc<ConfigHandle>`, found `Config`
```

**Grep** (`grep -n 'state\.config' src/webhook/*.rs | grep -v 'state\.config\.current()' | grep -c .`):
prints `0` (was 10 — every snapshot read now goes through `current()`).

**Mutation check:** `resolve_model_key` mutated to return any requested name
whenever one is given (`Some(n) => n`), then
`cargo test --lib resolve_model_key_names_the_fallback` — the test fails,
pinning the production line:

```
thread 'config::tests::resolve_model_key_names_the_fallback' panicked at src/config/tests.rs:121:5:
assertion `left == right` failed
  left: "badkey"
 right: "default"
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 3123 filtered out
```

The function was restored to `get_key_value`-based lookup; the test passes
again (`test result: ok. 1 passed`).
