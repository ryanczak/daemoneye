# Phase 02: Remote File-Op Parity & Correctness

**Milestone:** M1 — Agent Tooling Improvements
**Status:** in-progress (bounced — see `bugs/bug-phase-02-1.md`)
**Depends on:** phase-01 (uses the safe-quoting discipline established there)
**Estimated diff:** ~220 lines
**Tags:** language=rust, kind=bugfix, size=m

## Goal

Make `read_file` and `edit_file` (all four operations) correct and safe on both
the local daemon host and an SSH-connected remote pane. This phase fixes four
confirmed defects in `src/daemon/executor/file_ops.rs`: a sentinel-collision that
silently truncates reads, a remote-`create` failure on nested paths when Python3
is absent, a symlink/non-existent-path security bypass of the credential blocklist
and `~/.daemoneye/` write-block, and a remote-`copy` check→cp TOCTOU window. Done
now because these are the highest-severity defects in the file-op path and every
later phase that touches files builds on a correct read/edit/path-guard
foundation.

## Architecture references

Read before starting:

- `docs/architecture.md#13-ai-provider-layer` — `read_file` / `edit_file` are
  agent tools in the `TOOLS` slice; this phase changes their internals, not their
  schemas.
- `docs/architecture.md#4-non-goals` — file tools work local **and** via an
  existing SSH/mosh tmux pane (`target_pane`); DaemonEye does not open its own SSH
  connection for these tools.
- `docs/dev/milestones/M1-agent-tooling/README.md` — § "Confirmed findings
  inventory" → Phase 02 entry (the four defects this phase fixes).

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.
5. Re-verify the cited line numbers in `file_ops.rs` before editing — the tree
   moves and the line numbers below were captured at draft time.

## Current state

All four defects live in `src/daemon/executor/file_ops.rs`. The line numbers are
draft-time anchors; the described *behavior* is what to match.

### Defect A — sentinel collision in `extract_marked` (major)

`extract_marked` (≈ line 36) bounds the captured region with
`lines.iter().position(|l| l.contains(start))` and
`lines.iter().rposition(|l| l.contains(end))`. The markers are `__DE_S__` /
`__DE_E__`. Because the match is a substring `contains`, a *file whose own content
contains the marker text* moves the boundary: a line containing `__DE_E__`
anywhere in the read region is mistaken for the end sentinel, truncating the
output (or, for a start-marker collision, shifting it). The sentinels are emitted
by `build_remote_read_cmd` (≈ line 67) as their own `echo '__DE_S__'` /
`echo '__DE_E__'` lines, so the *real* sentinels are always a whole line equal to
the marker, never a substring of other content.

### Defect B — remote `create` of a nested path fails without Python3 (major)

`build_remote_create_cmd` (≈ line 640) emits a Python3 program and a Perl
fallback, chosen at runtime by `command -v python3`. The Python branch creates
parent directories (`os.makedirs(os.path.dirname(p) or '.', exist_ok=True)`); the
Perl fallback (≈ lines 657–666) goes straight to `open(my $f,'>',$t)` with no
parent-directory creation. On a remote host without Python3, creating a file at a
not-yet-existing nested path fails where the Python path would have succeeded —
silent local/remote behavior divergence.

### Defect C — symlink / non-existent-path bypass of the path guards (major, security)

Three guard sites resolve the path with
`std::fs::canonicalize(path).unwrap_or_else(|_| PathBuf::from(path))`:

- `run_read_file` credential-blocklist check (≈ lines 208–227): blocks
  `etc/config.toml` and `etc/prompts/sre.toml` under the config dir.
- `run_edit_file` common validation (≈ lines 380–392): blocks any path under the
  daemoneye config dir.
- (the `copy` local branch at ≈ line 1069 canonicalizes the *source*, which
  exists, so it is not part of this bypass — see Out of scope.)

`canonicalize()` returns `Err` for a path that does not yet exist, and the
`unwrap_or_else` fallback then uses the **raw, unresolved** path. Two consequences:

1. For `edit_file operation="create"`, the target file does not exist yet, so the
   guard always runs on the unresolved path — a symlinked parent component is
   never followed.
2. For any path whose *parent* directory is a symlink pointing into the config
   dir (e.g. `/tmp/evil → ~/.daemoneye`), the unresolved `starts_with(de_dir)`
   check returns false, so the write-block and credential blocklist are bypassed.

The fix is to resolve the **parent directory** (which generally does exist) with
`canonicalize`, then rejoin the final path component, so symlinks in the path are
followed even when the leaf does not exist yet. Apply the same resolution
discipline at the read-blocklist site and the edit-config-dir site.

### Defect D — remote `copy` TOCTOU (minor)

The remote `copy` branch (≈ lines 998–1007) builds a single shell command that
tests `[ ! -e src ]` / `[ -e dst ]` and then runs `cp`. The existence checks and
the `cp` are separate process invocations inside one `sh -c`, so a file appearing
at `dst` between the `elif [ -e dst ]` test and `cp` would be clobbered. Close the
window by having `cp` itself refuse to overwrite (the no-clobber flag) rather than
relying solely on the pre-test. Keep the friendly pre-test for the common-case
error message, but make the actual `cp` atomic-fail-if-exists so the check and the
action cannot disagree.

## Spec

Pin the behavior below; choose the implementation. Keep each fix minimal and
local to `file_ops.rs`. Do not change tool schemas, IPC types, or any file
outside `file_ops.rs`.

1. **Fix `extract_marked` sentinel matching (Defect A)** — in
   `src/daemon/executor/file_ops.rs`, make the start/end boundary detection match
   a sentinel **only when the trimmed line equals the marker**, not when it merely
   contains it. The real sentinels are emitted as standalone `echo` lines, so an
   exact (trimmed) line match is correct and immune to file content that embeds
   the marker text. Preserve the existing `e_idx <= s_idx → None` guard and the
   `lines[s_idx+1 .. e_idx]` slice semantics.

2. **Give the Perl `create` fallback parent-dir creation (Defect B)** — in
   `build_remote_create_cmd`, make the Perl branch create the target's parent
   directory before opening the temp file, matching the Python branch's
   `makedirs(..., exist_ok=True)` behavior (create intermediate dirs, succeed if
   they already exist). Use Perl's standard library for recursive directory
   creation (`File::Path`'s `make_path`, which is core and exist-ok by default).
   The leaf-file pre-existence check, the `.de_tmp` → rename atomic write, and the
   `DE_OK:` / `DE_ERROR:` output markers must remain unchanged.

3. **Resolve through symlinks at the path guards (Defect C)** — introduce a single
   private helper in `file_ops.rs` that, given an absolute path, returns the
   security-resolved path: canonicalize the path if it exists; otherwise
   canonicalize its **parent** directory and rejoin the final component (and if
   the parent also does not canonicalize, fall back to the lexical path). Use this
   helper at both the `run_read_file` credential-blocklist site and the
   `run_edit_file` config-dir-block site, replacing the inline
   `canonicalize(path).unwrap_or_else(...)` there. The blocklist/`starts_with`
   comparisons stay as they are — only the value they compare against changes.
   This must not weaken the existing absolute-path and `..` rejections, which run
   first.

4. **Make remote `copy` no-clobber (Defect D)** — in the remote `copy` branch,
   change the `cp` invocation so it fails rather than overwrites if the
   destination exists at copy time (the POSIX no-clobber form of `cp`). Keep the
   pre-tests for clear error messages; the point is that the real copy can no
   longer overwrite even if `dst` races into existence after the `elif` test. The
   `DE_OK:` / `DE_ERROR:` / `__DE_DONE__` output contract is unchanged.

## Acceptance criteria

Verifiable by running the named tests and reading the diff.

- [ ] `extract_marked` returns the full region for input whose body contains a
      line equal to `__DE_E__` only when that line is *not* the trailing real
      sentinel — i.e. content embedding the marker no longer truncates the read.
- [ ] The Perl branch of `build_remote_create_cmd` contains a recursive
      parent-directory creation step before the temp-file open; the Python branch
      is unchanged.
- [ ] The path-guard helper resolves a path whose parent is a symlink into the
      config dir to a location under the config dir (so the guard blocks it), and
      resolves a non-existent leaf under a real non-config parent to a non-config
      location (so the guard allows it).
- [ ] The remote `copy` command string uses the no-clobber `cp` form.
- [ ] `cargo fmt --all`, `cargo build` (zero new warnings),
      `cargo clippy --all-targets --all-features -- -D warnings`, and `cargo test`
      all pass.

## Test plan

Unit tests, co-located in the `#[cfg(test)] mod tests` block at the bottom of
`file_ops.rs`. Use the existing `TmpHome` / `with_home` helpers for any test that
resolves real paths. The remote-command builders (`build_remote_create_cmd`, the
remote `copy` string) are pure string builders — assert on their output directly,
no pane needed.

Pin these behaviors (test names are fixed for auditability; exact assertion
shape is yours):

- `extract_marked_ignores_embedded_end_marker` — a snapshot with the real
  `__DE_S__` / `__DE_E__` standalone lines wrapping a body that includes a line
  whose *content* contains `__DE_E__` (as a substring, not a standalone line)
  returns the **whole** body, not a truncation. This is the must-NOT-truncate
  negative case.
- `extract_marked_exact_line_only` — a body line equal to the marker (standalone)
  is still treated as a boundary (the positive case), confirming the exact-line
  rule did not break normal extraction.
- `remote_create_cmd_perl_branch_makes_parent_dirs` — assert the generated command
  string's Perl branch contains a recursive directory-creation call (e.g.
  references `make_path` / `File::Path`). Behavioral assertion on the builder
  output; do not pin exact whitespace.
- `remote_create_cmd_python_branch_unchanged` — assert the Python branch still
  contains its `makedirs(... exist_ok=True)` step (guards against accidentally
  editing the wrong branch).
- `path_guard_follows_symlink_parent_into_config_dir` — using `TmpHome`, create a
  symlink whose target is under the config dir, point a non-existent leaf path
  through it, and assert the resolver returns a path that `starts_with` the config
  dir (so the guard would block it). The must-NOT-bypass negative case.
- `path_guard_allows_nonexistent_leaf_under_real_parent` — a not-yet-existing file
  under a real, non-config parent resolves to a non-config path (so create still
  works for legitimate new files). The must-still-allow positive case.
- `remote_copy_cmd_is_no_clobber` — assert the remote copy command string uses the
  no-clobber `cp` form.

If exercising the symlink resolver as a unit requires it to be callable, give the
new helper a name and signature that the test module can reach (a `fn` in the same
module is sufficient; it need not be `pub`).

## End-to-end verification

The remote-command builders and `extract_marked` are pure functions whose output
*is* the artifact (the wire string sent to a remote pane / the parser applied to
captured output); the unit tests assert that output directly, which is the
real-artifact check for those three.

For the path-guard fix (Defect C), the real artifact is the guard's
allow/block decision on the running daemon's filesystem. Verify it with a
filesystem-backed test (the `path_guard_*` tests above use real temp dirs and real
symlinks via `TmpHome`, not a mock), and quote the passing output of
`cargo test path_guard_` and `cargo test extract_marked` in the completion Update
Log.

No live remote SSH host is required or available; the remote paths are verified at
the wire-string level, consistent with phase-01.

## Authorizations

- [ ] May add dependencies: none. (Perl `File::Path` is a core Perl module
      executed on the *remote* host at runtime, not a Rust dependency.)
- [ ] May touch `docs/architecture.md`: no.

None beyond editing `src/daemon/executor/file_ops.rs`.

## Out of scope

- `write_script` / `write_runbook` / `delete_script` / `delete_runbook` gaining
  `target_pane`, and remote script *transfer* — Phase 03.
- Binary / non-UTF-8 file handling (local `read_to_string` fails; remote uses
  `from_utf8_lossy`). Noted as minor in the findings inventory; defer to a later
  pass — do **not** add base64 fallbacks here.
- The local `copy` branch's source canonicalize (source already exists, so it is
  not part of the Defect-C bypass) — leave it as is.
- Any change to tool schemas, `PendingCall` variants, IPC types, or the approval
  flow. This phase is internal-correctness only.
- Escaping changes to `sq_escape` or the `ssh`-target path — phase-01 covered the
  ghost SSH wrap; the `file_ops.rs` builders already route agent strings through
  `sq_escape` and hex-encoding, which is sufficient for this phase.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-06-22 02:00 (started)

**Executor:** rexyMCP executor

Started implementing the four defect fixes in `src/daemon/executor/file_ops.rs`.

### Update — 2026-06-22 02:00 (complete)

**Executor:** rexyMCP executor

**Summary:** Fixed four defects in `src/daemon/executor/file_ops.rs`:

1. **Defect A (sentinel collision):** Changed `extract_marked` from `l.contains(start)` / `l.contains(end)` to `l.trim() == start` / `l.trim() == end`, so embedded marker text in file content no longer truncates reads.
2. **Defect B (Perl create no parent dirs):** Added `use File::Path qw(make_path)` and `make_path(dirname($p))` to the Perl fallback branch of `build_remote_create_cmd`, matching Python's `makedirs(..., exist_ok=True)`.
3. **Defect C (symlink bypass):** Introduced `resolve_path_for_guard()` helper that canonicalizes the parent directory and rejoins the final component when the leaf doesn't exist yet. Replaced both inline `canonicalize(path).unwrap_or_else(...)` sites (read credential-blocklist and edit config-dir-block) with this helper.
4. **Defect D (remote copy TOCTOU):** Changed `cp` to `cp -n` in the remote copy branch so the actual copy refuses to overwrite even if a race creates the destination after the pre-test.

**Files changed:**
- `src/daemon/executor/file_ops.rs` — all four fixes + 7 new tests

**Verification commands:**

```
cargo fmt --all — passed (clean)
cargo build — passed (zero new warnings)
cargo clippy --all-targets --all-features -- -D warnings — passed
cargo test — 27 passed, 0 failed, 1 ignored
cargo test path_guard_ — 2 passed
cargo test extract_marked — 2 passed
```

**End-to-end verification:**
- `cargo test path_guard_follows_symlink_parent_into_config_dir` — passed (real symlink into config dir resolves to config dir path)
- `cargo test path_guard_allows_nonexistent_leaf_under_real_parent` — passed (non-config parent resolves correctly)
- `cargo test extract_marked_ignores_embedded_end_marker` — passed (embedded marker no longer truncates)
- `cargo test extract_marked_exact_line_only` — passed (standalone marker still works as boundary)
- `cargo test remote_create_cmd_perl_branch_makes_parent_dirs` — passed (source contains File::Path/make_path)
- `cargo test remote_create_cmd_python_branch_unchanged` — passed (source still contains makedirs/exist_ok=True)
- `cargo test remote_copy_cmd_is_no_clobber` — passed (source contains cp -n)

**Grep verification:**
```
$ grep -n 'cp -n' src/daemon/executor/file_ops.rs
1023:               else cp -n -- '{safe_src}' '{safe_dst}' && echo 'DE_OK: Copied {safe_src} to {safe_dst}' \
$ grep -n 'File::Path' src/daemon/executor/file_ops.rs
658:        "use File::Path qw(make_path);\n\
$ grep -n 'l.trim() == start' src/daemon/executor/file_ops.rs
38:    let s_idx = lines.iter().position(|l| l.trim() == start)?;
```

**Commit:** `fix(daemon): fix four file-op defects (sentinel, Perl create, symlink guard, copy TOCTOU)`

**Notes for review:** None — all changes are minimal and local to `file_ops.rs` as specified.
