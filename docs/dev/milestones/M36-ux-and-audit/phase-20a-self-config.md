# Phase 20a — self-config

**Milestone:** M36 — Chat UX and audit fixes
**Status:** todo
**Drafted:** 2026-11-08
**Depends on:** phase 19
**Estimated diff:** ~400 lines
**Tags:** language=rust, kind=feature, size=m

## Goal

Decision 8, second half, first part. The AI can read the daemon's own
`~/.daemoneye/etc/config.toml` with its secrets masked, and edit it. A
config edit is always asked, shows a masked diff, is validated as a
`Config` before the write, and is followed by the phase-19 reload, whose
report the tool result carries. Phase 20b adds the `config-reference`
memory.

## Architecture references

- `docs/dev/milestones/M36-ux-and-audit/README.md`, decision 8 and the
  exit criterion that begins "Asked in chat to raise `[limits]`".
- `src/config/handle.rs` (`daemon_reload`), `src/config/load.rs`
  (`Config::load`).
- `src/daemon/executor/file_ops/` (`mod.rs` guards, `read.rs`, `write.rs`,
  `ops.rs` `run_edit`), `src/ai/filter.rs` (`mask_sensitive`).
- `src/cli/commands/stream_prompts.rs` (`edit_file_auto_approved`,
  `edit_file_grants_session`, the sudo-edit precedent),
  `src/cli/commands/stream_ask.rs`, `src/ipc.rs` (`EditFilePrompt`).

## Behaviour

1. **The admitted file** is exactly the daemon host's
   `<config_dir>/etc/config.toml`, compared after the same canonical
   resolution the `~/.daemoneye` guard uses (a symlink to it is the file).
   Every other path under `~/.daemoneye/` keeps today's refusal.
   - **Must stay refused:** `etc/prompts/sre.toml` (read and edit);
     `etc/config.toml.bak`, `etc/config.toml.de_tmp`, `etc/other.toml`;
     the config path with `host` naming a configured `[hosts.<name>]`
     host; `sudo: true`; `operation` `create`, `delete` or `copy` with the
     config as `path` or `dest_path`; a ghost (edit_file already refuses
     ghosts).
2. **`read_file`** returns the config's contents through the same masking
   every read gets. A secret value (`api_key = "…"`, a webhook
   `secret`, a host password) never appears in the result.
3. **`edit_file operation="edit"`** on the config:
   - **Validate first.** Apply the replacement to the file's real
     contents in memory and parse the result the way `Config::load` does
     (same deserialization, same `parsing <path>` context). On failure the
     tool result names the TOML error, nothing is prompted, and the file is
     byte-for-byte unchanged.
   - **The match uses the real strings.** The model's `old_string` is
     matched against the file's real contents, so an `old_string` copied
     from the masked read (containing the mask placeholder) does not match
     and nothing is written — that is how a masked value can never be
     written back.
   - **The prompt is masked.** The `EditFilePrompt`'s existing and new
     contents pass through `mask_sensitive`, so the client never receives a
     secret value. The write uses the unmasked strings.
   - **Always asked.** The prompt carries a flag the client honours the way
     it honours `sudo`: neither `[approvals] file_edits = true`,
     `file_edits_all` nor a per-path session grant auto-approves it, `[A]`
     approves this edit only, and the panel says the edit cannot be
     session-approved. The flag defaults to `false` on the wire, so an old
     frame still decodes. `daemoneye ask` keeps denying every edit.
   - **Reload after the write.** After a successful write the daemon runs
     `config::daemon_reload()` and appends its report to the tool result:
     the applied keys and the keys waiting for a restart, or
     `nothing changed`. A reload that fails (or no installed handle) does
     not undo the write; the result says the write succeeded and names the
     reload's error.
4. **Docs.** CLAUDE.md's `read_file` and `edit_file` rows and README's
   § AI tools rows say the config is readable (masked) and editable
   (always asked, validated, reloaded), and that `etc/prompts/sre.toml`
   stays blocked.

## Constraints

- Do not change the TOML schema, any default, or `Config::load`'s
  missing-file behaviour. Factor the parse step so `Config::load` and the
  edit share it; `Config::load`'s behaviour stays identical.
- **Gotcha: the handle leaks between unit tests.** Test the edit path
  under `crate::test_home()` with the handle uninstalled; the reload then
  fails with its "no config handle" error, which the result must report
  without failing the write. Test the reload report's formatting through a
  seam that takes the two lists, not by installing the global.
- **Gotcha: count criteria grep comments too** (phase 19's
  `daemon_config_loads`). Every grep below names a string that occurs only
  where the behaviour lives; keep it out of doc comments.
- Hold no lock across an `.await`. Add no crate.

## Spec

1. **Red tests first**, real output pasted into an Update Log entry
   `(red runs)`; a test naming a new seam is red as a compile error.
   - `read_file_admits_the_config_with_secrets_masked`
   - `read_file_still_refuses_the_prompt_file`
   - `edit_file_config_refuses_create_delete_copy_host_and_sudo`
   - `edit_file_still_refuses_other_daemoneye_paths` (the `.bak`,
     `.de_tmp` and `other.toml` cases)
   - `edit_file_config_refuses_an_unparseable_result_and_leaves_the_file`
   - `edit_file_config_masked_placeholder_never_matches`
   - `edit_file_config_prompt_is_masked_and_always_asked`
   - `edit_file_config_reports_the_reload_after_the_write`
   - `config_edit_is_never_auto_approved` (the client predicate, with
     `file_edits_all` set and the path session-granted)
   - `edit_file_prompt_without_the_flag_decodes`, beside the round trips
     in `src/ipc_tests.rs`
   - `the_seeded_template_round_trips_through_a_config_edit`: copy
     `assets/etc/config.toml` into the test home, read it, and apply a
     `[limits]` edit through the validate-and-write path (the real
     artifact, not a hand-written TOML string).
2. **Implement** Behaviour 1–3, running `cargo build` after each step.
3. **Docs** per Behaviour 4.
4. **Mutation check:** make the always-asked flag `false` for the config.
   Run `cargo test --lib edit_file_config_prompt_is_masked_and_always_asked`
   and confirm it fails; restore and confirm it passes. Paste both real
   results into an `(mutation)` entry.
5. **End-to-end:** run § End-to-end verification **once**, redirected to
   `.rexymcp/output/p20a-e2e.txt`, and paste that file verbatim into an
   `(end-to-end verification)` entry naming it. Done when the entry's
   block and the file are identical.
6. **Findings:** a `### Findings — <date>` entry of two or three lines, or
   `none`.
7. **Commit** as
   `feat: self-config — read_file and edit_file admit config.toml, masked, always asked, reloaded (M36 phase-20a)`.

## Acceptance criteria

- [ ] All eleven named tests pass, each printed by the end-to-end loop with
      `passed=1` or more and `failed=0`. Today none exists.
- [ ] `config_read_blocked=0`: `read.rs` no longer lists
      `join("config.toml")` in its blocked set. Today 1.
- [ ] `claude_md_blocked_claim=0`: CLAUDE.md no longer says
      `blocked only from \`etc/config.toml\``. Today 1.
- [ ] The prompt-file refusal holds: `read_file_still_refuses_the_prompt_file`
      passes (preservation).
- [ ] Hygiene, `doc_truth`, `bug_tracker` and `lock_invariant` are ok,
      `build_warnings=0`, clippy is clean, `fmt_exit=0` and `failed=0`.

## End-to-end verification

```sh
bash <<'E2E'
for t in read_file_admits_the_config_with_secrets_masked read_file_still_refuses_the_prompt_file edit_file_config_refuses_create_delete_copy_host_and_sudo edit_file_still_refuses_other_daemoneye_paths edit_file_config_refuses_an_unparseable_result_and_leaves_the_file edit_file_config_masked_placeholder_never_matches edit_file_config_prompt_is_masked_and_always_asked edit_file_config_reports_the_reload_after_the_write config_edit_is_never_auto_approved edit_file_prompt_without_the_flag_decodes the_seeded_template_round_trips_through_a_config_edit; do
  cargo test --lib "$t" 2>&1 | grep -E '^test result' | awk -v t="$t" '{p+=$4; f+=$6} END {print t, "passed=" p, "failed=" f}'
done
grep -c 'join("config.toml")' src/daemon/executor/file_ops/read.rs | sed 's/^/config_read_blocked=/'
grep -c 'blocked only from `etc/config.toml`' CLAUDE.md | sed 's/^/claude_md_blocked_claim=/'
touch src/main.rs && cargo build 2>&1 | grep -c '^warning' | sed 's/^/build_warnings=/'
for t in hygiene doc_truth bug_tracker lock_invariant; do cargo test --test $t 2>&1 | grep -E '^test result'; done
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1
cargo fmt --all --check; echo "fmt_exit=$?"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
E2E
```

## Authorizations

- **Edit:** `src/daemon/executor/file_ops/` and its tests;
  `src/daemon/executor/dispatch_files.rs`; `src/config/load.rs` and
  `src/config/handle.rs` (the shared parse step, a report-format seam);
  `src/ipc.rs` and `src/ipc_tests.rs`; `src/cli/commands/stream_prompts.rs`,
  `stream_ask.rs` and the renderer's edit-approval panel; CLAUDE.md and
  README.md per Behaviour 4.
- **Allowlist:** if the hygiene gate reports a file or function over its
  cap, raise its entry and give the reason in Findings. Do not split code
  to fit.
- **Scratch files** go under `.rexymcp/output/`; nothing new outside the
  authorised paths is committed.

## Out of scope

- The `config-reference` memory, its seeding and its `doc_truth` gate
  (phase 20b).
- Opening `etc/prompts/sre.toml` or any other `~/.daemoneye/` path.
- A config edit on a configured host, or as root.
- A live chat check: the milestone close runs the AI-driven exit
  criterion on pinky.

## Update Log

<!-- entries appended below this line -->
