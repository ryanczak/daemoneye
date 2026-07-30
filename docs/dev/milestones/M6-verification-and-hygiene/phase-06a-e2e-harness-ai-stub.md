# Phase 06a: E2E Harness — Canned-AI Stub and Webhook Plumbing

**Milestone:** M6 — Verification & Hygiene
**Status:** todo
**Depends on:** phase-01 (done), phase-05 (done)
**Estimated diff:** ~300 lines
**Tags:** language=rust, kind=test-infra, size=m

## Goal

Give `IsolatedEnv` the two things the webhook→ghost scenario needs and does not
have: a **canned-AI stub server** the daemon can be pointed at, and **webhook
plumbing** (a collision-free port plus a POST helper).

This phase ships no scenario and asserts nothing about ghosts. It ships the
instrument and proves the instrument works. Phase 06b writes the scenario.

**Why the split:** the original phase 06 needed a stub server, free-port
allocation, config plumbing, a runbook fixture, and the scenario itself. That is
more than one executor session (`WORKFLOW.md` § Phases). 06a is the
infrastructure; 06b is the assertion.

## Architecture references

Read before starting:

- `tests/harness/mod.rs` — `IsolatedEnv`, which you are extending. Phase 01 built
  it; do not redesign it.
- `src/webhook/server.rs:100-107` — how the listener binds.
- `src/webhook/process.rs:349-372` — the AI call your stub must satisfy.
- `src/ai/backends/` — the per-provider streaming implementations. **Read the one
  you choose** before writing the stub.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom, including §1's
   mechanical-capture box for end-to-end transcripts.
2. Read `tests/harness/mod.rs` in full.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is clean and `cargo test` is green at 964 lib / 30
   integration (2 ignored) / 3 isolation.

## Current state

**Everything below was verified against the tree while drafting.**

**The harness already has** a throwaway `$HOME`, a private tmux server via
`TMUX_TMPDIR`, `start_daemon()` / `stop_daemon()` / `daemon_log()`, and
`write_test_config()` which writes `TEST_CONFIG_TOML` (`tests/harness/mod.rs:11-19`)
**after** `daemoneye setup`, because setup overwrites `config.toml`.

**The daemon's AI endpoint is redirectable.** `maybe_analyze_alert`
(`src/webhook/process.rs:349-354`) builds its client as:

```rust
let model_entry = state.config.resolve_model(None);
let client = crate::ai::make_client(
    &model_entry.provider,
    model_entry.resolve_api_key(),
    model_entry.model.clone(),
    model_entry.effective_base_url(),
);
```

`ModelConfig::base_url` is `Option<String>` and `effective_base_url()`
(`src/config/types.rs:586`, `:661`) prefers it over the provider default. **So
setting `base_url` in the test config points the daemon at a local stub** — this
is the constraint the milestone README flagged as open for phase 06, and it is
now closed. There is no need to mock at the Rust level or to reach the network.

**The watchdog call is `use_tools=false`** and its result is consumed as plain
tokens (`process.rs:356-372`):

```rust
let (ai_tx, mut ai_rx) = tokio::sync::mpsc::unbounded_channel::<AiEvent>();
… client.chat(&system, msgs, ai_tx, false, Vec::new()).await …
while let Some(ev) = ai_rx.recv().await {
    if let AiEvent::Token(t) = ev { response.push_str(&t); }
}
```

So the stub only has to produce a token stream. It does **not** need to support
tool calls.

**The webhook port is configurable** — `WebhookConfig.port` (default 9393) and
`bind_addr` (`config/types.rs:462-481`). **This matters more than it looks:** per
`CLAUDE.md`, the webhook listener binds eagerly in `run_daemon` and **a bind
failure is fatal**. If the operator's own daemon holds 9393, an isolated daemon
that also asks for 9393 will fail to start. Every `IsolatedEnv` must therefore
get its own free port.

**No new dependencies are needed.** `axum` (0.8, with `http1`+`json`+`tokio`) and
`tokio` (with `rt-multi-thread`) are in `[dependencies]`, and Cargo makes
`[dependencies]` available to test targets alongside `[dev-dependencies]`.
Building the stub on axum is free. **Adding a dependency is a blocker — report
it, do not do it.**

## Spec

### 1. A free webhook port per environment

`IsolatedEnv` gains a port, allocated at construction, that nothing else on the
machine is using. Bind a `TcpListener` to port `0`, read the assigned port, and
release it — then hand that number to the daemon's config. Expose it (a
`webhook_port()` accessor or a public field; your call).

There is an inherent race between releasing the probe socket and the daemon
binding it. Accept it, but **do not paper over it**: if `start_daemon` fails, the
existing assertion already surfaces `daemon.log`, which is enough to diagnose.
Do not add retry loops or sleeps to hide it.

### 2. Config plumbing

`write_test_config()` currently writes a fixed string. It needs to produce a
config that also carries:

- `[webhook]` with `enabled = true`, the allocated `port`, and `bind_addr =
  "127.0.0.1"`.
- `base_url` on the default model, pointing at the stub's address.

Keep the existing model fields (a dummy `api_key` is still required — the
daemon's key resolution must succeed). Shape is yours: a format string, a builder,
or `toml` serialisation. Callers that do not care about the webhook must keep
working — `start_daemon` is used by phase 01's existing tests, and **those three
isolation tests must still pass unchanged**.

### 3. The canned-AI stub

A small HTTP server, in the harness, that answers the chat endpoint of **one**
provider with a canned body. Pick whichever of `src/ai/backends/` is simplest to
emit faithfully, and say in a comment which one you chose and why.

Requirements:

- Bound to `127.0.0.1` on its own free port (same technique as task 1).
- Serves a **caller-supplied** response string, so 06b can hand it a body
  containing `GHOST_TRIGGER: YES` and other tests can hand it something else.
- Runs for the lifetime of the environment and shuts down with it. No leaked
  threads, no leaked ports.

**The stub's correctness must be provable without the daemon.** That is task 4.

### 4. Prove the instrument, don't assume it

The failure mode this phase must avoid: a stub that looks right, a scenario that
fails in 06b, and no way to tell which half is broken.

So: a test that drives `crate::ai::make_client(...)` directly against the stub —
same four arguments `maybe_analyze_alert` passes, `use_tools=false` — collects
`AiEvent::Token` values off the channel, concatenates them, and asserts the
result **equals the canned string it supplied**.

That is a tight, observable property: it pins the stub against the real client
code path rather than against your idea of the wire format.

Also add a test that a started daemon accepts a POST to the allocated webhook
port and returns `200`.

### 5. A POST helper

A method on `IsolatedEnv` that POSTs a JSON body to the environment's webhook
endpoint and returns the status (and body, if useful). `reqwest` is available.
06b will use this to send a severity-less payload.

## Acceptance criteria

- [ ] `IsolatedEnv` exposes a webhook port that is free at construction time and
      differs between two environments constructed in the same process.
- [ ] The written `config.toml` contains `[webhook] enabled = true`, that port,
      and a `base_url` pointing at the stub.
- [ ] A test drives `make_client(...)` against the stub and asserts the
      concatenated `AiEvent::Token` text **equals** the supplied canned string.
- [ ] A test starts a daemon and gets `200` from a POST to the webhook port.
- [ ] Phase 01's three isolation tests still pass **unchanged**.
- [ ] No new entries in `[dependencies]` or `[dev-dependencies]`.
- [ ] All four gates green.

## Test plan

Tests live in `tests/isolation.rs` alongside phase 01's, or in a sibling file if
that reads better — your call, but they must run under `cargo test` with no
special flags and must not require network access.

**Do not weaken phase 01's isolation guarantees.** The environment must still
touch neither the operator's `~/.daemoneye/` nor their default tmux server. If a
new test needs a daemon, it goes through `start_daemon()`.

**Do not pin a test count in advance.** Report the resulting count in the Update
Log and explain the delta.

## End-to-end verification

**`STANDARDS.md` §1's mechanical-capture box applies. Read it.** Redirect each
command's output to a file and paste that file's contents into a **new Update Log
entry you author** titled `### Update — <date> (end-to-end verification)`.

The server-authored `(complete)` entry's "Command output tails" block is the
standard gate capture every phase gets automatically. **It does not satisfy this
requirement.** This has been the single most common defect on this milestone —
five bounces across phases 03, 04 and 05 — and every one of them was the evidence
missing, never the code.

Capture:

```sh
cargo test --test isolation -- --nocapture \
  > /tmp/e2e-06a.txt 2>&1; echo "exit=$?" >> /tmp/e2e-06a.txt

grep -n "stub\|webhook" /tmp/e2e-06a.txt \
  > /tmp/e2e-06a-grep.txt 2>&1; echo "grep-exit=$?" >> /tmp/e2e-06a-grep.txt
```

Paste the contents of both files. The `exit=` and `grep-exit=` lines are the
point: a command that finds nothing prints nothing, and an empty block proves
nothing on its own.

## Authorizations

- [ ] May modify `tests/harness/mod.rs` and `tests/isolation.rs`, and may add new
      files under `tests/`.
- [ ] May use `axum`, `tokio`, `reqwest`, `serde_json` in test code — all already
      available.

**No new dependencies.** If you believe one is required, that is a **blocker** —
write it in the Update Log and stop.

No changes to `src/`. No changes to `docs/architecture.md`.

## Out of scope

- **Do not write the webhook→ghost scenario.** No ghost assertions, no runbook
  fixture, no `ghost_*` event checks. That is phase 06b, and this phase exists so
  06b starts from a working instrument.
- **Do not modify anything under `src/`.** If the harness cannot express
  something without a production change, that is a blocker — report it.
- **Do not change phase 01's isolation tests** to accommodate the new plumbing.
  If they break, the plumbing is wrong.
- **Do not add retries, sleeps, or polling** to mask the port race described in
  task 1.
- **Do not touch `.gitignore`, `src/pane_prefs.rs`, or `main.rs`'s stale
  `daemon.log` help strings.** Milestone housekeeping and phase 11.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->
