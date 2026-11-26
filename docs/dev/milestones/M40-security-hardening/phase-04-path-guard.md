# Phase 04 — path-guard

**Milestone:** M40 — Security hardening
**Status:** todo
**Drafted:** 2026-11-25
**Depends on:** none
**Estimated diff:** ~250 lines
**Tags:** language=rust, kind=bugfix, size=s

## Goal

Audit row **S7**. `read_file` and `edit_file` guard two places: credential
files (`is_blocked_secret_path`) and the daemon's own `~/.daemoneye/`
(`in_daemoneye_dir`, `is_daemoneye_config`, the `sre.toml` read block).
Both guards are blind to symlinks:

- **`is_blocked_secret_path`** checks only the path as given. It never
  resolves it.
- **`resolve_path_for_guard`**, which the `~/.daemoneye` guards use,
  canonicalizes the path or its parent, and nothing deeper. If two
  components are missing, it falls back to the lexical path.

The architect **reproduced both** on 2026-11-25 with a scratch test, since
reverted. Under a temp `HOME`:

- **Read:** `validate_read_file_request` of a symlink
  `innocent.txt → ~/.ssh/id_rsa` returned `None`, so the read was allowed.
- **Read:** a path through a directory symlink `sshdir → ~/.ssh` was
  refused only because the leaf was named `id_rsa`. A leaf such as
  `config` or `known_hosts` would pass.
- **Edit:** `in_daemoneye_dir` returned `false` for `lnk/a/f.txt` and for
  `lnk/a/b/f.txt`, where `lnk → ~/.daemoneye` and `a` does not exist. So
  a single missing directory under the symlink is enough. An
  `operation="create"` there makes the directories and writes inside
  `~/.daemoneye/`.

## Statement of work

All of it is in `src/daemon/executor/file_ops/mod.rs`. The callers in
`read.rs` and `write.rs` do not change.

1. **`resolve_path_for_guard` resolves the deepest existing ancestor.**
   - Walk up from the path to the first ancestor that `canonicalize`
     resolves.
   - Join the missing components back onto it, in their original order.
   - A path that exists resolves exactly as today. The lexical fallback
     stays for a path with no resolvable ancestor.
   - Update the doc comment.
2. **`is_blocked_secret_path` checks the resolved path too.**
   - A path is blocked when **either** the given path or its
     `resolve_path_for_guard` form matches.
   - The `$HOME/.ssh` prefix is compared in both its raw and its
     canonical form. That way a symlinked `HOME` cannot hide it, the same
     reason `guard_roots` keeps both forms of `~/.daemoneye`.
   - The file-name rules (`.env`, `id_rsa`, …) apply to both paths.
3. **Docs.** Add one sentence to the "Path guards on read/edit" bullet in
   `docs/security.md` (§ "What is deliberately NOT a boundary"). It says
   that the guards resolve symlinks, including through directories that
   do not exist yet, and that the credential block checks both the given
   and the resolved path. The bullet still says these guards are not a
   boundary.

**Constraints:**

- No `unsafe`, `.unwrap()`, `.expect()` or `panic!` in production code.
- **Do not change the guards' callers or messages.** Every refusal
  string stays as it is.
- **Tests change `HOME`**, through `TmpHome` / `with_home` in
  `read_tests.rs`, or `crate::test_home_guard()` taken **once** at the
  top of a test. Create `config_dir()` and `$HOME/.ssh` on disk before
  linking to them, since a missing target cannot canonicalize.
- **Gotcha:** `validate_edit_file_request` refuses a ghost first and then
  resolves the host. Call it with `is_ghost = false`, `host: None` and
  `sudo: false`, so the refusal a test sees is the path guard's.

**Closed file list:**
- `src/daemon/executor/file_ops/mod.rs`;
- `src/daemon/executor/file_ops/read_tests.rs`, for the read-side tests
  and the resolver tests, beside `path_guard_follows_symlink_parent_into_config_dir`;
- `src/daemon/executor/file_ops/write.rs`, for its inline test module
  only;
- `docs/security.md`.

Read at most **3** files before your first edit.

**Edit order:** the first red test, then `resolve_path_for_guard`, then
`is_blocked_secret_path`, then the docs.

**Budget.** You have **1200 turns**, so use them efficiently. Your
**first tool call** after reading writes the first red test.

## First red test

`path_guard_resolves_through_missing_dirs_under_a_symlink`, in
`read_tests.rs`.

- **Setup:** `lnk → config_dir()` under the temp `HOME`, with `config_dir()`
  created on disk.
- **Assert:** `resolve_path_for_guard("<lnk>/a/b/f.txt")` starts with the
  canonical `config_dir()`.

It is red today. Paste the red run.

## Acceptance criteria

- [ ] **Resolver:** `path_guard_resolves_through_missing_dirs_under_a_symlink`
      passes. Red run pasted.
- [ ] **Edit guard:** `edit_create_under_a_symlinked_missing_path_is_refused`
      passes, in `write.rs`'s tests. `validate_edit_file_request` with
      `operation = "create"` at `<lnk>/a/b/f.txt`, where `lnk → config_dir()`,
      returns `Err` with the existing "daemoneye configuration directory"
      message. Red run pasted.
- [ ] **Read guard, file link:** `read_file_refuses_a_symlink_to_an_ssh_key`
      passes. `validate_read_file_request` of `innocent.txt → ~/.ssh/id_rsa`
      returns the credential-file refusal. Red run pasted.
- [ ] **Read guard, directory link:**
      `read_file_refuses_a_file_under_a_symlinked_ssh_dir` passes. The path
      is `sshdir/config`, with `sshdir → ~/.ssh` and `~/.ssh/config`
      existing, and it returns the credential-file refusal. Red run pasted.
- [ ] **Must stay allowed:**
      `path_guards_still_allow_ordinary_symlinks_and_missing_dirs` passes.
      These must **not** be refused:
      - `validate_read_file_request` of `notes.txt → <tmp>/real.txt`, an
        ordinary file;
      - `validate_read_file_request` of `<tmp>/config`, a file named
        `config` outside `.ssh`;
      - `in_daemoneye_dir("<tmp>/x/y/z.txt")`, with `x` missing and no
        symlink, is `false`.
- [ ] **Preserved:** these existing tests pass unchanged:
      - `path_guard_follows_symlink_parent_into_config_dir`;
      - `path_guard_allows_nonexistent_leaf_under_real_parent`;
      - every test in `ops_config_tests.rs`, which covers the
        `etc/config.toml` edit through `is_daemoneye_config`.
- [ ] **Mutation check,** run once with the output pasted:
      1. put back the parent-only body of `resolve_path_for_guard`;
      2. run `cargo test --lib path_guard_resolves_through_missing_dirs_under_a_symlink`
         once and see it fail;
      3. restore it.
- [ ] **Gates:** all four pass, run once each. The lib suite reports
      **more than 3143** passed (3143 today).

## Spec

1. **Break down:** write the first red test and run it red. Then write a
   numbered task list into an Update Log entry, one line per task, each
   naming the acceptance item it closes. Done when that entry is written.
2. **Implement:** in the edit order, writing each red test before its fix.
   Done when `cargo build --tests` is green.
3. **Meet the acceptance criteria:** check off every item. Run each
   command **once**. Done when every item is checked with its evidence
   named.
4. **Capture the evidence:** run the four gates, once each. Paste into a
   new Update Log entry:
   - each `test result:` line, without the per-test `… ok` lines;
   - the red runs;
   - the mutation check.

   Commit as one `fix:` commit.

## Authorizations

- You may touch the files in the closed file list, and no others.

## Out of scope

- **Named hosts.** The guards run on the daemon host before the host is
  resolved. A remote path is still resolved against the daemon host's
  filesystem, as today.
- **A dangling symlink in the path** (its target missing). It stays
  unresolved, as today.
- `..` and control-character checks, which already refuse before the
  guards run.
- Name validation for memory namespaces, session ids and prompt names
  (audit S8, phase 05).

## Update Log

<!-- entries appended below this line -->
