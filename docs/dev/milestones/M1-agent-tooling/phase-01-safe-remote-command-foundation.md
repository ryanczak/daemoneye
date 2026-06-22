# Phase 01: Safe Remote Command Foundation

**Milestone:** M1 — Agent Tooling Improvements
**Status:** todo
**Depends on:** none
**Estimated diff:** ~90 lines
**Tags:** language=rust, kind=bugfix, size=s

## Goal

Fix the SSH command-injection in `GhostPolicy::wrap_remote` — today it wraps an
agent-supplied command in single quotes with no escaping, so a `'` in the command
breaks out of the quoting and executes on the **daemon host** instead of being
passed intact to the remote shell. Introduce a canonical POSIX single-quote
helper that the later remote phases (02, 03) will reuse, making this the
foundation for safe remote command construction across the milestone.

## Architecture references

Read before starting:

- `docs/architecture.md#3-the-ghost-shell-subsystem` — `wrap_remote` only runs for
  ghost shells with `ssh_target` set; this is the one SSH-invocation point.
- `docs/dev/milestones/M1-agent-tooling/README.md` — § "Confirmed findings
  inventory" → Phase 01 entry.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

`src/daemon/policy.rs` — `wrap_remote` builds the SSH command by interpolating the
command into a single-quoted string with **no escaping**:

```rust
/// Wrap an approved command for remote SSH execution when `ssh_target` is set.
///
/// Called after policy approval, immediately before `run_background_in_window`.
/// Commands that already begin with `ssh ` are returned unchanged to prevent
/// double-wrapping if the AI emits an explicit SSH invocation despite instructions.
pub fn wrap_remote(&self, cmd: &str) -> String {
    match &self.ssh_target {
        Some(target) if !cmd.trim_start().starts_with("ssh ") => {
            format!("ssh {} '{}'", target, cmd)   // ← BUG: cmd is not escaped
        }
        _ => cmd.to_string(),
    }
}
```

The wrapped string is then executed by `run_background_in_window` **on the daemon
host** (`src/daemon/executor/foreground.rs:886`):

```rust
// Ghost shells: wrap the approved command in `ssh <target> <cmd>` when configured.
let ssh_wrapped_cmd;
let cmd = if let Some(policy) = ghost_policy.as_ref().filter(|_| is_ghost) {
    ssh_wrapped_cmd = policy.wrap_remote(cmd);
    ssh_wrapped_cmd.as_str()
} else {
    cmd
};
```

So if `cmd` is `echo '`, the daemon-host shell sees `ssh user@zap 'echo ''` — the
quoting is broken and trailing bytes are parsed by the **local** shell. The
security property we must enforce: the *entire* command reaches `ssh` as a single
argument, so nothing in it is ever interpreted by the daemon-host shell.

### Existing tests (must keep passing, unchanged expectations)

`src/daemon/policy_tests.rs` has these `wrap_remote` tests. Their inputs contain
**no single quotes**, so correct escaping must produce byte-identical output to
today:

```rust
#[test]
fn wrap_remote_wraps_script_in_ssh() {
    let p = remote_policy(&["fix.sh"], "user@zap");
    assert_eq!(
        p.wrap_remote("~/.daemoneye/scripts/fix.sh"),
        "ssh user@zap '~/.daemoneye/scripts/fix.sh'"
    );
}
// ...also: wrap_remote_no_target_returns_unchanged, wrap_remote_wraps_read_only_cmd_in_ssh,
// wrap_remote_no_double_wrap, wrap_remote_sudo_script, resolve_then_wrap_remote_sudo_prefix
```

These pass `remote_policy(scripts, "user@zap")` helpers defined at the top of
`policy_tests.rs`. Tilde paths must still reach the remote unexpanded — single
quotes around `~/...` already prevent *local* expansion; the remote shell expands
it. Correct escaping preserves this exactly.

## Spec

1. **Add a POSIX single-quote helper** — in `src/daemon/utils.rs`, immediately
   after the existing `shell_escape_arg` function (around line 52), add a new
   public function `sh_single_quote(s: &str) -> String`. It wraps the input in
   single quotes and replaces every embedded `'` with the four-character sequence
   `'\''` (close-quote, escaped-quote, reopen-quote). This is the canonical POSIX
   way to single-quote an arbitrary string so a shell parses it as one literal
   token. Exact algorithm:

   ```rust
   /// Single-quote an arbitrary string so a POSIX shell parses it as one literal
   /// token. Wraps in `'…'` and rewrites each embedded `'` as `'\''`. Use this
   /// (NOT `shell_escape_arg`) whenever a value is placed inside single quotes —
   /// e.g. building an `ssh <host> <cmd>` invocation.
   pub fn sh_single_quote(s: &str) -> String {
       format!("'{}'", s.replace('\'', r"'\''"))
   }
   ```

   Do **NOT** reuse `shell_escape_arg` for this. `shell_escape_arg` is built for a
   *double-quote* context (it escapes `\`, `"`, `$`, `` ` ``). Inside single
   quotes those characters are literal, so `shell_escape_arg` would corrupt them
   (e.g. turn `$HOME` into `\$HOME`, which the remote would receive verbatim).

2. **Use the helper in `wrap_remote`** — in `src/daemon/policy.rs`, change the
   interpolation from `format!("ssh {} '{}'", target, cmd)` to use
   `crate::daemon::utils::sh_single_quote(cmd)`:

   ```rust
   Some(target) if !cmd.trim_start().starts_with("ssh ") => {
       format!("ssh {} {}", target, crate::daemon::utils::sh_single_quote(cmd))
   }
   ```

   Leave the `ssh `-prefix double-wrap guard and the no-target fall-through arm
   exactly as they are. The `target` is operator-authored runbook config, not
   agent input — do not escape it in this phase (tracked as out of scope below).

3. **Add unit tests for `sh_single_quote`** — in the `#[cfg(test)]` module of
   `src/daemon/utils.rs`, alongside the existing `shell_escape_arg_*` tests. Cover:
   the no-quote passthrough (`echo hi` → `'echo hi'`), a single embedded quote, and
   a breakout-attempt string. See Test plan for exact assertions.

4. **Add injection regression tests for `wrap_remote`** — in
   `src/daemon/policy_tests.rs`, in the `wrap_remote` test group. Assert the exact
   escaped wire string for a command containing single quotes, proving the whole
   command stays inside one `ssh` argument. See Test plan.

## Acceptance criteria

- [ ] `cargo test sh_single_quote` passes (new helper tests).
- [ ] `cargo test wrap_remote` passes — all existing tests **and** the new
      injection regression tests.
- [ ] `wrap_remote` with a quote-free command produces byte-identical output to
      before (e.g. `~/.daemoneye/scripts/fix.sh` → `ssh user@zap '~/.daemoneye/scripts/fix.sh'`).
- [ ] `wrap_remote` on `echo 'pwned'` returns exactly
      `ssh user@zap 'echo '\''pwned'\'''` (single-quoted as one argument).
- [ ] `cargo fmt --all`, `cargo build`, `cargo clippy --all-targets --all-features
      -- -D warnings`, and `cargo test` all pass.

## Test plan

`sh_single_quote` tests in `src/daemon/utils.rs`:

- `sh_single_quote_plain` — asserts `sh_single_quote("echo hi") == "'echo hi'"`.
- `sh_single_quote_embedded_quote` — asserts
  `sh_single_quote("echo 'pwned'") == r"'echo '\''pwned'\'''"`.
  (Walk it: `'echo '` + `\'` + `'pwned'` + `\'` + `''` → the shell reassembles
  `echo 'pwned'` as one token.)
- `sh_single_quote_breakout_attempt` — asserts
  `sh_single_quote("x'; rm -rf ~ #") == r"'x'\''; rm -rf ~ #'"`. The `;`, `~`, and
  `#` stay inside the quotes, so the daemon-host shell never sees them as syntax.
- `sh_single_quote_dollar_is_literal` — asserts `sh_single_quote("$HOME") ==
  "'$HOME'"` (must NOT become `'\$HOME'` — that is the `shell_escape_arg` mistake
  this helper exists to avoid).

`wrap_remote` regression tests in `src/daemon/policy_tests.rs`:

- `wrap_remote_escapes_single_quote` — `remote_policy(&[], "user@zap")` then assert
  `p.wrap_remote("echo 'pwned'") == r"ssh user@zap 'echo '\''pwned'\'''"`.
- `wrap_remote_escapes_breakout_attempt` — assert
  `p.wrap_remote("x'; rm -rf ~ #") == r"ssh user@zap 'x'\''; rm -rf ~ #'"`.

Negative property the breakout tests pin: there is no point in the output where a
single quote closes the wrapper early and leaves daemon-host shell syntax exposed
— every original `'` is rewritten to `'\''`, so the command is exactly one `ssh`
argument.

## End-to-end verification

Not applicable — phase ships no independently runtime-loadable artifact reachable
without a live remote SSH host + ghost shell. `wrap_remote`'s output *is* the wire
string sent to the daemon-host shell, and the unit tests assert that exact string
byte-for-byte (the real artifact under test). Restate this one-line reason in the
completion Update Log.

## Authorizations

- [ ] May add dependencies: none.
- [ ] May touch `docs/architecture.md`: no.

None beyond editing `src/daemon/policy.rs`, `src/daemon/utils.rs`, and
`src/daemon/policy_tests.rs`.

## Out of scope

- Escaping or validating `ssh_target` itself (operator-authored config, not agent
  input) — a later hardening pass may validate it has no whitespace/quotes.
- The remote command construction in `src/daemon/executor/file_ops.rs` (sed/Python
  injected via send-keys into an existing pane) — that is Phase 02. Do **not** touch
  `file_ops.rs` here, even though it also builds remote commands.
- Remote script *transfer* (`write_script` → remote host) — Phase 03.
- Changing the no-double-wrap guard or the resolve_command tilde logic.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->
