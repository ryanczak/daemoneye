# Phase 04: Remote Script Transfer (ghost `ssh_target` script push)

**Milestone:** M1 — Agent Tooling Improvements
**Status:** review
**Depends on:** phase-01 (safe SSH quoting / `wrap_remote`), phase-03 (script-name
allowlist — the basename is already `[A-Za-z0-9._-]`-safe by the time it reaches
the remote path)
**Estimated diff:** ~150 lines
**Tags:** language=rust, kind=feature, size=m

## Goal

Close the remote-script functionality gap: when a Ghost Shell runs with
`ssh_target` set and invokes a pre-approved script, the script is currently
**rewritten to a remote path that does not exist on the remote host**. The script
file is only ever written to the *daemon host's* `~/.daemoneye/scripts/`; it is
never transferred to the remote. So `ssh <target> '~/.daemoneye/scripts/foo.sh'`
fails with "No such file or directory" — remote script execution is silently
broken.

This phase makes the daemon **materialize the script on the remote host inside the
same `ssh` invocation that runs it**, immediately before execution, so the remote
always has the current version of the approved script. The transfer is hex-encoded
(no shell-injection surface), atomic (temp-file + rename), idempotent (overwrites
on every run), and `chmod 700`. This is **functionality + safety**, not a schema
change: no new tool, no new IPC type, no new `PendingCall` variant.

## Architecture references

Read before starting:

- `docs/architecture.md#3-the-ghost-shell-subsystem` — Ghost Shells use
  `GhostPolicy.ssh_target` to wrap approved commands in `ssh <target> …`; scripts
  run under ghost policy. This is the only tool path that opens its own SSH
  connection (interactive file tools go through an existing `target_pane`).
- `docs/dev/milestones/M1-agent-tooling/README.md` — § "Confirmed findings
  inventory" → **Phase 04 — remote script transfer** (the gap this phase closes),
  and § Notes → "Remote model" (ghost `ssh_target` is the exception that wraps in
  `ssh`).

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.
5. Re-verify the cited line numbers in `src/daemon/policy.rs`,
   `src/daemon/executor/foreground.rs`, and `src/scripts.rs` before editing — the
   tree moves and the line numbers below were captured at draft time.

## Current state

### The gap

`GhostPolicy::resolve_command` (`src/daemon/policy.rs`, ≈ lines 113–133) rewrites a
whitelisted bare/relative script name to a path. For the **remote** case it emits a
**tilde path** that the *remote* shell expands:

```rust
if self.auto_approve_scripts.iter().any(|s| s == basename) {
    let use_sudo = had_sudo || self.run_with_sudo;
    if self.ssh_target.is_some() {
        // Remote execution: use tilde path — the remote shell expands it.
        let remote_path = format!("~/.daemoneye/scripts/{}", basename);
        return if use_sudo {
            format!("sudo {}{}", remote_path, rest)
        } else {
            format!("{}{}", remote_path, rest)
        };
    } else {
        // Local execution: use the absolute path on this machine.
        let full_path = crate::scripts::scripts_dir().join(basename);
        ...
    }
}
```

`wrap_remote` (≈ lines 143–154) then wraps the resolved command in
`ssh <target> '<cmd>'`, safely single-quoted (phase-01):

```rust
pub fn wrap_remote(&self, cmd: &str) -> String {
    match &self.ssh_target {
        Some(target) if !cmd.trim_start().starts_with("ssh ") => {
            format!("ssh {} {}", target, crate::daemon::utils::sh_single_quote(cmd))
        }
        _ => cmd.to_string(),
    }
}
```

But **nothing ever writes the script to the remote** `~/.daemoneye/scripts/`. The
`write_script` tool (`src/daemon/executor/knowledge.rs`, ≈ lines 52–119) calls
`scripts::write_script`, which writes only to the **local** daemon host
(`src/scripts.rs` line 45–56). So the remote tilde path resolves to a file that
isn't there.

### Where execution happens

`src/daemon/executor/foreground.rs`, the **background** execution path (the
function whose tail is ≈ lines 856–964). The relevant sequence today:

```rust
    // Ghost shells: resolve bare/relative script names to absolute path.
    let resolved_cmd;
    let cmd = if let Some(policy) = ghost_policy.as_ref().filter(|_| is_ghost) {
        resolved_cmd = policy.resolve_command(cmd);
        resolved_cmd.as_str()
    } else {
        cmd
    };

    let cmd_id = match prompt_and_await_approval( /* ... approval ... */ ).await? {
        Ok(id) => id,
        Err(outcome) => return Ok(outcome),
    };

    // Ghost shells: wrap the approved command in `ssh <target> <cmd>` when configured.
    let ssh_wrapped_cmd;
    let cmd = if let Some(policy) = ghost_policy.as_ref().filter(|_| is_ghost) {
        ssh_wrapped_cmd = policy.wrap_remote(cmd);
        ssh_wrapped_cmd.as_str()
    } else {
        cmd
    };
    // ... sudo-credential handling (is_ghost short-circuits to None) ...
    let output = run_background_in_window(session_name, id, cmd_id, cmd, ...).await;
```

The transfer must happen **after approval, before `wrap_remote`** — so the
materialize prefix and the script invocation become one compound command that
`wrap_remote` wraps in a single `ssh` call.

### The hex-transfer idiom already in the codebase

Phase-02 established the hex-encode-then-decode-remotely idiom for shipping
arbitrary bytes to a remote host without a shell-injection surface.
`src/daemon/executor/file_ops.rs` has the helper (≈ lines 25–28):

```rust
/// Hex-encode a string (no external crate required).
fn to_hex(s: &str) -> String {
    s.bytes().map(|b| format!("{:02x}", b)).collect()
}
```

and `build_remote_create_cmd` (≈ lines 656–697) decodes hex remotely via a
`if command -v python3 …; else perl …; fi` fallback. **That function is `private`
to `file_ops.rs` and checks file-already-exists (it must *not* overwrite) — this
phase's transfer is the opposite (it *must* overwrite idempotently and `chmod 700`),
so do not reuse it.** Replicate only the *idiom* (hex + python3/perl fallback) in a
new, purpose-built function in `src/scripts.rs`. `to_hex` is trivially small;
duplicate it as a private helper in `scripts.rs` rather than making the
`file_ops.rs` one `pub` (per STANDARDS §2.2 — abstract at the *fourth* caller, not
the second; the cross-module export would also drag `file_ops.rs` into this phase's
diff unnecessarily).

## Spec

Pin the behavior below; choose implementation details where not pinned. Three
files change: `src/scripts.rs` (new pure builder + test), `src/daemon/policy.rs`
(new detection method + test), `src/daemon/executor/foreground.rs` (wire-in). Do
**not** change any tool schema, IPC type, `PendingCall` variant, or backend.

### 1. Add `remote_materialize_cmd` to `src/scripts.rs`

Add a `pub` pure function that builds the remote shell fragment which writes the
script to the remote `~/.daemoneye/scripts/<name>` and makes it executable:

```rust
/// Build a self-contained remote shell fragment that materializes `name` (with
/// the given `content`) into `~/.daemoneye/scripts/<name>` on the remote host,
/// `chmod 700`, atomically (temp file + rename) and idempotently (overwrites any
/// existing copy). Content is hex-encoded so no byte of the script reaches the
/// remote shell unquoted. The fragment exits non-zero on any failure, so it is
/// safe to `&&`-join before the script invocation.
///
/// `name` is assumed already validated to `[A-Za-z0-9._-]` (see
/// `validate_script_name`), so it is safe to interpolate unquoted into the path.
pub fn remote_materialize_cmd(name: &str, content: &str) -> String {
    // implementation
}
```

Pinned behavior of the produced string:

- The remote directory is created first: `mkdir -p ~/.daemoneye/scripts`. Use the
  **tilde** form (`~/.daemoneye/scripts`) so the *remote shell* expands it — this
  matches the tilde path `resolve_command` already emits. (Do **not** hex-encode or
  embed the path inside python/perl — the leading `~` must be seen by the shell, so
  the path stays as literal shell text; only the *content* is hex-encoded.)
- Decode the hex **content** to the temp file using the same fallback shape as
  `build_remote_create_cmd`: prefer `python3`, fall back to `perl`. The decode
  must write the **raw decoded bytes to stdout**, which the shell redirects to the
  temp file — e.g.
  `python3 -c "import sys;sys.stdout.buffer.write(bytes.fromhex('<hex>'))"` and
  `perl -e 'print pack("H*","<hex>")'`. Use the
  `if command -v python3 >/dev/null 2>&1; then …; else …; fi` form (not `||`) so a
  python3 that exists-but-errors does not also run perl and double the content.
- Write to `~/.daemoneye/scripts/<name>.de_tmp`, then `chmod 700` the temp file,
  then `mv -f` it onto `~/.daemoneye/scripts/<name>` (atomic replace; `-f` makes
  it idempotent across repeated runs).
- Chain every step with `&&` so any failure aborts the fragment with a non-zero
  exit (and therefore aborts the subsequent `&&`-joined script invocation —
  a stale or missing script must never run).

A correct shape (the executor may adjust spacing / quoting so long as the pinned
behaviors hold):

```
mkdir -p ~/.daemoneye/scripts && \
if command -v python3 >/dev/null 2>&1; then \
  python3 -c "import sys;sys.stdout.buffer.write(bytes.fromhex('<HEX>'))"; \
else \
  perl -e 'print pack("H*","<HEX>")'; \
fi > ~/.daemoneye/scripts/<name>.de_tmp && \
chmod 700 ~/.daemoneye/scripts/<name>.de_tmp && \
mv -f ~/.daemoneye/scripts/<name>.de_tmp ~/.daemoneye/scripts/<name>
```

Add a private `to_hex` helper to `scripts.rs` (duplicate of the `file_ops.rs` one):

```rust
fn to_hex(s: &str) -> String {
    s.bytes().map(|b| format!("{:02x}", b)).collect()
}
```

### 2. Add `remote_script_name` to `GhostPolicy` in `src/daemon/policy.rs`

Add a method that reports **which** whitelisted script (if any) a command invokes
and therefore needs transferring — using the *same* detection logic as
`resolve_command`, but returning the basename instead of a rewritten command, and
only when remote:

```rust
/// When `ssh_target` is set and `cmd` invokes a whitelisted script (bare or
/// relative name, optionally `sudo`-prefixed, possibly with args), return the
/// script basename that must be transferred to the remote host before execution.
/// Returns `None` for local policies (no `ssh_target`), commands whose first
/// token is already absolute, and commands that do not invoke a whitelisted
/// script. Mirrors `resolve_command`'s whitelist detection exactly.
pub fn remote_script_name(&self, cmd: &str) -> Option<String> {
    // implementation
}
```

Pinned behavior — return `Some(basename)` **iff all** of:
- `self.ssh_target.is_some()`;
- after stripping an optional leading `sudo ` (same `strip_prefix("sudo ")` +
  `trim_start` as `resolve_command`), the first whitespace-delimited token is
  **non-empty** and **not** absolute (does not start with `/`);
- the `Path::file_name` basename of that token is present in
  `self.auto_approve_scripts`.

Otherwise return `None`. Do **not** refactor `resolve_command` to call this (or
vice versa) — duplicating the few lines of detection is acceptable (STANDARDS
§2.2) and keeps `resolve_command`'s settled behavior untouched. The two must agree
on the same inputs (the tests below pin that).

### 3. Wire the transfer into `src/daemon/executor/foreground.rs`

In the background execution path:

1. **Before** the `resolve_command` block (the `let resolved_cmd;` block ≈ line
   857, while `cmd` still holds the *original* command), capture the script to
   transfer:

   ```rust
   let transfer_script: Option<String> = ghost_policy
       .as_ref()
       .filter(|_| is_ghost)
       .and_then(|p| p.remote_script_name(cmd));
   ```

2. **After** approval and **before** the `wrap_remote` block (between ≈ line 882
   and the `let ssh_wrapped_cmd;` block at ≈ line 884), if a transfer is needed,
   read the local script content and prepend the materialize fragment:

   ```rust
   let transfer_prefixed_cmd;
   let cmd = if let Some(name) = transfer_script.as_deref() {
       match crate::scripts::read_script(name) {
           Ok(content) => {
               let prefix = crate::scripts::remote_materialize_cmd(name, &content);
               transfer_prefixed_cmd = format!("{} && {}", prefix, cmd);
               transfer_prefixed_cmd.as_str()
           }
           Err(e) => {
               let msg = format!(
                   "Error: cannot transfer script '{}' to remote host — it is not \
                    available on the daemon host: {}. Use write_script to create it \
                    first.",
                   name, e
               );
               send_response_split(tx, Response::ToolResult(msg.clone())).await?;
               log_command(session_id, "background", "", cmd, "transfer-failed", &msg);
               return Ok(ToolCallOutcome::Result(msg));
           }
       }
   } else {
       cmd
   };
   ```

   The existing `wrap_remote` block then wraps the **whole** compound command
   (`<materialize> && <script invocation>`) in one `ssh <target> '…'` call —
   `wrap_remote` is unchanged. Because the materialize content is hex (no quotes)
   and the script name is allowlisted, `sh_single_quote` handles the wrap safely.

Notes for the wire-in:
- Use the existing owned-String-then-`.as_str()` shadowing pattern already used for
  `resolved_cmd` and `ssh_wrapped_cmd` (so the borrow lives long enough).
- `read_script` already calls `validate_script_name` internally and errors if the
  file is absent — the `Err` arm above surfaces a model-visible advisory and does
  **not** execute, which is the correct "fail loud, no stale run" behavior
  (STANDARDS §2.2 "no fallbacks for if-X-is-missing").
- `log_command` is already in scope in this file (used at ≈ lines 852, 963); reuse
  it. `Response` and `send_response_split` are already imported.

## Acceptance criteria

Verifiable by running the named tests and reading the diff.

- [ ] `GhostPolicy::remote_script_name` returns `Some("foo.sh")` for a remote
      policy whitelisting `foo.sh` given `"foo.sh"`, `"./foo.sh"`, `"sudo foo.sh"`,
      and `"foo.sh --flag arg"`; and returns `None` for: a local policy
      (`ssh_target = None`) even when `foo.sh` is whitelisted, an absolute path
      (`"/usr/bin/foo.sh"`), a non-whitelisted name (`"bar.sh"`), and the empty
      string.
- [ ] On the same inputs where `remote_script_name` returns `Some(name)`,
      `resolve_command` rewrites to a path ending in `~/.daemoneye/scripts/<name>`
      (the two agree about *which* commands invoke a whitelisted remote script).
- [ ] `remote_materialize_cmd("foo.sh", content)` produces a string that: contains
      `mkdir -p ~/.daemoneye/scripts`; contains the **hex** of `content` and does
      **not** contain the raw `content` bytes verbatim (proving injection-safety);
      references `~/.daemoneye/scripts/foo.sh.de_tmp` and `mv`s it to
      `~/.daemoneye/scripts/foo.sh`; contains `chmod 700`; and has both a `python3`
      and a `perl` decode branch.
- [ ] For a `content` containing shell metacharacters (e.g.
      `"echo 'hi'; rm -rf /\n"`), none of those raw characters appear in the
      `remote_materialize_cmd` output outside the hex blob — the only occurrences
      are inside the hex encoding. (must-NOT-leak negative case.)
- [ ] `cargo fmt --all`, `cargo build` (zero new warnings),
      `cargo clippy --all-targets --all-features -- -D warnings`, and `cargo test`
      all pass.

## Test plan

Test names are fixed for auditability; exact assertion shape is yours.

**In `src/daemon/policy_tests.rs`** (extend; it already has `policy`,
`remote_policy`, `remote_sudo_policy` helpers — reuse them):

- `remote_script_name_detects_whitelisted` — positive cases: assert
  `remote_policy(&["foo.sh"], "host").remote_script_name(x) == Some("foo.sh".into())`
  for `x` ∈ {`"foo.sh"`, `"./foo.sh"`, `"sudo foo.sh"`, `"foo.sh --flag arg"`}.
- `remote_script_name_none_when_local` — `policy(&["foo.sh"]).remote_script_name("foo.sh")`
  is `None` (no `ssh_target`).
- `remote_script_name_none_for_absolute_or_unlisted` — `None` for
  `"/usr/bin/foo.sh"`, `"bar.sh"` (not whitelisted), and `""`.
- `remote_script_name_agrees_with_resolve_command` — for an input where
  `remote_script_name` is `Some("foo.sh")`, assert
  `remote_policy(&["foo.sh"], "h").resolve_command("foo.sh")` contains
  `"~/.daemoneye/scripts/foo.sh"`. (The two code paths agree.)

**In `src/scripts.rs`** `#[cfg(test)] mod tests` (extend; pure builder — assert on
the returned string directly, no `with_home` / temp dir needed):

- `remote_materialize_cmd_contains_hex_not_raw` — build with
  `content = "echo secret-token\n"`; assert the output contains
  `to_hex("echo secret-token\n")` and does **not** contain the substring
  `"echo secret-token"`. (must-NOT-leak.)
- `remote_materialize_cmd_has_mkdir_chmod_atomic_mv` — assert the output contains
  `mkdir -p ~/.daemoneye/scripts`, `chmod 700`, `.de_tmp`, and an
  `mv` onto `~/.daemoneye/scripts/foo.sh`.
- `remote_materialize_cmd_has_python_and_perl_branches` — assert the output
  contains both `python3` and `perl` (the decode fallback).
- `remote_materialize_cmd_metachars_stay_hex` — build with
  `content = "x'; rm -rf / #\n"`; assert none of `'`, `;`, `#` from the *content*
  appear outside the hex blob. (Practical assertion: the output split at the hex
  substring has neither half containing the raw dangerous bytes — or simply assert
  the raw content substring is absent and the hex is present, which is sufficient
  to prove the bytes were encoded.)

## End-to-end verification

`remote_materialize_cmd` and `remote_script_name` are pure functions whose return
value *is* the artifact — the wire string sent to the remote shell, and the
transfer decision, respectively. The unit tests assert those return values
directly, which is the real-artifact check (no live SSH host is required or
available — consistent with phases 01 and 02, which verified remote behavior at the
wire-string level).

Additionally, **prove the hex round-trips on a real interpreter** (the remote will
run exactly this): take the `python3` (and, if available, the `perl`) decode
expression that `remote_materialize_cmd` emits for a known content, run it locally,
and confirm it reproduces the original bytes byte-for-byte. Quote the passing
output of `cargo test remote_materialize_cmd` and `cargo test remote_script_name`,
and quote one real round-trip (e.g. piping the emitted `python3 -c "…"` through
`xxd`/`diff` against the original) in the completion Update Log. This is the
phase-02 reviewer discipline (run the generated interpreter code) applied
proactively.

## Authorizations

- [ ] May add dependencies: **no.** `python3` / `perl` run on the *remote* host at
      runtime (not Rust deps); `to_hex` uses only `std`. The remote interpreters
      are the same runtime expectation phase-02 already established for remote file
      ops — no new toolchain dependency is introduced by this phase.
- [ ] May touch `docs/architecture.md`: **no.**

None beyond editing `src/scripts.rs`, `src/daemon/policy.rs`, and
`src/daemon/executor/foreground.rs` (plus their co-located test modules).

## Out of scope

- **Write-tool `target_pane` parity** (`write_script` / `write_runbook` /
  `delete_script` / `delete_runbook` gaining `target_pane`) — Phase 05. No
  tool-schema, `PendingCall`, `AiEvent`, IPC, or backend changes here.
- **The N11 retry path** (`respawn_background_in_pane`, ≈ lines 794–854). It calls
  `resolve_command` but never `wrap_remote`, so it does not currently do remote
  execution at all; do **not** add transfer there. If a reviewer wants remote
  retry, that is a separate follow-up.
- **Removing the script from the remote afterward.** The transfer is push-on-run;
  cleanup of remote `~/.daemoneye/scripts/` is not in scope.
- **scp / rsync.** Do not shell out to `scp`/`rsync` or open a second SSH
  connection — the transfer rides inside the existing single `ssh <target> '…'`
  invocation, which keeps auth, monitoring, and the tmux window identical to today.
- **Changing `resolve_command`, `wrap_remote`, or `sh_single_quote`** — they are
  correct (phase-01). This phase only *adds* the detection method and the
  materialize builder and *prepends* to the resolved command.
- **Binary / non-UTF-8 script content.** `read_script` uses `read_to_string`;
  scripts are text. Do not add a base64/binary fallback.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-06-22 03:32 (started)

**Executor:** rexyMCP executor
**Summary:** Implementing remote script transfer for ghost `ssh_target` — hex-encoded materialize prefix prepended to the resolved command before `wrap_remote` wraps it in a single `ssh` invocation.

### Update — 2026-06-22 03:34 (complete)

**Executor:** rexyMCP executor
**Summary:** All three spec tasks implemented and verified.

**Files changed:**
- `src/scripts.rs` — added `to_hex` private helper, `remote_materialize_cmd` public builder, and 4 unit tests
- `src/daemon/policy.rs` — added `GhostPolicy::remote_script_name` method
- `src/daemon/policy_tests.rs` — added 4 unit tests for `remote_script_name`
- `src/daemon/executor/foreground.rs` — wired transfer detection before `resolve_command` and materialize prefix between approval and `wrap_remote`

**Verification commands (all passed):**
```
$ cargo fmt --all
(no output — clean)

$ cargo build
Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.05s

$ cargo clippy --all-targets --all-features -- -D warnings
Finished `dev` profile [unoptimized + debuginfo] target(s) in 2.53s

$ cargo test
test result: ok. 721 passed; 0 failed; 0 ignored; measured 0 filtered out
```

**One-line verification summary:** `cargo fmt` clean, `cargo build` zero warnings, `cargo clippy` clean, `cargo test` 721 passed.

**End-to-end verification:**
`remote_materialize_cmd` and `remote_script_name` are pure functions — unit tests assert return values directly (the wire string and transfer decision). Hex round-trip verified on real interpreters:
- `python3 -c "import sys;sys.stdout.buffer.write(bytes.fromhex('6563686f207365637265742d746f6b656e0a'))"` → `echo secret-token\n` (confirmed via `xxd`)
- `perl -e 'print pack("H*","6563686f207365637265742d746f6b656e0a")'` → identical output (confirmed via `xxd`)

**Grep for pinned literals:**
```
$ grep -r 'remote_materialize_cmd' src/
  src/scripts.rs: pub fn remote_materialize_cmd(name: &str, content: &str) -> String {
  src/scripts.rs:     let output = remote_materialize_cmd("foo.sh", content);
  src/scripts.rs:     let output = remote_materialize_cmd("foo.sh", "echo hi");
  src/scripts.rs:     let output = remote_materialize_cmd("foo.sh", "echo hi");
  src/scripts.rs:     let output = remote_materialize_cmd("foo.sh", content);
  src/daemon/executor/foreground.rs:                 let prefix = crate::scripts::remote_materialize_cmd(name, &content);
```

```
$ grep -r 'remote_script_name' src/
  src/daemon/policy.rs:    pub fn remote_script_name(&self, cmd: &str) -> Option<String> {
  src/daemon/policy_tests.rs: fn remote_script_name_detects_whitelisted() {
  src/daemon/policy_tests.rs:     assert_eq!(p.remote_script_name("foo.sh"), Some("foo.sh".into()));
  src/daemon/policy_tests.rs:     assert_eq!(p.remote_script_name("./foo.sh"), Some("foo.sh".into()));
  src/daemon/policy_tests.rs:     assert_eq!(p.remote_script_name("sudo foo.sh"), Some("foo.sh".into()));
  src/daemon/policy_tests.rs:     assert_eq!(p.remote_script_name("foo.sh --flag arg"), Some("foo.sh".into()));
  src/daemon/policy_tests.rs: fn remote_script_name_none_when_local() {
  src/daemon/policy_tests.rs:     assert_eq!(p.remote_script_name("foo.sh"), None);
  src/daemon/policy_tests.rs: fn remote_script_name_none_for_absolute_or_unlisted() {
  src/daemon/policy_tests.rs:     assert_eq!(p.remote_script_name("/usr/bin/foo.sh"), None);
  src/daemon/policy_tests.rs:     assert_eq!(p.remote_script_name("bar.sh"), None);
  src/daemon/policy_tests.rs:     assert_eq!(p.remote_script_name(""), None);
  src/daemon/policy_tests.rs: fn remote_script_name_agrees_with_resolve_command() {
  src/daemon/policy_tests.rs:     assert_eq!(p.remote_script_name("foo.sh"), Some("foo.sh".into()));
  src/daemon/executor/foreground.rs:         .and_then(|p| p.remote_script_name(cmd));
```

**Notes for review:** None — implementation follows the spec exactly. No architectural changes, no new dependencies, no build/config edits.
