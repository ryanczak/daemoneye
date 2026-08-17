# Phase 01: Transport scaffolding — two-phase stream timeouts, configurable, circuit-breaker stream hooks

**Milestone:** M16 — LLM Stream Robustness
**Status:** in-progress (dispatched 2026-08-16, DeepSeek V4 Flash 0731)
**Depends on:** none
**Estimated diff:** ~260 lines
**Tags:** language=rust, kind=feature, size=m

## Goal

Add the shared machinery every backend will use in phases 02–03: three new
`[ai]` config keys, a global stream-timeout store initialised at daemon start,
a `stream_next_with_timeout` helper that converts a stalled `stream.next()`
into an explicit phase-accurate error, and circuit-breaker hooks for
mid-stream failures. **Purely additive — no backend behavior changes in this
phase.** The shared `http()` client is NOT touched here (its `.timeout(300)`
and `.read_timeout` are removed in phase-03, after all backends carry their
own timeouts).

## Architecture references

Read before starting:

- `docs/design/daemon-stalls.md` § 1.5 — mechanism C (provider accepts the
  connection then goes silent); this phase builds the primitive that reports
  it accurately.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture reference above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

(Line numbers current as of 2026-08-16; re-derive with the greps shown.)

- `src/config/types.rs` — `AiConfig` (find with `grep -n "pub struct AiConfig" src/config/types.rs`, ~line 855) currently holds a single field, `prompt`, with the serde-default idiom used throughout the file:

```rust
pub struct AiConfig {
    /// Name of a prompt file in `~/.daemoneye/prompts/` (without `.toml`).
    /// Defaults to `"sre"`.
    #[serde(default = "default_prompt")]
    pub prompt: String,
}

fn default_prompt() -> String {
    "sre".to_string()
}

impl Default for AiConfig {
    fn default() -> Self {
        AiConfig {
            prompt: default_prompt(),
        }
    }
}
```

- `src/ai/mod.rs` — `STREAM_IDLE_TIMEOUT` (120 s const) and `stream_chunk()`
  live at ~lines 117–147; the shared client at ~197–206 has
  `.timeout(Duration::from_secs(300))` + `.read_timeout(STREAM_IDLE_TIMEOUT)`
  (leave both alone this phase). `send_with_retry` + the `CircuitBreaker`
  (`circuit()`, `record_success`/`record_failure`) are at ~lines 91–95 and
  237–305. The masking-init precedent this phase mirrors:
  `crate::ai::filter::init_masking(...)` is called from
  `src/daemon/mod.rs:472` during daemon startup.
- `grep -c "first_token" src/ai/mod.rs` returns `0` — nothing from this phase
  exists yet.

## Spec

### Task 1 — Add the three `[ai]` timeout keys to `AiConfig`

In `src/config/types.rs`, extend `AiConfig` with three fields, following the
exact serde-default idiom quoted in Current state (one `default_*` fn per
field, all fields added to `impl Default`):

- `first_token_timeout_secs: u64`, default **600** — max wait for the first
  real token of a response (covers slow prefill on long prompts).
- `stream_idle_timeout_secs: u64`, default **240** — max gap between chunks
  once the stream has produced a token.
- `connect_timeout_secs: u64`, default **30** — TCP/TLS connect budget
  (consumed by phase-03; the field ships now so config is complete).

Add doc comments on each field (they are user-facing config docs).

### Task 2 — Add `StreamTimeouts` and `init_stream_timeouts` to `src/ai/mod.rs`

A global store mirroring the `init_masking` pattern (set once at daemon
start, defaults when uninitialised — tests and CLI-side callers never call
the initialiser):

```rust
/// Stream-timeout budgets, set once at daemon start from `[ai]` config.
#[derive(Debug, Clone, Copy)]
pub struct StreamTimeouts {
    pub first_token: std::time::Duration,
    pub stream_idle: std::time::Duration,
    pub connect: std::time::Duration,
}

impl Default for StreamTimeouts {
    fn default() -> Self {
        StreamTimeouts {
            first_token: std::time::Duration::from_secs(600),
            stream_idle: std::time::Duration::from_secs(240),
            connect: std::time::Duration::from_secs(30),
        }
    }
}

static STREAM_TIMEOUTS: OnceLock<StreamTimeouts> = OnceLock::new();

/// Install the configured budgets. Later calls are ignored (OnceLock).
pub fn init_stream_timeouts(cfg: &crate::config::AiConfig) {
    let _ = STREAM_TIMEOUTS.set(StreamTimeouts {
        first_token: std::time::Duration::from_secs(cfg.first_token_timeout_secs),
        stream_idle: std::time::Duration::from_secs(cfg.stream_idle_timeout_secs),
        connect: std::time::Duration::from_secs(cfg.connect_timeout_secs),
    });
}

pub fn stream_timeouts() -> StreamTimeouts {
    STREAM_TIMEOUTS.get().copied().unwrap_or_default()
}
```

Ensure the `Default` values and the `default_*` fns in Task 1 agree (600 /
240 / 30). Adjust imports as needed (`OnceLock` is already imported in this
file for other statics).

### Task 3 — Call `init_stream_timeouts` at daemon start

In `src/daemon/mod.rs`, directly next to the existing
`crate::ai::filter::init_masking(&startup_config.masking.extra_patterns);`
call (~line 472), add:

```rust
crate::ai::init_stream_timeouts(&startup_config.ai);
```

### Task 4 — Port `select_timeout` and `stream_next_with_timeout`

In `src/ai/mod.rs`, add (adapted from rexyMCP, quoted here in full — do not
invent a different shape):

```rust
/// Select which timeout bounds the next stream read: the (long) first-token
/// budget before any real token has arrived, the (shorter) idle budget after.
pub(crate) fn select_timeout(first_token_seen: bool, t: StreamTimeouts) -> std::time::Duration {
    if first_token_seen {
        t.stream_idle
    } else {
        t.first_token
    }
}

/// Read the next stream item under a timeout, converting a stall into an
/// explicit, phase-accurate error instead of an unbounded await.
pub(crate) async fn stream_next_with_timeout<B>(
    stream: &mut (impl futures_util::Stream<Item = reqwest::Result<B>> + Unpin),
    timeout: std::time::Duration,
    first_token_seen: bool,
) -> Option<anyhow::Result<B>> {
    use futures_util::StreamExt;
    match tokio::time::timeout(timeout, stream.next()).await {
        Ok(Some(Ok(bytes))) => Some(Ok(bytes)),
        Ok(Some(Err(e))) => Some(Err(e.into())),
        Ok(None) => None,
        Err(_elapsed) => Some(Err(if first_token_seen {
            anyhow::anyhow!(
                "AI stream went idle mid-response: no data for {}s after output began \
                 (provider or network dropped the stream)",
                timeout.as_secs()
            )
        } else {
            anyhow::anyhow!(
                "AI produced no output for {}s before the first token \
                 (provider accepted the request then went silent)",
                timeout.as_secs()
            )
        })),
    }
}
```

Also port the two small classifiers, verbatim:

```rust
/// A stream error worth retrying: a transport/body failure (connection dropped
/// mid-stream), as opposed to a stall timeout or a runaway-buffer abort, which
/// are synthetic `anyhow` errors that don't downcast to `reqwest::Error`.
pub(crate) fn is_retriable_transport(e: &anyhow::Error) -> bool {
    e.downcast_ref::<reqwest::Error>().is_some()
}

/// Bounded exponential backoff for mid-stream transport retries:
/// 250 ms, 500 ms, 1 s, capped at 2 s.
pub(crate) fn stream_retry_backoff(attempt: u32) -> std::time::Duration {
    let ms = (250 * 2u64.pow(attempt.saturating_sub(1))).min(2000);
    std::time::Duration::from_millis(ms)
}
```

`futures_util` is already a dependency (used by the backends); no new
dependencies.

### Task 5 — Circuit-breaker hooks for mid-stream failures

In `src/ai/mod.rs`, next to `send_with_retry`, add two thin public helpers so
backends (phases 02–03) can feed stream-phase outcomes into the existing
breaker, which today only sees the header exchange:

```rust
/// Record a mid-stream failure against the circuit breaker. `send_with_retry`
/// only accounts for the header exchange; backends call this when the stream
/// itself fails after a 200.
pub(crate) fn record_stream_failure() {
    circuit().record_failure();
}

/// Record a stream that reached its natural end.
pub(crate) fn record_stream_success() {
    circuit().record_success();
}
```

### Task 6 — Unit tests

In the existing `mod tests` of `src/ai/mod.rs` (found via
`grep -n "mod tests" src/ai/mod.rs`), add:

- `select_timeout_uses_first_token_budget_before_first_token` — with a
  `StreamTimeouts { first_token: 600s, stream_idle: 240s, .. }`, asserts
  `select_timeout(false, t) == 600s` and `select_timeout(true, t) == 240s`.
- `stream_next_timeout_reports_first_token_stall` — `#[tokio::test(start_paused = true)]`:
  drive `stream_next_with_timeout` over a `futures_util::stream::pending()`
  stream with a short timeout and `first_token_seen = false`; assert the
  error message contains `"before the first token"`. (Use
  `tokio::time::advance` or rely on auto-advance under paused time.)
- `stream_next_timeout_reports_mid_stream_idle` — same with
  `first_token_seen = true`; assert the message contains
  `"idle mid-response"`.
- `stream_retry_backoff_is_bounded` — asserts backoff(1) = 250 ms,
  backoff(2) = 500 ms, backoff(3) = 1 s, backoff(10) = 2 s.
- `synthetic_stall_error_is_not_retriable_transport` — asserts
  `is_retriable_transport(&anyhow::anyhow!("stall"))` is `false`.
- `stream_timeouts_defaults_without_init` — asserts `stream_timeouts()`
  returns the 600/240/30 defaults when never initialised.

Note the pinned stream type: `futures_util::stream::pending::<reqwest::Result<bytes::Bytes>>()`
does not compile if `bytes` is not a direct dependency — use any payload type
(`Vec<u8>` or `()`); the helper is generic over `B`.

### Task 7 — Config-parse test

Next to the existing config tests (find the `[ai]` parsing tests with
`grep -rn "AiConfig" src/config/ | grep -n test` and follow the file's local
convention), add `ai_timeout_keys_parse_and_default`: parse a TOML snippet
containing `[ai]\nfirst_token_timeout_secs = 10` and assert the parsed value
is 10 while `stream_idle_timeout_secs` defaults to 240 and
`connect_timeout_secs` to 30; also assert `AiConfig::default()` matches
600/240/30.

### Task 8 — Capture the end-to-end evidence

Run the block in § End-to-end verification **verbatim and unmodified**, then
paste the resulting `/tmp/e2e-01.txt` into a new Update Log entry headed
`### Update — <date> (end-to-end verification)`. The server-authored
`(complete)` entry does not satisfy this.

## Acceptance criteria

- [ ] `grep -c "pub first_token_timeout_secs" src/config/types.rs` prints `1`
      (currently `0`).
- [ ] `grep -c "fn init_stream_timeouts" src/ai/mod.rs` prints `1`
      (currently `0`).
- [ ] `grep -c "init_stream_timeouts" src/daemon/mod.rs` prints `1`
      (currently `0`).
- [ ] `cargo test select_timeout_uses_first_token_budget_before_first_token`
      passes; `cargo test stream_next_timeout` passes (both stall tests).
- [ ] `cargo test ai_timeout_keys_parse_and_default` passes.
- [ ] All four gates green, with `cargo test --lib` as the test gate — the
      full suite carries one documented pre-existing failure
      (`hooks_land_on_private_server`, a post-M15 regression from `90567c3`
      in a parallel work stream; see NEXT.md "Deferred follow-ups"). It is
      not this phase's defect and must not be "fixed" here.
- [ ] The end-to-end entry ends with `PASTE MATCH`.

## Test plan

Tests are enumerated in Spec Tasks 6–7. All timing tests use
`#[tokio::test(start_paused = true)]` — no real sleeps (STANDARDS § 3.3).

## End-to-end verification

The phase ships config keys the running binary loads, but no daemon-visible
behavior change (backends still use the old path until phase-02). Evidence is
the gate run plus the new-surface greps:

```sh
A=/tmp/e2e-01.txt; : > "$A"
grep -c "pub first_token_timeout_secs" src/config/types.rs >> "$A"
grep -c "fn init_stream_timeouts" src/ai/mod.rs >> "$A"
grep -c "init_stream_timeouts" src/daemon/mod.rs >> "$A"
cargo test stream_next_timeout 2>&1 | tail -5 >> "$A"; echo "exit=${PIPESTATUS[0]}" >> "$A"
cargo test ai_timeout_keys_parse_and_default 2>&1 | tail -5 >> "$A"; echo "exit=${PIPESTATUS[0]}" >> "$A"
cargo test --lib 2>&1 | tail -3 >> "$A"; echo "exit=${PIPESTATUS[0]}" >> "$A"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -3 >> "$A"; echo "exit=${PIPESTATUS[0]}" >> "$A"
```

Then the paste-fidelity self-check (run after pasting the entry):

```sh
D=docs/dev/milestones/M16-llm-stream-robustness/phase-01-transport-scaffolding.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-01.txt
diff /tmp/pasted-01.txt /tmp/e2e-01.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

Append the verdict line to the entry.

## Authorizations

None. (No new dependencies — `futures_util`, `tokio`, `anyhow`, `reqwest`
are all already direct dependencies.)

## Out of scope

- Touching `http()` — the shared client keeps `.timeout(300)` and
  `.read_timeout` until phase-03.
- Modifying any backend (`src/ai/backends/`) or `stream_chunk()`.
- Threading timeouts through `make_client` — the global store is the design.
- Any daemon `stream.rs` / CLI changes.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-08-16 (architect notes for re-dispatch)

**Round 1 hard-failed on `RunawayOutput` (one bash call emitted 291 KB)
during wrap-up. The implementation is DONE and VERIFIED** — the architect
independently ran the gates against your tree: `cargo build` green, clippy
`-D warnings` green, `cargo test --lib` green (1306 passed), and all six
Task-6 tests plus the Task-7 config test exist and pass. **Do not re-edit
any source file.**

The ONLY remaining work is **Task 8**: run the § End-to-end verification
block verbatim, paste `/tmp/e2e-01.txt` into a new
`### Update — <date> (end-to-end verification)` entry, run the PASTE MATCH
self-check, append its verdict line, write the completion entry, and commit
(one conventional commit; do NOT include `docs/dev/NEXT.md`,
`docs/dev/milestones/M15-*`, `rexymcp.toml`, or the `src/memory/` rename in
your commit — stage only the four source files you changed plus this phase
doc).

Constraints that prevent a repeat of the round-1 kill:

- **Pipe every cargo command through `tail`** exactly as the E2E block
  does. Never run `cargo test` or `cargo build` bare, and never run
  `git diff` without `--stat`.
- The working tree legitimately contains, besides your own work: an
  unrelated staged rename under `src/memory/` and untracked M16 milestone
  docs. This is expected — **skip Pre-flight step 4 and do not investigate
  tree state.**
- The full `cargo test` (non `--lib`) shows one pre-existing failure
  (`hooks_land_on_private_server`) that is NOT yours — the gate for this
  phase is `cargo test --lib`, per the amended acceptance criterion.

### Update — 2026-08-17 (end-to-end verification)

Verbatim output of the § End-to-end verification block (`/tmp/e2e-01.txt`):

```text
1
1
1

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 10 filtered out; finished in 0.00s

exit=0

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 10 filtered out; finished in 0.00s

exit=0

test result: ok. 1306 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 4.13s

exit=0
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.07s
exit=0
```

PASTE MATCH

