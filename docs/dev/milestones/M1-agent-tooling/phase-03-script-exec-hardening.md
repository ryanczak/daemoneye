# Phase 03: Script-Exec Hardening (sudoers quoting + script-name allowlist)

**Milestone:** M1 — Agent Tooling Improvements
**Status:** review
**Depends on:** none (self-contained in `src/scripts.rs`; phases 01–02 are unrelated files)
**Estimated diff:** ~90 lines
**Tags:** language=rust, kind=security, size=s

## Goal

Close the two script-execution security holes in `src/scripts.rs`:

1. `validate_script_name` accepts spaces and shell metacharacters — it only
   rejects empty, `/`, NUL, `.`, and `..`. The script name is the
   agent-/user-controlled string that flows into both the filesystem path and
   (via `install_sudoers`) a `/etc/sudoers.d/` rule. Tighten it to a strict
   `[A-Za-z0-9._-]` allowlist.
2. `sudoers_rule` interpolates the script path into a NOPASSWD rule **unquoted
   and unescaped**. A path component containing a sudoers metacharacter (space,
   `,`, `:`, `=`, `(`, `)`, `!`, `@`, `\`) could terminate the command or inject
   a directive. Escape those characters per sudoers syntax.

Both are pure functions with no external I/O, so the fix is small, hermetic, and
fully unit-testable. This is the highest-severity bug remaining after phase-01;
it is pulled early to minimize its exposure window (per the milestone's
"interleave security early" decision). This phase is **security hardening only** —
remote script *transfer* and write-tool `target_pane` parity are separate later
phases (see Out of scope).

## Architecture references

Read before starting:

- `docs/architecture.md#3-the-ghost-shell-subsystem` — scripts run under ghost
  policy / scheduled jobs; `install_sudoers` grants NOPASSWD access to a vetted
  script so a ghost shell or scheduled job can `sudo` it without a TTY.
- `docs/dev/milestones/M1-agent-tooling/README.md` — § "Confirmed findings
  inventory" → **Phase 03 — script-exec hardening** (the two defects this phase
  fixes).

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture reference above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.
5. Re-verify the cited line numbers in `src/scripts.rs` before editing — the tree
   moves and the line numbers below were captured at draft time.

## Current state

Both defects live in `src/scripts.rs`. Line numbers are draft-time anchors; the
described *behavior* is what to match.

### Defect A — `validate_script_name` too permissive (high, security)

The current function (≈ lines 112–121):

```rust
/// Reject names containing path separators or other unsafe characters.
fn validate_script_name(name: &str) -> Result<()> {
    if name.is_empty() {
        bail!("Script name cannot be empty");
    }
    if name.contains('/') || name.contains('\0') || name == "." || name == ".." {
        bail!("Invalid script name: '{}'", name);
    }
    Ok(())
}
```

It rejects path separators, NUL, and the `.`/`..` directory entries, but it
**accepts** names containing spaces, shell metacharacters (`;`, `|`, `&`, `$`,
backticks, `(`, `)`, `<`, `>`, newlines, quotes), and sudoers metacharacters.
This name is later used:

- as a filesystem path component: `scripts_dir().join(script_name)`
  (`install_sudoers`, ≈ line 145), and by `write_script` (≈ line 46) and the
  read/resolve path (≈ line 60) — all three call `validate_script_name` first;
- as the command path in a sudoers rule (via `abs_path_str` → `sudoers_rule`).

A strict allowlist is the primary, well-understood fix: it removes every
character that could be special to a shell, to sudoers, or to path resolution in
one place that all three call sites share.

### Defect B — `sudoers_rule` interpolates the path unescaped (high, security)

The current function (≈ lines 123–129):

```rust
/// Generate the content for a sudoers drop-in file that grants the current user
/// NOPASSWD access to the given script.
///
/// This is a pure function and does not touch the filesystem — useful for testing.
pub fn sudoers_rule(user: &str, script_path: &str) -> String {
    format!("{} ALL=(ALL) NOPASSWD: {}\n", user, script_path)
}
```

`script_path` is the canonicalized absolute path
(`/home/<user>/.daemoneye/scripts/<name>`). After Defect A tightens the *name*,
the name portion is safe by construction, but the **prefix** (home directory,
username) is system-derived and could legitimately contain a sudoers
metacharacter (e.g. a home dir with a space, or an unusual username). `sudoers_rule`
is also `pub` and testable in isolation, so it must be safe for any input.
Defense-in-depth: escape sudoers-special characters in the path so no component
can terminate the command spec or inject a new directive.

#### Reference: sudoers escaping rule (authoritative — executor has no web)

From the sudoers(5) manual (sudo.ws / real-world-systems.com mirror,
verified 2026-06-21):

> "The following characters must be escaped with a backslash (`\`) when used as
> part of a word (e.g. a username or hostname): `@`, `!`, `=`, `:`, `,`, `(`,
> `)`, `\`."

A command pathname in a user spec is such a "word". In addition, sudoers treats
**whitespace** as a token separator, so a space or tab inside a pathname must
also be backslash-escaped (or the value double-quoted). Backslash-escaping each
special character is the simplest correct form and is what this phase pins.

The complete set to escape in the path, in this order (backslash first so the
escapes introduced for the other characters are not themselves re-escaped):

```
\   (literal backslash)  -> \\
(space)                  -> \(space)
\t  (tab)                -> \\t  ... i.e. backslash + the tab character
@ ! = : , ( )            -> each prefixed with a single backslash
```

## Spec

Pin the behavior below; choose the implementation. Keep both fixes minimal and
local to `src/scripts.rs`. Do not change any other file, any tool schema, IPC
type, or the `install_sudoers` process orchestration (the `sudo install` /
`visudo -c` steps stay exactly as they are — only the *rule string* and the
*name validation* change).

1. **Tighten `validate_script_name` to an allowlist (Defect A).** A name is
   valid iff it is non-empty, every character is in `[A-Za-z0-9._-]` (ASCII
   letters, digits, dot, underscore, hyphen), and the whole name is **not** `.`
   or `..`. Reject everything else with the existing `bail!` style
   (`bail!("Invalid script name: '{}'", name)`); keep the distinct empty-name
   message. Do not allow any other character — in particular reject space, `/`,
   NUL, and shell metacharacters. This is a single function body change; the
   three call sites are unaffected (they only care about `Ok`/`Err`).

   Note: this is intentionally stricter than before — a previously-accepted name
   containing a space or metacharacter will now be rejected. That is the desired
   behavior (such names were unsafe); the milestone exit criteria require it.

2. **Escape sudoers-special characters in `sudoers_rule` (Defect B).** Before
   interpolating `script_path` into the rule, transform it so each of the
   characters listed in the Reference subsection above is backslash-escaped.
   Escape the backslash itself first, then the remaining characters and
   whitespace. The `user` field and the surrounding rule text
   (`{user} ALL=(ALL) NOPASSWD: {escaped_path}\n`) are unchanged. Keep the
   function `pub` and side-effect-free. A path containing none of the special
   characters (the common case — a normal home dir + allowlisted name) must come
   out **byte-for-byte identical** to today, so existing callers and the existing
   tests still see the same output.

## Acceptance criteria

Verifiable by running the named tests and reading the diff.

- [ ] `validate_script_name` accepts `check-disk.sh`, `my_script`, `a.b.c`,
      `Backup-01` and rejects (each `is_err()`): `""`, `.`, `..`, `foo bar`
      (space), `foo/bar`, `../etc/passwd`, `foo;rm`, `foo$x`, `foo|bar`,
      `foo\nbar` (newline), `foo\0bar` (NUL), and a name with a backtick.
- [ ] `sudoers_rule` leaves a path with no special characters unchanged
      (byte-for-byte), and backslash-escapes a path containing a space and a
      comma (e.g. `/home/od d/scripts/a,b.sh` → the space and comma appear
      preceded by `\` in the output, and the output still ends in `\n`).
- [ ] The two pre-existing tests `sudoers_rule_content` and
      `sudoers_rule_special_chars_in_path` still pass unchanged (their inputs
      contain no special characters).
- [ ] `cargo fmt --all`, `cargo build` (zero new warnings),
      `cargo clippy --all-targets --all-features -- -D warnings`, and
      `cargo test` all pass.

## Test plan

Unit tests, co-located in the existing `#[cfg(test)] mod tests` block at the
bottom of `src/scripts.rs` (it already imports `crate::util::UnpoisonExt` and has
`validate_*` / `sudoers_rule_*` tests — extend that block). Both functions under
test are pure; assert on their return values directly, no `with_home` / temp dir
needed.

Pin these behaviors (test names are fixed for auditability; exact assertion shape
is yours). **Extend, do not replace,** the existing `validate_rejects_path_traversal`
/ `validate_accepts_normal_names` / `sudoers_rule_content` /
`sudoers_rule_special_chars_in_path` tests — those stay and keep passing.

- `validate_rejects_metacharacters` — the must-NOT-accept negative case. Assert
  `validate_script_name(x).is_err()` for each of: `"foo bar"` (space),
  `"foo;rm -rf"`, `"foo|bar"`, `"foo&bar"`, `"foo$x"`, `"foo>out"`, `"a`b"`
  (backtick), `"foo\nbar"` (embedded newline), `"foo\0"` (NUL), `"foo'bar"`,
  `"foo(bar)"`. (Path-separator and empty cases are already covered by
  `validate_rejects_path_traversal`; `.`/`..` too — you may add `.`/`..`
  explicitly here for clarity.)
- `validate_accepts_allowlisted` — the positive case. Assert `is_ok()` for
  `"check-disk.sh"`, `"my_script"`, `"a.b.c"`, `"Backup-01"`, `"x"`. (Overlaps
  `validate_accepts_normal_names`; both may stay.)
- `sudoers_rule_escapes_special_chars` — the must-escape case. Build a rule with
  a `script_path` containing a space and a comma; assert the resulting string
  contains the space and comma each preceded by a backslash and is **not** the
  naive unescaped interpolation. Also assert a path containing a literal `\`
  yields a doubled `\\` (backslash-escaped first).
- `sudoers_rule_passthrough_when_safe` — the must-NOT-change case. Assert that a
  path with only `[A-Za-z0-9._/-]` (e.g.
  `/home/alice/.daemoneye/scripts/check-disk.sh`) is interpolated byte-for-byte
  identically to the old `format!` output (this is what guarantees the two
  pre-existing `sudoers_rule_*` tests keep passing).

## End-to-end verification

Both `validate_script_name` and `sudoers_rule` are pure functions whose return
value *is* the artifact — the validation decision and the exact bytes written to
`/etc/sudoers.d/daemoneye-<name>` respectively. The unit tests assert those
return values directly, which is the real-artifact check for both (no daemon, no
tmux, no `sudo` invocation required — and none should be added).

In the completion Update Log, quote the passing output of `cargo test validate_`
and `cargo test sudoers_rule` so the negative cases are visible.

## Authorizations

- [ ] May add dependencies: no (use only `std` / the existing `anyhow`
      `bail!`/`Result` already imported in `scripts.rs`).
- [ ] May touch `docs/architecture.md`: no.

None beyond editing `src/scripts.rs`.

## Out of scope

- **Remote script transfer** (the `ssh_target` gap where a script written on the
  daemon host never reaches the remote) — Phase 04. Do **not** touch
  `src/daemon/policy.rs`, `foreground.rs`, or `knowledge.rs` here.
- **Write-tool `target_pane` parity** (`write_script`/`write_runbook`/
  `delete_script`/`delete_runbook`) — Phase 05. No tool-schema, `PendingCall`,
  `AiEvent`, IPC, or backend changes in this phase.
- The `install_sudoers` process orchestration (`sudo install -m 0440`,
  `visudo -c -f`, temp-file handling, the `safe_name` filename sanitizer at
  ≈ lines 171–182) — leave it exactly as is. Only `sudoers_rule` (the string it
  installs) and `validate_script_name` change.
- Quoting/escaping of the `user` field in `sudoers_rule` — the username comes
  from `$USER` / `id -un`, is not agent-controlled, and the runspec position has
  the same escaping rules; this phase only hardens the agent-/path-controlled
  command path. (If trivially in reach, escaping `user` with the same helper is
  acceptable but not required — do not expand scope chasing it.)

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-06-22 02:58 (started)

**Executor:** rexyMCP executor
**Status:** in-progress

Implementing Defect A (allowlist validation) and Defect B (sudoers path escaping) in `src/scripts.rs`.

### Update — 2026-06-22 02:58 (complete)

**Executor:** rexyMCP executor
**Status:** review

**Summary:** Tightened `validate_script_name` to `[A-Za-z0-9._-]` allowlist and added `sudoers_escape_path` to backslash-escape sudoers-special characters in the command path.

**Files changed:**
- `src/scripts.rs` — replaced permissive `validate_script_name` with allowlist; added `sudoers_escape_path` helper; updated `sudoers_rule` to call it; added 4 new unit tests.

**Commits:**
- `fix: harden script-name validation and sudoers path escaping` — one commit covering both defects.

**Verification commands:**
- `cargo fmt --all` — passed
- `cargo build` — zero warnings
- `cargo clippy --all-targets --all-features -- -D warnings` — passed
- `cargo test` — 27 passed, 0 failed, 1 ignored

**End-to-end verification:**
- `cargo test validate_` — 9 passed including `validate_rejects_metacharacters` and `validate_accepts_allowlisted`
- `cargo test sudoers_rule` — 4 passed including `sudoers_rule_escapes_special_chars` and `sudoers_rule_passthrough_when_safe`

**Grep proof — allowlist literal landed:**
```
$ grep -n 'is_ascii_alphanumeric' src/scripts.rs
118:    if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-') {
```

**Grep proof — sudoers escape helper landed:**
```
$ grep -n 'sudoers_escape_path' src/scripts.rs
134:    format!("{} ALL=(ALL) NOPASSWD: {}\n", user, sudoers_escape_path(script_path))
143:fn sudoers_escape_path(path: &str) -> String {
```

**Notes for review:**
- `sudoers_escape_path` is a private helper; only `sudoers_rule` calls it.
- The `user` field in `sudoers_rule` is intentionally left unescaped per spec authorization (username is system-derived, not agent-controlled).
- No other files were touched; `install_sudoers` orchestration is unchanged.
