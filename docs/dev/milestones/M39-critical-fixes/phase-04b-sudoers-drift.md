# Phase 04b — sudoers-drift

**Milestone:** M39 — Critical fixes
**Status:** todo
**Drafted:** 2026-11-23
**Depends on:** phase-04 (done)
**Estimated diff:** ~250 lines
**Tags:** language=rust, kind=feature, size=m

## Goal

After phase 04, a ghost's `sudo <name>` runs a root-owned **copy** made at
`install-sudoers` time, never the user's `~/.daemoneye/scripts/<name>`.
Editing the script therefore changes nothing that runs as root until the
user re-runs `install-sudoers`. That is safe, but silent. This phase makes
the drift between a copy and its source **visible**, in three places. It
never syncs the copy and never refuses a call because of drift.

## Statement of work

**PE decision (2026-11-23, raised in review by DaemonEye):** the copy stays
authoritative. Drift is reported and never repaired automatically. A stale
copy still runs.

**What "done" means:**

- **A pure drift comparison** in `src/scripts.rs`. For one copy and its
  source, it says whether the copy is **current** (byte-identical to the
  source), **stale** (they differ) or **orphaned** (the source no longer
  exists). A second function walks a copy directory and returns one entry
  per installed copy, named. A missing copy directory gives an empty list,
  not an error. Both take their directories as arguments, so tests run on
  temporary directories and never read the real `/usr/local/libexec`.
  Production callers pass the per-uid directory, the parent of
  `root_script_path(name)`, and `scripts_dir()`.
- **`daemoneye doctor`** gains one row (`src/cli/doctor.rs`), following its
  existing pattern: `gather()` fills a new `DoctorInputs` field, and a pure
  check function reads it. With no copies installed, the row is `Ok`. With
  every copy current, it is `Ok` and shows the count. With any stale or
  orphaned copy, it is `Warn`, names each one, and gives
  `daemoneye install-sudoers <name>` as the fix for a stale copy. `Warn`
  never changes the exit code; that rule already exists.
- **A ghost sudo call that resolves to a stale copy** still runs the copy
  unchanged and publishes one `Warning` notice (`crate::bus::publish`,
  `Source::Ghost`). The notice's title names the script and says the
  installed copy differs from `~/.daemoneye/scripts/<name>`. Its label is
  `script=<name>`. Build the notice in a pure function that returns
  `Option<Notice>` (`None` unless stale), and test that. The publish call
  sits in `ghost_exec.rs`, right after `resolve_command`. The command, the
  approval and the outcome are unchanged.
- **`write_script`** (`knowledge::write_script`, `src/daemon/executor/knowledge/artifacts.rs`):
  if the script just written has an installed copy that now differs, the
  success result gains one sentence. It says `sudo <name>` still runs the
  copy installed by `install-sudoers`, and to re-run
  `daemoneye install-sudoers <name>` to update it. Put the sentence in a
  pure function that returns `Option<String>`, and test it. With no copy,
  or a current copy, the result text is exactly what it is today.

**Constraints:**

- Tests are **hermetic**. Every test builds its copy and source files under
  a `tempfile::TempDir`. No test reads `/usr/local/libexec`, the real
  `scripts_dir()` or `$HOME`.
- No `unsafe`, `.unwrap()`, `.expect()` or `panic!` in production code. To
  get the euid, use `root_script_path`, which phase 04 made safe.
- Drift is computed by **reading both files**. The copy is `root:root 0755`
  and readable by the user, so no `sudo` is needed. A read error on either
  side counts as "cannot tell": report it, and never fail the caller.
- Only the local host. A remote ghost's sudo path is unchanged (see Out of
  scope).

**Scoping hints.** Start with these, and widen only if the work cannot be
done without it. If you do widen, log why in the Update Log.

- `src/scripts.rs`: `root_script_path`, `scripts_dir`, and the existing
  test module's temp-directory pattern.
- `src/cli/doctor.rs`: `DoctorInputs`, `evaluate`, `Check::ok` / `Check::warn`,
  `gather`, and one existing check plus its tests (`tmux_check` is a good
  model).
- `src/daemon/executor/ghost_exec.rs`: the `policy.resolve_command(cmd)`
  site.
- `src/bus/notice.rs`: `Notice::new`, `.label`, `Source`; and `Severity`
  (`src/bus/severity.rs`).
- `src/daemon/executor/knowledge/artifacts.rs`: `write_script`'s success
  arm.

**Do not read:** `tests/doc_truth.rs`, `tests/hygiene.rs`, `README.md`,
`CLAUDE.md`, `docs/dev/WORKFLOW.md`. This phase changes no docs. The
architect writes them at review.

**Budget.** You have **1200 turns**, so use them efficiently. Make your
first edit within your first **10** tool calls. Edit in this order:
`scripts.rs` (the comparison and its tests), then `doctor.rs`, then
`artifacts.rs`, then `ghost_exec.rs`.

## First red test

**Write this test before you open any file other than `src/scripts.rs`.**

`copy_drift_reports_current_stale_and_orphaned` (in `src/scripts.rs`'s test
module). In one `TempDir`, set up three pairs: one where the copy and the
source have the same bytes, one where they differ, and one where the copy
exists but the source does not. The drift comparison must return current,
stale and orphaned respectively. Run it and see it fail to compile, or
fail, before you write the comparison.

## Acceptance criteria

Each item is covered by a named test that fails on the current tree. Paste
each red run into the Update Log before its fix lands.

- [ ] **Comparison:** `copy_drift_reports_current_stale_and_orphaned`
      passes.
- [ ] **Directory walk:** `installed_copies_drift_lists_each_copy` passes.
      A directory holding two copies gives two named entries with their
      states, and a missing directory gives an empty list.
- [ ] **Doctor, all clear:** `sudo_scripts_check_ok_when_none_or_current`
      passes. No copies gives `Ok`, and two current copies give `Ok` with
      the count in the detail.
- [ ] **Doctor, drift:** `sudo_scripts_check_warns_naming_stale_and_orphaned`
      passes. One stale and one orphaned copy give `Warn`, and the detail
      names both. The fix names `daemoneye install-sudoers`, and
      `exit_code` over the checks is still 0.
- [ ] **write_script note:** `sudo_copy_note_only_when_stale` passes. A stale
      copy gives `Some` sentence naming the script and
      `daemoneye install-sudoers <name>`. A current copy, or no copy, gives
      `None`.
- [ ] **Ghost notice:** `stale_sudo_notice_is_a_warning_naming_the_script`
      passes. A stale copy gives a `Notice` with `Severity::Warning`,
      `Source::Ghost`, a title naming the script and a `script` label. A
      current copy gives `None`.
- [ ] **Mutation check,** with the output pasted: make the comparison
      report every pair as current, then run the six tests above and see
      the drift tests fail. Restore it.
- [ ] **No refusal (preservation):** the diff to `ghost_exec.rs` adds no
      early return and does not change `cmd`, the approval request or the
      outcome. It only adds the lookup and publish.
- [ ] **Gates:** all four pass, and the lib suite reports **at least 3085**
      passed (3079 plus at least six new tests).

## Spec

1. **Break down** — write the first red test and run it red. Then write a
   numbered task list into an Update Log entry, one line per task, each
   naming the acceptance item it closes. Ends when that entry is written.
2. **Implement** — work through the list in the edit order above, writing
   each test red before its code. Ends at a green `cargo build --tests`.
3. **Meet the acceptance criteria** — check off every item against your
   tests and the gates, including the mutation check. Ends when every item
   is checked with its evidence named.
4. **Capture the evidence** — run the four gate commands. Paste each
   `test result:` line, your red runs and the mutation check into a new
   Update Log entry. Do not paste per-test `… ok` lines. Commit as one
   `feat:` commit.

## Authorizations

- May touch `src/scripts.rs`, `src/cli/doctor.rs`,
  `src/daemon/executor/ghost_exec.rs` and
  `src/daemon/executor/knowledge/artifacts.rs`, each with its test module.
  Nothing else.
- If the work needs another file, record the file and the reason in the
  Update Log before editing it.

## Out of scope

- Syncing the copy, refusing a stale call, or prompting the user. The PE
  ruled all three out.
- Remote hosts. A remote ghost's sudo runs `~/.daemoneye/scripts/<name>` on
  a host whose sudoers belongs to the user (phase 04's out-of-scope).
- Old dotted `daemoneye-*.sh` rule files. They were inert, and the release
  note covers them.
- User-facing docs (README, CLAUDE.md, help text). The architect writes
  them at review.
- Running `install-sudoers` for real. The architect checks drift live at
  review: install a copy, edit the source, then run `daemoneye doctor`.

## Update Log

<!-- entries appended below this line -->
