# Bug 1 on phase-02: Perl `create` fallback calls unimported `dirname` — branch dies at runtime

**Severity:** major
**Status:** open
**Filed:** 2026-06-22

## What's wrong

The Defect-B fix in `build_remote_create_cmd` (`src/daemon/executor/file_ops.rs`)
adds parent-dir creation to the Perl fallback, but calls `dirname` without
importing it:

```
src/daemon/executor/file_ops.rs:674   "use File::Path qw(make_path);\n\
src/daemon/executor/file_ops.rs:678    make_path(dirname($p));\n\
```

`make_path` is imported from `File::Path`, but `dirname` is provided by
`File::Basename`, which is never imported. In Perl `dirname` is **not** a
builtin, so the generated program dies before creating anything:

```
$ perl <generated-program>
Undefined subroutine &main::dirname called at - line 5.
exit=2
```

The Perl branch is exactly the path taken on a remote host **without Python3**
(`command -v python3` fails → run Perl). So the fix that was meant to make
nested-path `create` work without Python3 instead makes the *entire* Perl
`create` branch fail at runtime — a regression worse than the original
behavior, which at least created the file when the parent dir already existed.

The test `remote_create_cmd_perl_branch_makes_parent_dirs` does not catch this:
it only asserts the source string `contains("File::Path") && contains("make_path")`,
never that the generated Perl is runnable. A passing test over broken Perl
(see STANDARDS.md §5 / review skill §5 — fake test).

## What should happen

Per the phase Spec item 2 and acceptance criterion: "The Perl branch of
`build_remote_create_cmd` contains a recursive parent-directory creation step
before the temp-file open" — and it must actually **run**. The generated Perl
program must execute successfully on a host with Perl but no Python3, creating
intermediate directories (exist-ok) before opening the temp file, matching the
Python branch's `os.makedirs(..., exist_ok=True)`.

## How to fix

In `build_remote_create_cmd` (`src/daemon/executor/file_ops.rs`, the `pl`
format string ≈ line 673), import `dirname` alongside `make_path`. Either:

- `use File::Path qw(make_path); use File::Basename qw(dirname);\n\` — then
  `make_path(dirname($p));` resolves; or
- avoid `dirname` entirely (e.g. derive the parent via a regex/`make_path` on
  the full path is not valid — keep `dirname` and just import it).

Both `File::Path` and `File::Basename` are core Perl modules (no new
dependency; consistent with the phase's Authorizations).

Strengthen the test so it would fail on this bug: assert the generated Perl is
runnable — e.g. pipe the generated program through `perl -c -` (syntax+import
check is insufficient for undefined subs; prefer an actual run) or, minimally,
assert the source imports `File::Basename`/`dirname`. A `perl`-based behavioral
test should be gated on `command -v perl` so it skips cleanly where Perl is
absent (STANDARDS.md §2.6 — missing toolchain is a skip, not a failure).

## Verification

- [ ] `perl -e '<generated create program for a nested path>'` exits 0 and
      creates the nested file (run on a host with Perl; Python3 need not be
      consulted — test the Perl branch directly).
- [ ] The generated Perl source imports both `File::Path qw(make_path)` and
      `File::Basename qw(dirname)` (or otherwise resolves `dirname`).
- [ ] `remote_create_cmd_perl_branch_makes_parent_dirs` is strengthened to fail
      on an unimported `dirname` and passes after the fix.
- [ ] `cargo fmt --all`, `cargo build`, `cargo clippy --all-targets
      --all-features -- -D warnings`, `cargo test` all pass.
