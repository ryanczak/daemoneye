# Phase 06 — wrap-remote

**Milestone:** M39 — Critical fixes
**Status:** in-progress
**Drafted:** 2026-11-24
**Depends on:** none
**Estimated diff:** ~120 lines, almost all deletions
**Tags:** language=rust, kind=cleanup, size=s

## Goal

The audit (S3d) flags two problems in `GhostPolicy::wrap_remote`
(`src/daemon/policy.rs`): it returns any command starting `ssh ` unwrapped,
and it puts `target` into the command unquoted, with no `--`.

The architect re-verified this at drafting on 2026-11-24 and found the
function is **dead code**. It lost its last production caller when commands
stopped running through tmux. Now:
- A ghost whose `ssh_target` resolves to a configured host runs in a PTY
  shell on that host (`executor/ghost_remote.rs`).
- A target that resolves to no configured host is refused.
- Only tests in `src/daemon/policy_tests.rs` call `wrap_remote`.

Decided at drafting (PE, 2026-11-24): **delete it**, rather than harden a
function nothing calls. Deleting it closes S3d.

## Statement of work

1. Delete `GhostPolicy::wrap_remote` and its doc comment.
2. Delete the tests that exist only to exercise it: the `wrap_remote_*`
   tests in `src/daemon/policy_tests.rs`. For
   `resolve_then_wrap_remote_sudo_prefix`, delete it if another test
   already pins `resolve_command("sudo fix.sh")` under a remote sudo
   policy; otherwise keep its `resolve_command` assertion under a new name
   that does not mention `wrap_remote`.
3. Rewrite the `GhostPolicy` comments that still describe wrapping:
   - the doc comment on the `ssh_target` field;
   - the line in `resolve_command`'s doc comment that defers to
     `wrap_remote()`.

   Say instead what the field does now. It selects the remote route, and
   the command runs in a PTY shell on the `[hosts]` host it resolved to;
   `remote_host` is that host. It also selects the tilde script path.

**Constraints:**

- Keep `remote_script_call`, `resolve_command`, `may_use_ssh_target`,
  `crate::daemon::utils::sh_single_quote` and every other `GhostPolicy`
  item. Each one still has production callers.
- Change no behaviour. This phase is deletions plus comments.
- No new tests; the gates are the evidence.

**Scoping hints.** These files are enough:
- `src/daemon/policy.rs`
- `src/daemon/policy_tests.rs`

**Do not read:** the executor modules, `README.md`, `CLAUDE.md` or the
audit docs. The architect updates the docs and the audit ledger at review.

**Budget.** You have **1200 turns**, so use them efficiently. Make your
first edit within your first **5** tool calls.

## Acceptance criteria

- [ ] `grep -rn 'wrap_remote' src` prints nothing. Today it prints
      matches in `policy.rs` and `policy_tests.rs`.
- [ ] `grep -rn 'ssh <target>' src/daemon/policy.rs` prints nothing. Today
      it matches the `ssh_target` field's doc comment.
- [ ] **Preservation:** every test outside `src/daemon/policy_tests.rs`
      passes unchanged, and every remaining test in that file passes
      unchanged.
- [ ] **Gates:** all four pass. The lib suite count falls by at most 8 from
      3097. Paste the before and after `test result:` lines.

## Spec

1. **Break down:** write a numbered task list into an Update Log entry, one
   line per Statement of work item. Done when the entry is written.
2. **Implement:** delete and rewrite. Done when `cargo build --tests` is
   green.
3. **Meet the acceptance criteria:** run both greps and check off each
   item. Done when every item is checked.
4. **Capture the evidence:** run the four gates and paste each
   `test result:` line, without the per-test `… ok` lines. Commit as one
   `refactor:` commit.

## Authorizations

- You may touch `src/daemon/policy.rs` and `src/daemon/policy_tests.rs`.
  Nothing else.

## Out of scope

- Fencing a ghost's own `ssh …` command against its runbook's `hosts:`
  list. That is egress control; the PE logged it as an observation for the
  sandbox work.
- The audit ledger and the user docs. The architect updates these at
  review.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-11-24 04:54 (started)

Executor: DaemonEye (executor run). Status flipped `todo` → `in-progress`.

Task breakdown (one line per Statement of work item):
1. Break down — this entry: numbered task list into the Update Log.
2. Implement — delete `GhostPolicy::wrap_remote` and its doc comment from `src/daemon/policy.rs`; delete the `wrap_remote_*` tests in `src/daemon/policy_tests.rs`; rewrite the `ssh_target` field doc comment and the `resolve_command` doc line that defers to `wrap_remote()`; handle `resolve_then_wrap_remote_sudo_prefix` per the SOW rule.
3. Meet the acceptance criteria — run both greps (`wrap_remote` and `ssh <target>` in `src/daemon/policy.rs`) and confirm zero matches; confirm no behaviour change.
4. Capture the evidence — run the four gates, paste the `test result:` lines, commit as one `refactor:` commit.
