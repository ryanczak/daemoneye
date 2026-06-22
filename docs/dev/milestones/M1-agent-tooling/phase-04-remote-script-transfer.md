# Phase 04: Remote Script Execution (ghost `ssh_target`: stream by default, persist for sudo)

**Milestone:** M1 — Agent Tooling Improvements
**Status:** in-progress
**Depends on:** phase-01 (safe SSH quoting / `wrap_remote`), phase-03 (script-name
allowlist — the basename is already `[A-Za-z0-9._-]`-safe by the time it reaches the
remote)
**Estimated diff:** ~200 lines
**Tags:** language=rust, kind=feature, size=m

> **Reopened 2026-06-22.** v1 of this phase (approved first try) materialized the
> script to a **persistent** `~/.daemoneye/scripts/<name>` path on the remote. The
> remote-execution model was then reset (architecture § 2.4): the daemon host is the
> only place DaemonEye stores managed artifacts; **remotes are execution targets, not
> storage targets**, because the daemon may lack remote write privileges, the remote
> FS may be read-only, or its only writable storage may be volatile. Persistent
> materialize can therefore no longer be the **default**. This revision makes
> **streaming** (pipe the script to a remote interpreter's stdin — *no remote disk*)
> the default, and keeps the v1 persistent materialize **only for the `sudo` case**,
> where a NOPASSWD sudoers rule fundamentally requires a fixed authorized path.

## Goal

Run a daemon-host script on a Ghost Shell's `ssh_target` remote host, with the script
content always sourced from the daemon host (never stored on the remote), via two
mechanisms:

- **Default — stream, no remote disk.** Hex-decode the script content on the remote
  and pipe it straight into the shebang-derived interpreter's stdin
  (`… | bash /dev/stdin <args>`). Nothing is written to the remote filesystem, so this
  works on read-only and volatile remotes.
- **Sudo exception — persistent materialize.** When the invocation runs under `sudo`,
  stream is impossible: a NOPASSWD `sudoers` rule authorizes a *fixed path*, and
  neither piped stdin nor a random `mktemp` path can be pre-authorized. So fall back to
  the v1 mechanism: materialize the script to `~/.daemoneye/scripts/<name>` (atomic,
  hex-encoded, `chmod 700`) before running `sudo ~/.daemoneye/scripts/<name>`.

This is functionality + safety; no new tool, IPC type, `PendingCall`, or backend.

## Architecture references

Read before starting:

- `docs/architecture.md#24-remote-host-execution-model` — the daemon-host-storage
  principle and the two remote-script mechanisms (stream default / persist-for-sudo).
  **This is the governing design for this phase.**
- `docs/architecture.md#3-the-ghost-shell-subsystem` — Ghost Shells use
  `GhostPolicy.ssh_target` to wrap approved commands in `ssh <target> …`; scripts run
  under ghost policy. This is the only tool path that opens its own SSH connection
  (interactive file tools go through an existing `target_pane`).
- `docs/dev/milestones/M1-agent-tooling/README.md` — § Notes →
  "Remote-execution model redirection (2026-06-22)" and § "Confirmed findings
  inventory" → **Phase 04**.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above (especially § 2.4).
3. Read this entire phase doc before touching any code.
4. Re-verify the cited line numbers in `src/daemon/policy.rs`,
   `src/daemon/executor/foreground.rs`, and `src/scripts.rs` before editing — the tree
   moves and the numbers below were captured at draft time. **Note:** v1 of this phase
   already added `remote_materialize_cmd` (scripts.rs), `to_hex` (scripts.rs),
   `remote_script_name` (policy.rs), and the foreground wire-in; this revision *adds*
   the stream path and *gates* the persistent path on `sudo` — it does not start from
   scratch. Read what v1 left in those files first.

## Current state (after v1)

v1 shipped these, which this revision builds on:

- `src/scripts.rs` — `fn to_hex(s) -> String` (private), and
  `pub fn remote_materialize_cmd(name, content) -> String` (the persistent
  hex-materialize fragment: `mkdir -p ~/.daemoneye/scripts && {decode} > tmp &&
  chmod 700 && mv -f tmp ~/.daemoneye/scripts/<name>`). **Keep both.**
- `src/daemon/policy.rs` — `GhostPolicy::remote_script_name(cmd) -> Option<String>`:
  returns the basename of a whitelisted, non-absolute, remote (`ssh_target.is_some()`)
  script invocation (after stripping an optional `sudo `). **Keep.**
- `src/daemon/policy.rs` — `GhostPolicy::resolve_command(cmd)` rewrites a whitelisted
  bare name to `~/.daemoneye/scripts/<name>` for the remote case (used for the
  approval display and the sudo path). **Unchanged.**
- `src/daemon/executor/foreground.rs` — the background path detects
  `remote_script_name`, and **after approval, before `wrap_remote`** prepends the
  materialize fragment with `&&`. v1 did this **unconditionally** for every remote
  whitelisted script. **This revision makes that the sudo-only branch and adds the
  streamed branch.**

The hex round-trip idiom (python3 with perl fallback) lives in both
`remote_materialize_cmd` and `file_ops.rs::build_remote_create_cmd`; reuse the *shape*.

## Spec

Pin the behavior below; choose unpinned implementation details. Three files change:
`src/scripts.rs` (new stream builder + interpreter helper + tests),
`src/daemon/policy.rs` (one detection method that also yields args + a test),
`src/daemon/executor/foreground.rs` (branch the post-approval wire-in on `sudo`). Do
**not** change any tool schema, IPC type, `PendingCall` variant, or backend.

### 1. Add `remote_stream_cmd` to `src/scripts.rs`

```rust
/// Build a remote shell fragment that runs `content` (a daemon-host script) on the
/// remote host **without writing it to the remote filesystem**: the hex-encoded
/// content is decoded on the remote and piped straight into the shebang-derived
/// interpreter's stdin via `/dev/stdin`. `args` is the verbatim argument tail from
/// the original invocation (already shell text, e.g. " --flag arg", or empty); it is
/// appended after the interpreter so the script's positional parameters are set.
///
/// Content is hex-encoded, so no byte of the script reaches the remote shell unquoted.
pub fn remote_stream_cmd(content: &str, args: &str) -> String {
    // implementation
}
```

Pinned behavior of the produced string:

- **No remote filesystem write.** The output must contain **no** `mkdir`, no `.de_tmp`,
  no `mv`, and **no redirection of the decoded bytes to a file path** (no `> ~/…`,
  no `> /tmp/…`). The decoded content goes only to a pipe. (Negative property — tested.)
- Decode the hex **content** with the same fallback shape as `remote_materialize_cmd`:
  prefer `python3`, fall back to `perl`, using
  `if command -v python3 >/dev/null 2>&1; then …; else …; fi` (not `||`, so a python3
  that exists-but-errors does not also run perl and double the content). The decode
  writes raw bytes to **stdout**.
- Pipe that stdout into `<interp> /dev/stdin<args>`, where `<interp>` is derived from
  the script's shebang (see § 2 below). `/dev/stdin` lets the interpreter read the
  piped script as if it were a file, so the shebang's interpreter runs the source
  correctly (a Python script runs under `python3`, a bash script under `bash`) — this
  is why streaming does not regress non-shell scripts.

A correct shape (executor may adjust spacing/grouping so long as the pinned behaviors
hold), for a bash script invoked as `foo.sh --flag arg`:

```
{ if command -v python3 >/dev/null 2>&1; then \
    python3 -c "import sys;sys.stdout.buffer.write(bytes.fromhex('<HEX>'))"; \
  else \
    perl -e 'print pack("H*","<HEX>")'; \
  fi; } | bash /dev/stdin --flag arg
```

### 2. Shebang-derived interpreter (private helper in `src/scripts.rs`)

```rust
/// Derive the interpreter command name from a script's shebang line. Returns a name
/// guaranteed to match `[A-Za-z0-9._-]+` (safe to interpolate unquoted into the remote
/// command). Falls back to `"bash"` when there is no shebang or the derived name is
/// not a clean interpreter token.
fn shebang_interpreter(content: &str) -> String {
    // implementation
}
```

Pinned behavior:

- If the first line starts with `#!`: strip `#!`, trim, `split_whitespace`. If the
  first token's basename is `env` and a second token exists, the interpreter is the
  **second** token; otherwise it is the **basename of the first token**.
  - `#!/bin/bash` → `bash`; `#!/usr/bin/env python3` → `python3`;
    `#!/usr/bin/perl -w` → `perl`.
- **Validate** the derived name against `^[A-Za-z0-9._-]+$`. If it does not match
  (e.g. it picked up a `;`, space, quote, or `$`), **fall back to `"bash"`** — never
  emit an unvalidated interpreter token into the remote command.
  - `#!/bin/sh; rm -rf /` → first token basename is `sh;` → fails the charset →
    falls back to `bash`. (Injection negative case — tested.)
- No shebang, or empty content → `"bash"`.

This is the one new place agent-/file-controlled text could reach the remote shell as
a *command word*; the charset gate is the security boundary. Do not skip it.

### 3. Make `remote_script_name` also yield the argument tail

The streamed builder needs the script's args (the tail after the script token). Add a
method that returns both basename and args, and reimplement `remote_script_name` to
delegate (so the detection logic exists once, not twice):

```rust
/// When `ssh_target` is set and `cmd` invokes a whitelisted script (bare or relative
/// name, optionally `sudo`-prefixed, possibly with args), return
/// `(basename, args_tail)` where `args_tail` is everything after the first token
/// (verbatim, including its leading space; empty if none). `None` otherwise. Mirrors
/// `resolve_command`'s whitelist detection exactly.
pub fn remote_script_call(&self, cmd: &str) -> Option<(String, String)> {
    // implementation
}

pub fn remote_script_name(&self, cmd: &str) -> Option<String> {
    self.remote_script_call(cmd).map(|(name, _)| name)
}
```

Pinned: `remote_script_call` returns `Some((name, args))` under **exactly** the same
conditions `remote_script_name` did in v1 (ssh_target set; first token after an
optional `sudo ` is non-empty and not absolute; its basename is in
`auto_approve_scripts`). `args` is the substring of the post-`sudo` command after the
first whitespace-delimited token, taken verbatim (do **not** re-quote or normalize it —
it must round-trip through the outer `sh_single_quote` unchanged, exactly as v1's
materialize-prefixed command did). For `"foo.sh"` → `args == ""`; for
`"sudo foo.sh --flag arg"` → `(name="foo.sh", args=" --flag arg")`.

### 4. Branch the foreground wire-in on `sudo`

In `src/daemon/executor/foreground.rs`, the background path. **Before** the
`resolve_command` block (while `cmd` still holds the original), capture the call and
whether it is sudo:

```rust
// § 2.4 remote execution: a ghost ssh_target whitelisted-script invocation ships to
// the remote either by streaming (default, no remote disk) or — under sudo — by a
// persistent materialize to the sudoers-authorized path.
let remote_script = ghost_policy
    .as_ref()
    .filter(|_| is_ghost)
    .and_then(|p| p.remote_script_call(cmd)); // Option<(String, String)>
let remote_script_is_sudo = remote_script.is_some()
    && (crate::daemon::utils::command_has_sudo(cmd)
        || ghost_policy.as_ref().map(|p| p.run_with_sudo).unwrap_or(false));
```

Keep `resolve_command` and the approval call exactly as today (so the approval prompt
still shows the clean `~/.daemoneye/scripts/foo.sh --flag` form, **not** a hex blob).

**After** approval and **before** the `wrap_remote` block, replace the v1
unconditional materialize-prefix with this branch (read the local content once; the
`Err` arm is the v1 fail-loud advisory — keep it verbatim):

```rust
let remote_built_cmd;
let cmd = if let Some((name, args)) = remote_script.as_ref() {
    match crate::scripts::read_script(name) {
        Ok(content) => {
            remote_built_cmd = if remote_script_is_sudo {
                // Sudo: persistent materialize to the sudoers-authorized path, then
                // run the resolved `sudo ~/.daemoneye/scripts/<name> …` command.
                format!("{} && {}", crate::scripts::remote_materialize_cmd(name, &content), cmd)
            } else {
                // Default: stream content to the interpreter's stdin — no remote disk.
                crate::scripts::remote_stream_cmd(&content, args)
            };
            remote_built_cmd.as_str()
        }
        Err(e) => {
            let msg = format!(
                "Error: cannot run script '{}' on the remote host — it is not \
                 available on the daemon host: {}. Use write_script to create it first.",
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

`wrap_remote` then wraps the whole built command in one `ssh <target> '…'`. Because the
content is hex (no quotes) and the interpreter name is charset-validated, the streamed
pipeline survives `sh_single_quote` intact; the sudo branch is byte-identical to v1.

Notes:
- Reuse the owned-`String`-then-`.as_str()` shadowing already used for `resolved_cmd` /
  `ssh_wrapped_cmd`.
- In the **streamed** branch the resolved-path form produced by `resolve_command` is
  intentionally discarded (the pipeline reads from stdin; there is no remote path).
  That is correct — the resolved form was only needed for the approval display.
- `read_script` validates the name and errors if absent (fail loud, no stale run —
  STANDARDS §2.2 "no fallbacks for if-X-is-missing").

## Acceptance criteria

- [ ] `remote_stream_cmd(content, " --flag arg")` for `content` with a `#!/bin/bash`
      shebang produces a string that: contains the **hex** of `content` and **not** the
      raw `content` bytes; has both a `python3` and a `perl` decode branch; pipes into
      `bash /dev/stdin --flag arg`; and contains **no** `mkdir`, no `.de_tmp`, no `mv`,
      and no `>`-redirection of the decoded bytes to a file (the no-remote-disk
      negative property).
- [ ] `shebang_interpreter` returns `bash` for `#!/bin/bash`, `python3` for
      `#!/usr/bin/env python3`, `perl` for `#!/usr/bin/perl -w`, `bash` for content
      with no shebang, and `bash` for the injection case `#!/bin/sh; rm -rf /`
      (charset gate rejects `sh;`).
- [ ] `remote_script_call` returns `Some(("foo.sh", ""))` for `"foo.sh"`,
      `Some(("foo.sh", " --flag arg"))` for `"foo.sh --flag arg"` and
      `"sudo foo.sh --flag arg"`, `Some(("foo.sh", ""))` for `"./foo.sh"`; and `None`
      for a local policy, `"/usr/bin/foo.sh"`, `"bar.sh"`, and `""`. `remote_script_name`
      still returns the basename for all the `Some` cases (delegation intact).
- [ ] `remote_materialize_cmd` is unchanged and still used for the sudo path (its v1
      tests still pass).
- [ ] `cargo fmt --all`, `cargo build` (zero new warnings),
      `cargo clippy --all-targets --all-features -- -D warnings`, and `cargo test`
      all pass.

## Test plan

Test names are fixed; assertion shape is yours.

**In `src/scripts.rs` `#[cfg(test)] mod tests`** (extend; pure builders — assert on the
returned string directly):

- `remote_stream_cmd_pipes_hex_no_disk` — build with `content = "#!/bin/bash\necho hi\n"`,
  `args = ""`; assert output contains `to_hex(content)`, does **not** contain
  `"echo hi"`, contains `bash /dev/stdin`, and contains **none** of `"mkdir"`,
  `".de_tmp"`, `" mv "`, or a `> ` redirect of the decoded bytes to a path. (no-disk
  negative property.)
- `remote_stream_cmd_passes_args` — with `args = " --flag arg"`, assert the output ends
  the pipeline with `bash /dev/stdin --flag arg`.
- `remote_stream_cmd_python_and_perl_branches` — assert output contains both `python3`
  and `perl`.
- `remote_stream_cmd_honors_shebang` — `content = "#!/usr/bin/env python3\nprint(1)\n"`;
  assert the pipeline interpreter is `python3 /dev/stdin`.
- `shebang_interpreter_cases` — assert the five mappings in the acceptance criterion,
  including the `#!/bin/sh; rm -rf /` → `bash` injection fallback.

**In `src/daemon/policy_tests.rs`** (extend; reuse `policy`, `remote_policy`,
`remote_sudo_policy` helpers):

- `remote_script_call_returns_name_and_args` — for `remote_policy(&["foo.sh"], "h")`:
  `remote_script_call("foo.sh") == Some(("foo.sh".into(), "".into()))`,
  `remote_script_call("foo.sh --flag arg") == Some(("foo.sh".into(), " --flag arg".into()))`,
  `remote_script_call("sudo foo.sh --flag arg") == Some(("foo.sh".into(), " --flag arg".into()))`,
  `remote_script_call("./foo.sh") == Some(("foo.sh".into(), "".into()))`.
- `remote_script_call_none_cases` — `None` for `policy(&["foo.sh"])` (local),
  `"/usr/bin/foo.sh"`, `"bar.sh"`, `""`.
- `remote_script_name_delegates_to_call` — `remote_script_name` returns the basename for
  each `Some` case above (proves the delegation, keeps v1 behavior).

(Keep the v1 `remote_materialize_cmd_*` tests; they still guard the sudo path.)

## End-to-end verification

`remote_stream_cmd`, `shebang_interpreter`, and `remote_script_call` are pure functions
whose return value *is* the artifact (the wire string / the decision). Unit tests assert
those directly (no live SSH host — consistent with phases 01/02/04-v1).

Additionally, **prove the streamed pipeline runs the script on a real interpreter**:
take the `python3` (and `perl`) decode expression `remote_stream_cmd` emits for a known
`#!/bin/bash` script, pipe it through `bash /dev/stdin <args>` locally, and confirm the
script executes with the args set (e.g. a script that echoes `"$1"`). Quote the passing
output of `cargo test remote_stream_cmd`, `cargo test shebang_interpreter`, and
`cargo test remote_script_call`, plus one real local run of the emitted pipeline, in the
completion Update Log. This is the phase-02 reviewer discipline (run the generated
interpreter code) applied proactively.

## Authorizations

- [ ] May add dependencies: **no.** `python3` / `perl` / `bash` run on the *remote* at
      runtime; `to_hex` and `shebang_interpreter` use only `std`.
- [ ] May touch `docs/architecture.md`: **no** (§ 2.4 was already written as part of the
      reopen).

None beyond editing `src/scripts.rs`, `src/daemon/policy.rs`, and
`src/daemon/executor/foreground.rs` (plus their co-located test modules).

## Out of scope

- **Write-tool `target_pane`** (`write_script` / `write_runbook` / `delete_*`). Per
  § 2.4 these are daemon-host-only and gain **no** `target_pane`. The original phase-05
  was dropped; do not add it here.
- **Interactive (non-ghost) remote script execution** — streaming a daemon-host script
  into a remote *user* pane. That is the repurposed phase-05; do not build it here.
- **Scripts that read their own stdin.** Streaming consumes the interpreter's stdin with
  the script source (same limitation as `bash -s`). Do not try to dup an alternate fd;
  out of scope.
- **Removing/cleaning the remote `~/.daemoneye/scripts/` file** left by the sudo path.
- **scp / rsync / a second SSH connection.** Everything rides inside the existing single
  `ssh <target> '…'` invocation.
- **Changing `resolve_command`, `wrap_remote`, or `sh_single_quote`** — correct as-is;
  this phase only adds the stream builder + the args-yielding detection and re-branches
  the post-approval wire-in.
- **The N11 retry path** (`respawn_background_in_pane`) — still does not do remote
  execution; do not add transfer there.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-06-22 03:32 (started, v1)

**Executor:** rexyMCP executor
**Summary:** Implementing remote script transfer for ghost `ssh_target` — hex-encoded materialize prefix prepended to the resolved command before `wrap_remote` wraps it in a single `ssh` invocation.

### Update — 2026-06-22 03:34 (complete, v1)

**Executor:** rexyMCP executor
**Summary:** All three v1 spec tasks implemented and verified.

**Files changed:**
- `src/scripts.rs` — added `to_hex` private helper, `remote_materialize_cmd` public builder, and 4 unit tests
- `src/daemon/policy.rs` — added `GhostPolicy::remote_script_name` method
- `src/daemon/policy_tests.rs` — added 4 unit tests for `remote_script_name`
- `src/daemon/executor/foreground.rs` — wired transfer detection before `resolve_command` and materialize prefix between approval and `wrap_remote`

**Verification commands (all passed):** `cargo fmt --all` clean, `cargo build` zero
warnings, `cargo clippy --all-targets --all-features -- -D warnings` clean, `cargo test`
721 passed.

**End-to-end verification:** hex round-trip verified on real `python3` and `perl`
interpreters; full materialize fragment executed in a sandbox `HOME` (mode-700 atomic
file create, no `.de_tmp` left behind).

### Review verdict — 2026-06-22 (v1)

- **Verdict:** approved_first_try
- **Bounces:** none
- **Executor:** rexyMCP executor (Qwen/Qwen3.6-27B-FP8)
- **Scope deviations:** none
- **Calibration:** none

### Update — 2026-06-22 (reopened by architect)

**By:** Claude (architect)
**Summary:** Phase reopened (done → in-progress) after the remote-execution model was
reset (architecture § 2.4): the daemon host is the only place DaemonEye stores managed
artifacts; remotes are execution targets, not storage targets. v1's persistent
`~/.daemoneye/scripts/<name>` materialize assumed a writable, persistent remote home,
which the new constraints forbid as a default.

**Revision required (see updated Goal/Spec):**
- Add `remote_stream_cmd` + `shebang_interpreter` to `src/scripts.rs` — stream the
  hex-decoded script into `<interp> /dev/stdin <args>`, **no remote disk** (the new
  default).
- Add `GhostPolicy::remote_script_call` (basename + args); `remote_script_name`
  delegates to it. (v1's `remote_script_name`/`remote_materialize_cmd` are retained.)
- In `foreground.rs`, branch the post-approval wire-in on `sudo`: **stream** by default,
  **persist (v1 materialize) only under sudo** (sudoers needs a fixed authorized path).

**Notes for executor:** This is a *delta on v1*, not a rewrite. `remote_materialize_cmd`,
`to_hex`, `resolve_command`, and `wrap_remote` stay. Keep the v1 materialize tests. The
new default must write nothing to the remote filesystem — the `remote_stream_cmd_*` tests
pin that as a negative property.
