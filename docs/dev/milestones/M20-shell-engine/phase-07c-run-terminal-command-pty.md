# Phase 07c: `run_terminal_command` on the shell substrate

**Milestone:** M20 — Shell Engine
**Status:** todo
**Depends on:** phase-07a (`client::run_command`), phase-07b
(`shell_for_owner` + the registry in `run_daemon`) — both done.
**Estimated diff:** ~320 lines
**Tags:** language=rust, kind=feature, size=m

## Goal

Make a chat turn's `run_terminal_command` actually run in a daemon-owned PTY
shell when `[execution] backend = "pty"`, and return what the model needs: the
command's **real exit code**, its output **ANSI-annotated and masked**, and a
`command` record in `events.jsonl` that carries the exit code.

This is the first phase where the new substrate does the user's work. With the
default `backend = "tmux"` **nothing changes** — same code path, same output,
byte for byte.

## Architecture references

- `docs/design/daemoneye-2.0.md` § 2.1 — the shell engine, the marker
  protocol, "masked on every read that reaches a model or a tool result".
- `docs/security.md` § 2 — raw at rest, masked on egress. This phase owns the
  M20 masking exit criterion (PE decision 2026-09-06): *"a seeded secret
  appears in the log and not in the tool result."*
- `docs/dev/milestones/M20-shell-engine/README.md` § Exit criteria — the
  real-exit-code criterion this phase makes reachable.

## Pre-flight

- `git rev-parse --abbrev-ref HEAD` → `master`, tree clean.
- `cargo test --lib shell::` and the two counts below must match § Current
  state before you start. If any differs, stop and say so.

## Current state

Counts today: `shell::registry` **17**, `shell::launch` **8**, `shell::pty`
**13**, `shell::log` **13**, `shell::screen` **11**, `shell::host` **8**,
`shell::proto` **5**, `shell::backend` **7**, `shell::client` **6**,
`shell::session` **6**, `daemon::executor::foreground` **13**, `tmux::ansi`
**13**. Full lib suite: **1630 passed; 0 failed; 4 ignored**.

`src/daemon/executor/shell_exec.rs` **does not exist**; `cargo test --lib
daemon::executor::shell_exec::` matches **0** tests today.

**Nothing consumes the registry.** 07b stood one up in `run_daemon` as
`let _shells = …` and stopped there, by design. `grep -rn 'shell_for_owner'
src/ --include=*.rs | grep -v '^src/shell/'` returns nothing.

### The four things this phase composes — all exported, none to be rewritten

```rust
// src/shell/session.rs (07b)
pub async fn shell_for_owner<F, Fut>(
    reg: &ShellRegistry, owner: Owner, max_per_owner: u32,
    ready_timeout: Duration, spawn: F,
) -> anyhow::Result<(ShellId, PathBuf)>;
pub async fn spawn_detached_host(id: ShellId, rows: u16, cols: u16) -> anyhow::Result<()>;

// src/shell/client.rs (07a)
pub async fn run_command(sock: &Path, cmd: &str, shell_name: &str, timeout: Duration)
    -> anyhow::Result<CommandOutcome>;          // { output: Vec<u8>, exit_code: i32 }

// src/shell/registry.rs (06a)
pub enum Owner { Chat(String), Ghost(String), Adopted }

// src/config
// Config.execution.uses_pty() -> bool     (default false — backend = "tmux")
// Config.shells.max_per_owner: u32        (default 5, 0 = unlimited)
```

## Measured facts — executed 2026-09-07, not reasoned about

Probes compiled into the tree, run, then reverted.

**F1 — the whole chain already works end to end, exit code included.** One
`shell_for_owner` (in-process host) + one `client::run_command`:

```
PROBE id=s0 socket=…/var/run/shells/s0.sock
PROBE exit_code=3
PROBE raw_output="AKIAIOSFODNN7EXAMPLE\r\n\u{1b}[31mred line\u{1b}[0m\r\n"
PROBE reuse_same_id=true
PROBE cd_out="/tmp\r\n" exit=0
PROBE cast_contains_secret=true
PROBE cast_len=1995
```

Read this closely — it settles four things:

1. **The exit code is real** (`3`), and it arrives without any polling.
2. **The output is raw**: CRLF line ends from the PTY line discipline, and
   **raw ESC bytes**. It is *not* fit to hand a model as-is.
3. **The command is not echoed back.** `parse_outcome` returns the bytes
   strictly between the markers, so there is no `extract_command_output`
   equivalent to write.
4. **The `.cast` contains the secret in the clear** (1995 bytes, raw at rest),
   which is exactly what the masking criterion needs on the log side.

**F2 — `exit 3` kills the shell; a per-command exit code must come from a
child.** The first version of the probe used `…; exit 3` and **hung until the
15 s timeout**, because `exit` terminates the bash session and the end marker
never arrives. `sh -c 'exit 3'` (or `false`) is the shape that yields a
non-zero code with the shell still alive. Use that in tests, and note the
consequence for behaviour: a shell the user's command kills is dropped by
`shell_for_owner`'s `host_is_alive` check on the *next* call and replaced —
which 07b already tests.

**F3 — `annotate_ansi` is the sanitiser, not just the annotator.** Measured on
the raw bytes from F1:

```
PROBE normalize={"AKIAIOSFODNN7EXAMPLE\n\u{1b}[31mred line\u{1b}[0m"}   <- ESC survives
PROBE annotate={"AKIAIOSFODNN7EXAMPLE\n[ERROR: red line]\n"}            <- ESC gone, CRLF folded
PROBE tui_annotated={"beforeafter\n"}          <- from "before\x1b[2J\x1b[H\x1b[?25lafter\r\n"
PROBE colored_secret_annotate_then_mask={"[ERROR: <AWS_KEY>]\n"}
```

`normalize_output` alone leaves ESC bytes in the model's input.
`annotate_ansi` converts colour to `[ERROR: …]` / `[WARN: …]` / `[OK: …]`
**and drops every other control sequence** (`\x1b[2J`, `\x1b[H`, `\x1b[?25l`
all vanish) **and** folds CRLF to LF. So it must run **first**, before
masking — otherwise a control byte embedded mid-token could split a secret
past its regex.

**F4 — the built-in masking pattern to seed with.**

```
PROBE mask_akia="key <AWS_KEY> here"      # AKIA[0-9A-Z]{16} -> <AWS_KEY>
PROBE mask_pw="<REDACTED>"                # password=… -> <REDACTED>
```

`AKIAIOSFODNN7EXAMPLE` masks with no config and no `init_masking` call. Use
it as the seeded secret; do not invent a pattern.

**F5 — today's foreground tool result is *not* ANSI-annotated.**
`tmux::capture_pane` (`src/tmux/pane.rs:131`) runs `capture-pane -p -t … -S`
with **no `-e`**, so tmux strips the escapes before we see them. The PTY gives
us the escapes for the first time, which is why annotation is this phase's job
and not a no-op. `annotate_ansi` is `pub(super)` in `src/tmux/ansi.rs:50` —
**not reachable** from `src/daemon/`. The existing way across that boundary is
a public wrapper: `src/tmux/pane.rs:671`

```rust
/// `read_pane` (M12 D3) uses this: `annotate_ansi` is private to `crate::tmux`,
/// so the conversion has to happen on this side of the module boundary.
pub fn capture_pane_annotated(pane_id: &str, depth: usize) -> Result<String> {
    let raw = capture_pane_with_escapes(pane_id, depth)?;
    Ok(super::ansi::annotate_ansi(&raw))
}
```

Do the same shape for a pane-free input (Task 2) rather than widening
`annotate_ansi`'s visibility.

**F6 — `log_command` has no exit code.** `src/daemon/utils/event_log.rs:291`
writes `session / mode / pane / cmd / status / out` and nothing else. Measured
call sites: **11** — nine in `src/daemon/executor/foreground.rs`, two in
`src/daemon/executor/mod.rs`. Adding a parameter is an eleven-site breaking
change for one caller's benefit; add a sibling instead (Task 3).

**F7 — `uses_pty()` defaults false** (`PROBE uses_pty_default=false`,
`PROBE max_per_owner=5`), so every negative criterion in this phase is the
default path.

## Spec

### Task 1 — `src/daemon/executor/shell_exec.rs` (new file)

```rust
/// Run `cmd` in `owner`'s PTY shell and format the result for the model.
/// Returns the text to send as the tool result, and the shell's id.
pub(super) async fn run_in_shell(
    reg: &ShellRegistry,
    owner: Owner,
    cmd: &str,
    max_per_owner: u32,
) -> anyhow::Result<(ShellId, ShellRunResult)>;

/// What one command produced, already fit for a model.
pub(super) struct ShellRunResult {
    /// Annotated, masked, normalized. Ready to send as the tool result.
    pub text: String,
    pub exit_code: i32,
}
```

Sequence:

1. `shell_for_owner(reg, owner, max_per_owner, SHELL_READY_TIMEOUT, |id| spawn_detached_host(id, ROWS, COLS))`.
2. `client::run_command(&socket, cmd, &login_shell(), LOCAL_CMD_TIMEOUT)`.
3. Decode the bytes **once, at the end**: `String::from_utf8_lossy(&outcome.output)`.
   Never decode a partial read — that is `client.rs`'s problem and it already
   solved it.
4. Format, in exactly this order (F3):
   ```rust
   let text = mask_sensitive(&normalize_output(&crate::tmux::annotate_output(&raw)));
   ```
5. Append the non-zero note with the **existing** helper — do not write a
   second one: `foreground::exit_status_annotation(Some(outcome.exit_code))`
   (`src/daemon/executor/foreground.rs:1309`; make it `pub(super)`). It
   returns `None` for `Some(0)`, so a clean command reads exactly as its
   output.

Constants in this file: `SHELL_READY_TIMEOUT` = 15 s, `ROWS` = 24, `COLS` =
80. Reuse `foreground::LOCAL_CMD_TIMEOUT` (45 s, `foreground.rs:36`) for the
command itself — make it `pub(super)`.

**`login_shell()`** — add to `src/shell/session.rs` and call it from
`spawn_detached_host` too, so there is **one** source of truth:

```rust
/// The shell a host is spawned with, and the same name `client::run_command`
/// must be told so it reads the right exit variable (`$?` vs fish's
/// `$status`). One function so the two can never disagree.
pub fn login_shell() -> String {
    std::env::var("SHELL").unwrap_or_else(|_| "bash".to_string())
}
```

### Task 2 — `src/tmux/mod.rs`: one public wrapper

```rust
/// Convert ANSI colour to semantic markers and strip every other control
/// sequence, for output that did not come from a pane. Same conversion
/// `capture_pane_annotated` applies; `annotate_ansi` itself is private to
/// this module.
pub fn annotate_output(s: &str) -> String
```

One line of body. **Do not** change `annotate_ansi`'s visibility or its
behaviour.

### Task 3 — `src/daemon/utils/event_log.rs`: the shell command record

```rust
/// A `command` event for a shell-substrate run. Same shape as `log_command`
/// plus the real exit code, which the tmux path cannot supply.
pub fn log_shell_command(
    session_id: Option<&str>,
    shell_id: &str,
    command: &str,
    status: &str,
    exit_code: i32,
    output_excerpt: &str,
)
```

It writes event `"command"` with `mode` = `"shell"`, `pane` = the shell id,
and an **`exit`** field carrying `exit_code`. Reuse `log_command`'s newline
folding and 200-char excerpt truncation. **Do not modify `log_command` or any
of its ten call sites** (F6).

### Task 4 — route it, in `src/daemon/executor/mod.rs` only

In the `PendingCall::Foreground` arm, **after** the existing ghost gate and
**before** `foreground::run_foreground(…)`, take the shell path when **all**
of these hold:

- `Config::load().unwrap_or_default().execution.uses_pty()`, and
- `!is_ghost`, and
- `session_id.is_some()`, and
- `target` is `None` or `Some("")`.

Anything else falls through to `run_foreground` unchanged. The approval gate
still runs first — call `prompt_and_await_approval` with
`ApprovalRequest { id, cmd, background: false, target_pane_hint: None }`
exactly as the tmux path does, and on `Err(outcome)` return it untouched.

After the run: `stats::finish_command(cmd_id, exit_code)`, send
`Response::ToolResult`, `log_shell_command(session_id, id.as_str(), cmd,
"approved", exit_code, &text)`, return `ToolCallOutcome::Result(text)`.

Owner is `Owner::Chat(session_id.to_string())`.

The registry comes from a process-global accessor — **Task 5**.

**On error** (`shell_for_owner` fails, or `run_command` times out): return a
one-line message naming the shell id when there is one, and say the command
may still be running. Do **not** fall back to the tmux path — a silent
fallback would make `backend = "pty"` untestable.

### Task 5 — `src/shell/mod.rs`: the daemon's one registry

```rust
static REGISTRY: OnceLock<ShellRegistry> = OnceLock::new();

/// The daemon's single live shell registry. `ShellRegistry` is `Default` and
/// holds its own `Arc<Mutex<…>>`, so this needs no initialisation step and
/// has no ordering hazard: the first caller creates it, everyone gets the
/// same one.
pub fn registry() -> &'static ShellRegistry { REGISTRY.get_or_init(ShellRegistry::default) }
```

`run_daemon` (`src/daemon/mod.rs:736`) then sweeps and adopts **into this
one** instead of its own `ShellRegistry::default()`; keep the `uses_pty()`
gate and the two `info!` logs exactly as they are.

**Why a global and not a threaded parameter.** `cache: &Arc<SessionCache>`
reaches `execute_tool_call` through `AskCtx` → `ConversationLoopCtx` →
`execute_tool_call`, plus a second chain through `trigger_ghost_turn` →
`do_ghost_turn`. Threading a second handle the same way is a ~10-site
breaking type change across five files for one consumer. The daemon already
keeps its process-lifetime singletons as globals — `BG_DONE_TX`,
`FG_HOOK_COUNTER`, `DAEMON_START`, `BUFFER_COUNTER` (CLAUDE.md § "Global
statics in daemon"). This is one more of those.

**`run_in_shell` still takes `&ShellRegistry`**, so tests pass their own and
never touch the global. Only the call site in `execute_tool_call` passes
`crate::shell::registry()`.

## Acceptance criteria

Each was run against the current tree and returns the "now" value shown.

- [ ] `test -f src/daemon/executor/shell_exec.rs && echo yes` → **yes**
      (now: absent).
- [ ] `grep -c 'mod shell_exec;' src/daemon/executor/mod.rs` → **1** (now `0`).
- [ ] `grep -c 'pub(super) async fn run_in_shell' src/daemon/executor/shell_exec.rs`
      → **1**.
- [ ] **One source of truth for the shell name:**
      `grep -c 'pub fn login_shell' src/shell/session.rs` → **1** (now `0`),
      and `grep -c 'SHELL' src/shell/session.rs` → **1** (now `1` — the count
      must not grow: `spawn_detached_host` calls `login_shell()` rather than
      keeping its own `env::var("SHELL")`).
- [ ] **The formatting pipeline is composed, not reimplemented:** in
      `src/daemon/executor/shell_exec.rs`,
      `grep -c 'annotate_output' …` → **1 or more**,
      `grep -c 'mask_sensitive' …` → **1 or more**,
      `grep -c 'normalize_output' …` → **1 or more**, and
      `grep -c 'exit_status_annotation' …` → **1 or more**.
- [ ] **`annotate_ansi` did not change visibility:**
      `grep -c 'pub(super) fn annotate_ansi' src/tmux/ansi.rs` → **1**, and
      `grep -c 'pub fn annotate_output' src/tmux/mod.rs` → **1** (now `0`).
- [ ] **The exit code reaches the event log:**
      `grep -c 'pub fn log_shell_command' src/daemon/utils/event_log.rs` → **1**
      (now `0`), and `grep -c '"exit"' src/daemon/utils/event_log.rs` → **1 or
      more** (now `0`).
- [ ] **`log_command` and its call sites are untouched (F6):**
      `grep -c 'pub fn log_command' src/daemon/utils/event_log.rs` → **1**, and
      `grep -c 'log_command(' src/daemon/executor/foreground.rs` → **9**
      (now `9`) and `grep -c 'log_command(' src/daemon/executor/mod.rs` → **2**
      (now `2`) — *preservation* criteria.
- [ ] **The registry is a global with one accessor:**
      `grep -c 'pub fn registry' src/shell/mod.rs` → **1** (now `0`), and
      `grep -c 'ShellRegistry::default()' src/daemon/mod.rs` → **0** (now `1`).
- [ ] **Routing is gated on all four conditions:** in
      `src/daemon/executor/mod.rs`, `grep -c 'uses_pty' …` → **1 or more**
      (now `0`) and `grep -c 'run_in_shell' …` → **1 or more** (now `0`).
- [ ] No `unsafe` in non-test code:
      `awk '/^#\[cfg\(test\)\]/{exit} {print}' src/daemon/executor/shell_exec.rs | grep -vE '^\s*(//|///|//!|\*)' | grep -c 'unsafe'`
      → **0**.
- [ ] No `unwrap`/`expect`/`panic!` outside test code:
      `awk '/^#\[cfg\(test\)\]/{exit} {print}' src/daemon/executor/shell_exec.rs | grep -cE '\.(unwrap|expect)\(|panic!\('`
      → **0**.
- [ ] `cargo test --lib daemon::executor::shell_exec::` reports **5 or more**
      passing, `0 failed` (now: `0 passed`).
- [ ] Unchanged: `shell::registry::` **17**, `shell::launch::` **8**,
      `shell::pty::` **13**, `shell::log::` **13**, `shell::screen::` **11**,
      `shell::host::` **8**, `shell::proto::` **5**, `shell::backend::` **7**,
      `shell::client::` **6**, `shell::session::` **6**,
      `daemon::executor::foreground::` **13**, `tmux::ansi::` **13**.
- [ ] `cargo test --lib` reports `0 failed` on **three consecutive runs** at
      the default thread count — no `RUST_TEST_THREADS` cap, no skip filter.
      *(bug-07a-1: a phase's own PTY-spawning tests turned this gate red 19
      runs in 20 while every per-module count above stayed green.)*
- [ ] All four gates pass.

## Test plan

Names pinned; placement is not. Tests begin `shell_exec_`.

**Any test that spawns a PTY must hold `crate::test_home_guard()` across the
spawn** (bug-05c-1), bound to a named variable (`_home`, not bare `_`), and
must substitute the spawn closure with an in-process host — `spawn_detached_host`
cannot run under a test (07b F4). `src/shell/session.rs`'s test module has the
in-process host helper to copy.

**Use `sh -c 'exit 3'`, never a bare `exit 3`** (F2) — a bare `exit` kills the
shell and the marker never arrives, so the test hangs until the timeout rather
than failing.

- `shell_exec_returns_the_real_exit_code` — **the primary-use test.** A real
  in-process host; a command that both prints and fails
  (`printf 'HELLO\n'; sh -c 'exit 3'`). Assert `exit_code == 3` **and** that
  the text contains `HELLO` **and** the non-zero note. Asserting only the code
  would pass an implementation that dropped the output.
- `shell_exec_masks_a_secret_that_stays_raw_in_the_cast` — **the M20 masking
  exit criterion.** Seed `AKIAIOSFODNN7EXAMPLE` (F4) through a real host whose
  `.cast` path the test knows: assert the returned text does **not** contain
  it and **does** contain `<AWS_KEY>`, and that the `.cast` file on disk
  **does** contain it verbatim. Both halves, in one test — the criterion is
  the pair, not either side.
- `shell_exec_annotates_colour_and_strips_control_bytes` — a command printing
  `\033[31mred\033[0m` and a `\033[2J` clear: the text contains
  `[ERROR: red]` and contains **no** `\x1b` byte at all.
- `shell_exec_reuses_the_owners_shell_across_calls` — two `run_in_shell` calls
  for the same owner: `cd /tmp` then `pwd` returns `/tmp` (F1 measured this
  through `shell_for_owner`; this asserts the executor layer preserves it),
  and both calls report the same `ShellId`.
- `shell_exec_pipeline_is_annotate_then_mask_then_normalize` — a pure test, no
  PTY: feed the exact F3 byte string
  (`"AKIAIOSFODNN7EXAMPLE\r\n\x1b[31mred line\x1b[0m\r\n"`) to whatever
  function does step 4 and assert it equals
  `"<AWS_KEY>\n[ERROR: red line]"`. This is the regression guard for the
  ordering; if the pipeline is inlined, extract it so this test can reach it.
- `log_shell_command_writes_the_exit_code` — write one record under a temp
  `HOME`, read the last `events.jsonl` line, assert `event == "command"`,
  `mode == "shell"` and `exit == 3`.

## End-to-end verification

Run this block verbatim from the repo root. It writes `/tmp/e2e-07c.txt`.

```sh
{
echo "== A. build =="
cargo build 2>&1 | tail -2; echo "cargo_exit=${PIPESTATUS[0]}"
echo "== B. named tests (each line is one pinned test) =="
cargo test --lib 2>&1 | grep -E "^test .*(shell_exec_|log_shell_command_).* ok$" | sed 's/^test //' | sort
echo "cargo_exit=${PIPESTATUS[0]}"
echo "== C. shell_exec totals =="
cargo test --lib daemon::executor::shell_exec:: 2>&1 | grep -E "^test result:"; echo "cargo_exit=${PIPESTATUS[0]}"
echo "== D. every other module unchanged (17,8,13,13,11,8,5,7,6,6 / fg 13 / ansi 13) =="
for m in shell::registry shell::launch shell::pty shell::log shell::screen shell::host shell::proto shell::backend shell::client shell::session daemon::executor::foreground tmux::ansi; do
  printf '%-30s ' "$m"
  cargo test --lib $m:: 2>&1 | grep -E "^test result:"; echo "cargo_exit=${PIPESTATUS[0]}"
done
echo "== E. lib suite, three consecutive runs =="
for r in 1 2 3; do cargo test --lib 2>&1 | grep -E "^test result:" | head -1; done
echo "== F. structural greps (each must print the stated number) =="
echo -n "shell_exec.rs exists       (1): "; test -f src/daemon/executor/shell_exec.rs && echo 1 || echo 0
echo -n "mod shell_exec declaration (1): "; grep -c 'mod shell_exec;' src/daemon/executor/mod.rs
echo -n "run_in_shell declared      (1): "; grep -c 'pub(super) async fn run_in_shell' src/daemon/executor/shell_exec.rs
echo -n "login_shell declared       (1): "; grep -c 'pub fn login_shell' src/shell/session.rs
echo -n "SHELL named once           (1): "; grep -c 'SHELL' src/shell/session.rs
echo -n "annotate_output used     (>=1): "; grep -c 'annotate_output' src/daemon/executor/shell_exec.rs
echo -n "mask_sensitive used      (>=1): "; grep -c 'mask_sensitive' src/daemon/executor/shell_exec.rs
echo -n "normalize_output used    (>=1): "; grep -c 'normalize_output' src/daemon/executor/shell_exec.rs
echo -n "exit_status_annotation   (>=1): "; grep -c 'exit_status_annotation' src/daemon/executor/shell_exec.rs
echo -n "annotate_ansi still private(1): "; grep -c 'pub(super) fn annotate_ansi' src/tmux/ansi.rs
echo -n "annotate_output wrapper    (1): "; grep -c 'pub fn annotate_output' src/tmux/mod.rs
echo -n "log_shell_command declared (1): "; grep -c 'pub fn log_shell_command' src/daemon/utils/event_log.rs
echo -n "exit field in event log  (>=1): "; grep -c '"exit"' src/daemon/utils/event_log.rs
echo -n "log_command still one fn   (1): "; grep -c 'pub fn log_command' src/daemon/utils/event_log.rs
echo -n "foreground log_command     (9): "; grep -c 'log_command(' src/daemon/executor/foreground.rs
echo -n "executor mod log_command   (2): "; grep -c 'log_command(' src/daemon/executor/mod.rs
echo -n "registry accessor          (1): "; grep -c 'pub fn registry' src/shell/mod.rs
echo -n "no local registry default  (0): "; grep -c 'ShellRegistry::default()' src/daemon/mod.rs
echo -n "executor uses_pty gate   (>=1): "; grep -c 'uses_pty' src/daemon/executor/mod.rs
echo -n "executor run_in_shell    (>=1): "; grep -c 'run_in_shell' src/daemon/executor/mod.rs
echo -n "no unsafe pre-test         (0): "
awk '/^#\[cfg\(test\)\]/{exit} {print}' src/daemon/executor/shell_exec.rs | grep -vE '^\s*(//|///|//!|\*)' | grep -c 'unsafe'
echo -n "unwrap/expect/panic pre-test (0): "
awk '/^#\[cfg\(test\)\]/{exit} {print}' src/daemon/executor/shell_exec.rs | grep -cE '\.(unwrap|expect)\(|panic!\('
echo "== G. tmux default is untouched: no shell host started under a fresh HOME =="
T=$(mktemp -d)
HOME="$T" ./target/debug/daemoneye --help >/dev/null 2>&1
echo -n "sockets in the shells dir (0): "
find "$T/.daemoneye/var/run/shells" -name '*.sock' 2>/dev/null | wc -l
echo -n "locks in the shells dir   (0): "
find "$T/.daemoneye/var/run/shells" -name '*.lock' 2>/dev/null | wc -l
find "$T" -depth -delete
echo "== H. no shell-host leaked by the test run =="
echo -n "stray daemoneye shell-host processes (0): "
ps -eo comm,args | awk '$1=="daemoneye" && /shell-host/ {n++} END{print n+0}'
} > /tmp/e2e-07c.txt 2>&1
cat /tmp/e2e-07c.txt
```

Paste the contents of `/tmp/e2e-07c.txt` into your Update Log entry as a fenced
block. **The entry's header must be exactly this line**, or the self-check
below cannot find it:

```
### Update — YYYY-MM-DD HH:MM (end-to-end verification)
```

Then run the self-check and paste its verdict line into the same entry:

```sh
D=docs/dev/milestones/M20-shell-engine/phase-07c-run-terminal-command-pty.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-07c.txt
diff /tmp/pasted-07c.txt /tmp/e2e-07c.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

**Paste the block exactly as it came out, including any section that fails.**
`PASTE MATCH` is not optional and has no "by design" exemption: commentary,
interpretation and any argument that a failure is pre-existing belong in prose
**outside** the fenced block.

**Your prose about section F must state each grep's value as it printed.**
bug-07b-1 was filed on this phase's predecessor for a transcript that honestly
printed `2` under prose claiming "every structural grep prints its pinned
number". The transcript is the evidence; a sentence summarising it is a claim
that has to match.

**Sections B through E can each report success with nothing having run.**
`cargo test --lib daemon::executor::shell_exec::` exits `0` today and prints
`0 passed`. The pass conditions are the named lines in B, **five or more** in
C, and exactly `17, 8, 13, 13, 11, 8, 5, 7, 6, 6, 13, 13` in D.

**Section E runs three times on purpose** (bug-07a-1). Per-module counts cannot
see a cross-module concurrency regression; all three must report `0 failed`.

**Section G is the negative criterion.** It counts sockets and locks, **not**
the directory — the shells directory exists under any fresh `$HOME` because
first-run seeding creates the whole FHS tree.

**A `0` in section F can be an error, not a measurement.** Validated by running
this block against the current tree: with `shell_exec.rs` absent, the `grep -c`
lines print `grep: …: No such file or directory` (exit 2) and the two `awk`
lines print a fatal error **and then a `0` on the next line** — the `0` is
`grep -c` counting an empty stream, not a passing check. Every line of section
F must carry a number on the *same* line as its label.

## Authorizations

You may edit, and nothing else:

- **create** `src/daemon/executor/shell_exec.rs`
- `src/daemon/executor/mod.rs` — `mod shell_exec;` and the routing branch in
  the `PendingCall::Foreground` arm (Task 4)
- `src/daemon/executor/foreground.rs` — **only** to widen
  `exit_status_annotation` and `LOCAL_CMD_TIMEOUT` to `pub(super)`. No other
  line of this 1399-line file changes.
- `src/shell/mod.rs` — the `registry()` accessor and its `use`/`static`
- `src/shell/session.rs` — add `login_shell()`; `spawn_detached_host` calls it
- `src/tmux/mod.rs` — the `annotate_output` wrapper
- `src/daemon/utils/event_log.rs` — add `log_shell_command`
- `src/daemon/mod.rs` — **only** the two lines in `run_daemon` that now take
  the registry from `crate::shell::registry()`
- this phase doc's Update Log

**Do not** change `annotate_ansi`, `log_command`, `client.rs`, `registry.rs`,
`launch.rs`, `host.rs`, `backend.rs`, `pty.rs`, `proto.rs`, `screen.rs`,
`log.rs`, or `run_foreground`'s body.

**Do not run `pkill`, `killall`, `tmux kill-server`, `kill -- -1`, or any
signal to a process group or to PID -1.** Use `find <dir> -depth -delete` for
cleanup, never `rm -rf`.

## Out of scope

- **`background=true`.** The tmux `de-bg-*` path is untouched. Background is
  fire-and-forget with a completion hook; the shell substrate's equivalent
  needs the async notification machinery, and no phase owns it yet — flagged
  for the PE at this phase's close.
- **Ghost sessions.** `is_ghost` falls through to the tmux path this phase.
- **Remote / `target_pane` execution.** A non-empty `target_pane` falls
  through unchanged; remote work becomes SSH shells later (design § 2.2).
- **Interactive commands, signals, pause/resume** — phase-08. A command that
  never returns a marker hits `LOCAL_CMD_TIMEOUT` here and says so.
- **Shell close, GC of exited shells, log retention** — phase-09.
- **The vt100 screen.** This phase annotates the command's own output bytes;
  rendering the live grid through `status.rs` is a separate exit criterion.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->
