# Phase 04 — sudoers-pin

**Milestone:** M39 — Critical fixes
**Status:** todo
**Drafted:** 2026-11-23
**Depends on:** none
**Estimated diff:** ~250 lines
**Tags:** language=rust, kind=bugfix, size=m

## Goal

A NOPASSWD rule that `daemoneye install-sudoers` writes can run only the
script as it was when the rule was installed. A ghost cannot gain root by
rewriting a script, or by calling an approved script name from another
directory.

## Statement of work

Two defects from the 2.10.0 audit (S3a, S3b) together let an unsandboxed
ghost run anything as root.

- **S3a — the rule names a file the user can write.** `install_sudoers`
  (`src/scripts.rs`) writes
  `<user> ALL=(ALL) NOPASSWD: <home>/.daemoneye/scripts/<name>`. The user
  owns that file. `GhostPolicy::is_safe` lets every non-sudo command
  through, so a ghost can overwrite the script with `cat > …` and then run
  `sudo <that path>` as root.
- **S3b — `is_safe` approves by basename.** For a sudo command,
  `GhostPolicy::is_safe` (`src/daemon/policy.rs`) takes the first word's
  `file_name()` and checks only that against `auto_approve_scripts`. So
  `sudo /tmp/anything/fix.sh` is approved whenever `fix.sh` is.

**PE decision (2026-11-23): a root-owned copy, not a `sha256:` digest.**
This host runs **sudo-rs 0.2.13**. Its sudoers grammar
(`man 5 sudoers-rs`: `Cmnd ::= '!'* command name | '!'* directory | …`) has
no digest spec, so a `sha256:` rule would not parse. `install-sudoers`
therefore installs a root-owned copy of the script, and the rule names that
copy:

- The copy lives at `/usr/local/libexec/daemoneye/<euid>/<name>`. `<euid>`
  is the numeric effective uid of the user running `install-sudoers`
  (`libc::geteuid()`); a numeric uid needs no passwd lookup and cannot
  collide between users. Add one function that every caller uses,
  `crate::scripts::root_script_path(name: &str) -> PathBuf`, and if you
  need one for tests, a variant that takes the uid as an argument.
- The per-uid directory and the copy are both `root:root`, mode `0755`.
  The copy is made with `sudo install`, which is how the rule is already
  installed. The copy is installed **before** the rule, so a rule never
  names a file that is missing or not yet root-owned.
- Editing the script changes nothing that runs as root until
  `install-sudoers` is run again. That is the point of the fix. The
  architect updates the user-facing docs (README, CLAUDE.md, the CLI help
  text) at review. **This phase touches no documentation.**

**What "fixed" means:**

- **`is_safe`, local policy (`ssh_target` unset).** A sudo command is
  approved only when its first word after `sudo` is **exactly**
  `root_script_path(<name>)` for a name in `auto_approve_scripts`,
  compared as a whole path string, not by `file_name()`. Everything else
  is refused: a bare name (`sudo fix.sh`, which looks the name up in
  PATH), any other directory (including the user's own scripts dir), and
  a non-canonical spelling (`…/<euid>/../<euid>/fix.sh`, `…/<euid>/./fix.sh`).
  The existing "plain tail" rule (no `;`, `&`, `|`, `$`, backtick,
  parentheses, redirections after the word) stays as it is.
- **`resolve_command`, local policy.** When the command runs under sudo
  (written `sudo fix.sh`, or `run_with_sudo` is set), it resolves to
  `sudo <root_script_path(name)><args>`. Without sudo it still resolves
  to `<scripts_dir>/<name><args>`, unchanged. `is_safe` is called on the
  resolved command (`ghost_exec.rs` resolves first, then
  `ghost_auto_outcome` in `executor/mod.rs` calls `is_safe`), so a
  resolved sudo call must pass `is_safe`.
- **Remote policy (`ssh_target` set): unchanged in behaviour.** The remote
  host's sudoers is the user's to manage. `resolve_command` still emits
  `sudo ~/.daemoneye/scripts/<name><args>`, and `is_safe` approves exactly
  that path for an approved name, and nothing else. Today it approves any
  path with the right basename, so this narrows `is_safe` on remote too,
  but the command `resolve_command` emits keeps working.
- **`install_sudoers`.** The steps it runs are built by a pure function
  the tests call: create the per-uid directory, copy the script, install
  the rule. Tests assert on that plan. The real `sudo` is never run in a
  test. The rule `sudoers_rule` produces names `root_script_path(name)`.

The existing tests that encode the old behaviour are wrong and must change:
`is_safe_sudo_on_whitelist` (approves the user's scripts-dir path),
`is_safe_run_with_sudo_does_not_allow_arbitrary_sudo` (its last line
approves the bare `sudo ghost-test-remediation.sh`), and every
`resolve_command` test in `policy_tests.rs` that expects a sudo call to
resolve into the scripts dir. Change their expectations and say which ones
in the Update Log. Do not delete a test to make it pass.

**Scoping hints.** Start here, and read further only when the work cannot
be finished from these. Record in the Update Log what you read beyond them
and why.

- `src/daemon/policy.rs`: `GhostPolicy`, `is_safe`, `resolve_command`
  (both its local and remote branches).
- `src/daemon/policy_tests.rs`: the helpers `policy(&[…])` and
  `sudo_policy(&[…])` at the top, and the `is_safe` and `resolve_command`
  sections. Your tests go here.
- `src/scripts.rs`: `install_sudoers`, `sudoers_rule`,
  `sudoers_escape_path`, `validate_script_name`, `scripts_dir`, and the
  `sudoers_rule_*` tests in its test module.
- **Do not read** `tests/doc_truth.rs`, `tests/hygiene.rs`,
  `docs/dev/WORKFLOW.md`, `README.md`, `CLAUDE.md` or `src/main.rs`. The two
  test files are gates: you **run** them (`cargo test`), you never read
  them. Nothing in this phase can trip `doc_truth`, which checks only the
  AI tool tables and the config reference. If `hygiene` fails, its message
  names the file and the cap, and that is all you need.
- Not needed: the sbx modules (a sandboxed ghost never runs sudo scripts,
  and `sbx::script_invocation` returns `None` for any sudo command), the
  executor arms, `command_has_sudo` (phase 05), and `wrap_remote`
  (phase 06), `ghost_exec.rs` and its helpers (they call `resolve_command`
  and `is_safe` and need no change), and `src/lib.rs` (no test here needs
  `test_home`). A wide grep for `scripts_dir` or `sudo` hits all of them.
  Leave them alone.

**Budget.** You have **1200 turns** to finish, so use them efficiently. Make
an edit as soon as you know what it is, and run a gate rather than
re-reading a file you have already read. **Your first source edit is the
first red test below, and it lands within 20 turns of starting.** Every
test you write red stays in the tree after its fix.

## First red test

**Write this test before you open any other file.** Your first tool call
reads `src/daemon/policy_tests.rs` lines 1–85 (the two helpers and the
first `is_safe` tests). Your second tool call is the edit that adds this
test after `is_safe_sudo_not_on_whitelist`. Your third runs it. Read
`policy.rs` only after that.

- **File:** `src/daemon/policy_tests.rs`, in the `is_safe` section.
- **Name:** `is_safe_refuses_an_approved_basename_in_another_directory`, a
  plain `#[test]`.
- **Body:** `let p = policy(&["fix.sh"]);` then
  `assert!(!p.is_safe("sudo /tmp/x/fix.sh"));`. Nothing else is needed: no
  temp HOME, no registry, no config.
- **Red looks like:** the assertion fails, because today `is_safe` takes
  the basename `fix.sh`, finds it approved, and returns `true`. Run
  `cargo test --lib is_safe_refuses_an_approved_basename_in_another_directory`
  and paste the failing `test result:` line into a progress entry.

## Test helpers

- `policy(&["fix.sh"])` and `sudo_policy(&["fix.sh"])` in
  `policy_tests.rs` build a local `GhostPolicy`. For a remote policy, copy
  `policy(…)` and set `ssh_target: Some("pinky".into())` in the test.
- Build expected paths with `root_script_path("fix.sh")` and
  `crate::scripts::scripts_dir().join("fix.sh")`. Never hard-code a uid or
  a home directory.
- `is_safe`, `resolve_command`, `sudoers_rule` and the install plan are
  pure. No test in this phase needs `crate::test_home()`, a socket or
  `sudo`.

## Evidence

- `src/scripts.rs`: `sudoers_rule(user, script_path)` formats
  `"{user} ALL=(ALL) NOPASSWD: {path}\n"`, and `install_sudoers` passes the
  canonicalized `scripts_dir().join(name)`. The test
  `sudoers_rule_passthrough_when_safe` pins a
  `/home/alice/.daemoneye/scripts/…` path.
- `src/daemon/policy.rs` `is_safe`: `let basename = Path::new(word).file_name()…;
  let approved = self.auto_approve_scripts.iter().any(|s| s == basename);`.
- `src/daemon/policy_tests.rs` `is_safe_run_with_sudo_does_not_allow_arbitrary_sudo`
  ends with `assert!(p.is_safe("sudo ghost-test-remediation.sh"));`, a bare
  name that sudo resolves through PATH.
- `sudo -V` on the dev host prints `sudo-rs 0.2.13-0ubuntu1.2`, and
  `zcat /usr/share/man/man5/sudoers-rs.5.gz` has no digest in the `Cmnd`
  grammar.

## Acceptance criteria

Each behaviour is covered by a test that fails on the current tree, unless
it is marked as preservation. Show each red run in the Update Log before
its fix lands. Every such test stays in the tree.

- [ ] **S3b, other directory:** with `fix.sh` approved, `is_safe` refuses
      `sudo /tmp/x/fix.sh`. (This is the first red test.)
- [ ] **S3b, user-writable path:** `is_safe` refuses
      `sudo <scripts_dir()>/fix.sh`, the path the old rule named.
- [ ] **S3b, bare and non-canonical:** `is_safe` refuses `sudo fix.sh`,
      `sudo <root dir>/../<euid>/fix.sh` and `sudo <root dir>/./fix.sh`,
      where `<root dir>` is the parent of `root_script_path("fix.sh")`.
- [ ] **S3b, the approved path:** `is_safe` accepts
      `sudo <root_script_path("fix.sh")>` and the same with `--flag`. It
      refuses `sudo <root_script_path("other.sh")>` when only `fix.sh` is
      approved, and refuses the approved path followed by `; id`.
- [ ] **S3b, resolve:** with a local policy, `resolve_command("sudo fix.sh")`
      and `sudo_policy(&["fix.sh"]).resolve_command("fix.sh")` both return
      `sudo <root_script_path("fix.sh")>`, and the result passes `is_safe`.
      `policy(&["fix.sh"]).resolve_command("fix.sh")` still returns
      `<scripts_dir()>/fix.sh` (preservation).
- [ ] **Remote (preservation):** with `ssh_target` set and `fix.sh`
      approved, `resolve_command("sudo fix.sh")` still returns
      `sudo ~/.daemoneye/scripts/fix.sh` and `is_safe` accepts it. On
      the same policy, `is_safe` refuses `sudo /tmp/x/fix.sh`.
- [ ] **S3a, the rule:** the rule `install_sudoers` installs names
      `root_script_path(name)`, not a path under `scripts_dir()`.
- [ ] **S3a, the install plan:** a test on the pure install plan shows, in
      order, (1) the per-uid directory created `root:root` mode `0755`
      under `/usr/local/libexec/daemoneye/`, (2) the script copied to
      `root_script_path(name)` as `root:root` mode `0755`, and (3) the rule
      installed into `/etc/sudoers.d/` mode `0440`. The copy comes before
      the rule.
- [ ] **No docs (preservation):** `git diff --stat` for this phase lists no
      `README.md`, `CLAUDE.md`, `src/main.rs` or `docs/` file other than
      this phase doc.
- [ ] `cargo fmt --all`, `cargo build`, `cargo clippy --all-targets
      --all-features -- -D warnings` and `cargo test` all pass, with
      `tests/hygiene.rs` and `tests/doc_truth.rs` included. Run them, do
      not read them.

## Spec

1. **Break down** — read the statement of work and the acceptance criteria,
   then write the first red test and run it red. Then write a numbered task
   list into a progress entry in the Update Log, one line per task, each
   naming the acceptance item it closes. Ends when that entry is written.
2. **Implement** — work through your task list, writing each test red
   before its fix. Ends at a green `cargo build --tests`.
3. **Meet the acceptance criteria** — check off every item against your
   tests and the gates. Ends when every item is checked, with its evidence
   named.
4. **Capture the end-to-end evidence** — run the four gate commands, then
   paste each `test result:` line and your red runs into a new Update Log
   entry. Do not paste per-test `… ok` lines.

## Authorizations

- May touch `src/daemon/policy.rs`, `src/daemon/policy_tests.rs`,
  and `src/scripts.rs` (and its test module). Nothing else.
- May change the expectations of the existing `is_safe` and
  `resolve_command` tests that encode the old behaviour. Name each one in
  the Update Log.
- If the work needs a file outside these, record the file and the reason
  in the Update Log before editing it.

## Out of scope

- `command_has_sudo` and the commands it misses. That is phase 05.
- `wrap_remote` and its `ssh ` escape. That is phase 06.
- Hardening the **remote** host's sudo scripts. The remote sudoers belongs
  to the user. This phase only keeps the remote path working and narrows
  `is_safe` to the exact path `resolve_command` emits.
- Removing rules installed by older versions. They still name the
  user-writable path, but `is_safe` no longer approves that path, so a
  ghost cannot use them. The release note tells users to re-run
  `install-sudoers`.
- Running `install-sudoers` for real. It needs the user's sudo password.
  The architect verifies it live at review.

## Update Log

<!-- entries appended below this line -->

### Notes for executor — 2026-11-23

The first run (session `6b03a2bc`) stopped at 200 read-only calls with no
edits. It never wrote the first red test. It read `policy_tests.rs` in seven
slices instead, and then spent about 170 turns reading
`tests/doc_truth.rs`, checking whether the documentation edits this doc
used to ask for would break it. Those edits are gone: the architect does
the documentation at review. `doc_truth.rs`, `hygiene.rs`, `WORKFLOW.md`,
`README.md`, `CLAUDE.md` and `main.rs` are now on the do-not-read list.
The **First red test** section now names your first three tool calls. Make
them in that order.

### Update — 2026-11-23 (escalation)

**Chosen lever:** refined re-dispatch
**Rationale:** the stall came from the spec. The doc asked for README and
CLAUDE.md edits and named `doc_truth` in the gate criterion without saying
that `doc_truth` cannot see those passages, so the executor went to read
it. Taking the documentation out and naming the first three calls removes
both the detour and the slow start. The run started before the PE turned on
`enable_thinking = true` / `reasoning_effort = "medium"` and
`output_filter = false`, so it tested neither setting. The re-dispatch is
the first run with both.

