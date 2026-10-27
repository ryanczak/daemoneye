# Phase 11 — Move the IPC payload structs out of `ipc.rs`

**Status:** review
**Milestone:** M31 — Code hygiene
**Drafted:** 2026-10-27

## Goal

`src/ipc.rs` (966 production lines) drops under the 800-line cap by
moving the payload structs that `Request` and `Response` carry
(`ContainerInfo` through `SessionSummary`) into a new
`src/ipc_types.rs`. The new file is declared as a child module of `ipc`
and glob re-exported, so every `crate::ipc::<Type>` path still resolves.
`ipc.rs` keeps `APPROVAL_TIMEOUT`, the two enums, `classify` and
`Response`'s impl, and stays "the full wire protocol". This settles the
README's open question on `ipc.rs`: it needs a split, not an exemption.
Pure move.

## Architecture references

- `docs/dev/milestones/M31-code-hygiene/README.md` — decisions 2, 5, 6, 7.

## Spec

1. **Cut with the shell — run this as written.** Dry-run on a scratch
   worktree of `6b9ee86c`: fmt (a no-op), clippy and the full suite (2586)
   green, and the moved block is byte-identical to its source range.
   **No `sed -i` on source files** (the executor refuses in-place edits):
   the rewrite goes through a temp file and `mv`. Nothing inside the moved
   code changes. Every moved struct is already `pub`. The child's
   `use super::*;` and the parent's `pub use types::*;` form a circular
   glob, which Rust resolves; leave it.

   ```bash
   F=src/ipc.rs; N=src/ipc_types.rs
   S=$(grep -n '^/// One sandboxed container this daemon owns' $F | cut -d: -f1)
   E=$(( $(grep -n '^/// Messages sent from the CLI client to the daemon' $F | cut -d: -f1) - 2 ))
   echo "S=$S E=$E"                      # expect 11 256
   { printf '//! The payload structs `Request` and `Response` carry: container, pane,\n//! schedule, script, shell, log, ghost, limits, runbook and session summaries.\n\nuse super::*;\n\n'
     sed -n "${S},${E}p" $F; } > $N
   { head -n $((S-1)) $F
     printf '#[path = "ipc_types.rs"]\nmod types;\npub use types::*;\n\n'
     tail -n +$((E+2)) $F; } > $F.tmp && mv $F.tmp $F
   ```
2. **Log the cut at once.** Append a `### Update — <date> (cuts done)`
   entry to this doc's Update Log **with the patch or write tool**. The
   stall governor counts only edit-tool calls as progress (rexyMCP issue
   #14).
3. `cargo fmt --all`. No import pruning: the build has no warnings.
4. **Allowlist:** delete the line `file src/ipc.rs 966`. No `fn` entry
   names `ipc.rs`.
5. **CLAUDE.md** "Key files": add a row for `src/ipc_types.rs` directly
   after the `src/ipc.rs` row: the payload structs the wire enums carry
   (`ContainerInfo`, `PaneInfo`, `ShellInfo`, `GhostConfig`,
   `SessionSummary`, …), a child module of `src/ipc.rs`, glob re-exported
   from it.
6. **Findings** (decision 7): read the moved section once and record what
   you see, or `none`.
7. **Run the End-to-end block and paste its output into the Update Log
   with the write or patch tool**, as a `### Update — <date> (end-to-end
   verification)` entry holding the contents of `/tmp/e2e-m31-11.txt` in
   a fenced block. Check with `grep -c '^lost_other=0$'` on this doc
   before you commit; it must print 1.

**Work in this checkout.** The spec has already been dry-run; do not
repeat it in a scratch worktree.

## Acceptance criteria

- [ ] `src/ipc_types.rs` defines `ContainerInfo`, `PaneInfo`, `ShellInfo`,
      `GhostConfig` and `SessionSummary`. `ipc.rs` defines none of them,
      still defines `Request` and `Response`, and has at most 800
      production lines (`cargo test --test hygiene` measures it).
- [ ] `grep -c '^file src/ipc.rs ' tests/hygiene_allowlist.txt` is 0 and
      `cargo test --test hygiene` passes.
- [ ] The End-to-end block prints `lost_other=0`, `added_other=0` and
      `moved_identical=1`.
- [ ] Preservation: `cargo test` passes with exactly 2586 passing tests.
- [ ] The Update Log has a `(cuts done)` entry, a `### Findings — <date>`
      entry and an `(end-to-end verification)` entry containing the
      block's output. An edit tool wrote all three, and all three are in
      this file.

## End-to-end verification

Run with `bash`. Paste the output byte-exact into the Update Log (Spec 7).

```sh
export LC_ALL=C   # sort and comm disagree under the host locale without it
B=6b9ee86c; F=src/ipc.rs; N=src/ipc_types.rs
A=/tmp/e2e-m31-11.txt; : > "$A"
wc -l $F $N | sed 's/^ *//' >> "$A"
echo "allowlisted=$(grep -c '^file src/ipc.rs ' tests/hygiene_allowlist.txt)" >> "$A"
norm() { tr -d ' \t' | grep -v '^$' | sort; }
git show $B:$F | norm > /tmp/m31-11-old.txt; cat $F $N | norm > /tmp/m31-11-new.txt
echo "lost_other=$(comm -23 /tmp/m31-11-old.txt /tmp/m31-11-new.txt | wc -l)" >> "$A"
echo "added_other=$(comm -13 /tmp/m31-11-old.txt /tmp/m31-11-new.txt | grep -vE '^(usesuper::\*;|pubusetypes::\*;|//!|#\[path=|modtypes;)' | wc -l)" >> "$A"
if diff -q <(git show $B:$F | sed -n 11,256p) <(tail -n +6 $N) >/dev/null; then echo moved_identical=1; else echo moved_identical=0; fi >> "$A"
cargo test --test hygiene 2>&1 | grep -E '^test result' | sed 's/finished in [0-9.]*s/finished in <t>s/' >> "$A"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1 | sed 's/in [0-9.]*s/in <t>s/' >> "$A"
cargo fmt --all --check; echo "fmt_exit=$?" >> "$A"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}' >> "$A"
cat "$A"
```

Expected (from the dry run): `726 …ipc.rs`, `251 …ipc_types.rs`,
`allowlisted=0`, `lost_other=0`, `added_other=0`, `moved_identical=1`,
hygiene `ok.`, clippy clean, `fmt_exit=0`, `passed=2586 failed=0`.

## Authorizations

- Edit `src/ipc.rs`; create `src/ipc_types.rs`. Edit
  `tests/hygiene_allowlist.txt` (one line) and `CLAUDE.md` (one row).

## Out of scope

- `Request`, `Response`, `RecentCommand`, `Classification` and
  `classify`, which stay in `ipc.rs`; `src/ipc_tests.rs`.
- Any change inside the moved structs, including their serde attributes
  (they are the wire format).

## Update Log

<!-- entries appended below this line -->

### Update — 2026-10-26 20:04 (cuts done)

Executor run (session 2026-10-26). Spec 1 ran as written: the shell block
computed `S=11 E=256` (matching the expected 11 256), cut lines 11–256 of
`src/ipc.rs` into a new `src/ipc_types.rs` with the module doc header and
`use super::*;`, and left the deletion in place via a temp file + `mv` —
no `sed -i` on source files. Byte-identity of the moved block verified
against the source range (`diff` of `git show 6b9ee86c:src/ipc.rs`
lines 11–256 vs `tail -n +6 src/ipc_types.rs` → identical). Line counts
after the cut: `src/ipc.rs` 726, `src/ipc_types.rs` 251. No edits with
the edit tool to either file; no import pruning. Spec 2 logged
immediately with the patch tool.

### Findings — 2026-10-26 20:13

Read of the moved section (lines 11–256 of the pre-cut `ipc.rs`, now
`ipc_types.rs` in full), decision 7:

1. `ipc_types.rs` · `ContainerInfo.image` — smell, nit. The field is
   documented as docker detail, but its only producer
   (`daemon/executor/sbx_lifecycle.rs` `sandbox_rows`) hard-codes
   `"shell"`, and the sandbox renderer (`cli/status.rs`
   `sandbox_section`) never prints the per-container `image` — only the
   report-level `image_detail`, itself the constant "not used by sbx".
   Dead-ish wire field kept for protocol compatibility. Worth a look if
   the sbx payload shapes ever change; not a defect today.
2. `ipc_types.rs` · `ContainerInfo.session` / `is_ghost` doc comments —
   smell, nit. The docs say the session comes "from the `de.session`
   label" and the ghost flag "carries `de.ghost=1`" — wording left over
   from the docker-label era (M18/M19). The actual source is the sandbox
   name prefix (`de-ghost-` / `de-chat-`, stripped in `sandbox_rows`);
   no `de.session` or `de.ghost` label exists anywhere else in the
   codebase. Doc-only drift, no behaviour behind it.
3. `ipc_types.rs` · `GhostConfig.auto_approve_scripts` doc — smell, nit.
   The doc references `daemoneye install-sudoers` and `/etc/sudoers.d/`
   NOPASSWD entries; the struct also carries the newer
   `auto_approve_commands` / `tool_policy` fields 60 lines later, which
   the prose predates and does not cross-reference. Doc polish only.

Everything else in the moved block read clean: the `#[serde(default)]`
placement on the late-added `GhostConfig` / `ShellInfo` /
`LimitsSummary` fields is consistent, and the summaries are plain
display-shaped structs with no logic to review.

### Update — 2026-10-26 20:13 (end-to-end verification)

Ran the phase doc's End-to-end block with `bash` (LC_ALL=C, base
commit `6b9ee86c`); output captured mechanically to
`/tmp/e2e-m31-11.txt` and pasted below byte-exact:

```text
726 src/ipc.rs
251 src/ipc_types.rs
977 total
allowlisted=0
lost_other=0
added_other=0
moved_identical=1
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in <t>s
    Finished `dev` profile [unoptimized + debuginfo] target(s) in <t>s
fmt_exit=0
passed=2586 failed=0
```

Every expected line matches the dry run: `lost_other=0` (nothing from
the old file is gone except what moved), `added_other=0` (nothing new
beyond the module header, the `use super::*;`, and the re-export
stanzas the block's own grep excludes), `moved_identical=1` (the moved
block is byte-identical to source lines 11–256), `allowlisted=0`
(the `file src/ipc.rs 966` allowlist line is deleted), hygiene
`ok. 16 passed`, clippy clean, `fmt_exit=0`, and the full suite at
`passed=2586 failed=0`.

### Update — 2026-10-27 03:16 (complete, server-authored)

**Summary:** </think>Everything is committed. Working tree is clean. Let me do the final checklist:

- [x] Phase doc's Status: line says `in-progress` (flipped at start; server flips to review)
- [x] Milestone README's phase-table row says `in-progress`
- [x] Did NOT hand-write a `(complete)` Update Log entry
- [x] `cargo fmt --all` run immediately before `git add`
- [x] All verification commands ran clean (fmt, build, clippy, test; hygiene test; E2E block)
- [x] Final message will be substantive Summary + Notes for review
- [x] `git status --short` shows nothing
- [x] `git log -1 --stat` shows the commit includes every file touched

All acceptance criteria:
- [x] `src/ipc_types.rs` defines ContainerInfo, PaneInfo, ShellInfo, GhostConfig, SessionSummary (verified in read). ipc.rs defines none of them, still defines Request and Response, and has 726 production lines (measured by hygiene test which passes).
- [x] `grep -c '^file src/ipc.rs '` = 0 and `cargo test --test hygiene` passes.
- [x] E2E block prints lost_other=0, added_other=0, moved_identical=1.
- [x] cargo test passes with exactly 2586 passing tests.
- [x] Update Log has (cuts done), Findings, and (end-to-end verification) entries, all written with edit tools, all in the file. `grep -c '^lost_other=0$'` printed 1.

Now write the final summary message.</think>
Phase 11 is complete. Summary and notes for review:

**Summary:** Moved the IPC payload structs (`ContainerInfo` through `SessionSummary`) out of `src/ipc.rs` into a new `src/ipc_types.rs`, exactly per the spec's shell block — `S=11 E=256` matched the expected range, the deletion went through a temp file + `mv` (no `sed -i`), and the moved block diffed byte-identical against `git show 6b9ee86c:src/ipc.rs` lines 11–256. `ipc.rs` keeps `APPROVAL_TIMEOUT`, `Request`/`Response`, `classify` and `Response`'s impl, declared the child module with `#[path = "ipc_types.rs"] mod types; pub use types::*;`, and is now 726 production lines. Deleted the `file src/ipc.rs 966` allowlist line, added the `src/ipc_types.rs` row to CLAUDE.md's Key files directly after the `src/ipc.rs` row, flipped the phase doc and README phase-table row to `in-progress`, and wrote the `(cuts done)`, `### Findings`, and `(end-to-end verification)` Update Log entries with the edit tools. The E2E block output pasted into the Update Log matches the dry run exactly: `726/251` line counts, `allowlisted=0`, `lost_other=0`, `added_other=0`, `moved_identical=1`, hygiene `ok. 16 passed`, clippy clean, `fmt_exit=0`, `passed=2586 failed=0`; `grep -c '^lost_other=0$'` on the doc prints 1. Committed as `f23f6559` and the tree is clean.

**Notes for review:** Findings (decision 7) are nits only — `ContainerInfo.image` is a dead-ish wire field (`"shell"` hard-coded in `sandbox_rows`, never rendered per-container), two `ContainerInfo` doc comments still describe the docker-label era (`de.session` / `de.ghost=1`) when the source is the sandbox name prefix, and `GhostConfig.auto_approve_scripts`' doc predates the newer `auto_approve_commands`/`tool_policy` fields. No code changed inside the moved block; the only `ipc.rs` additions are the three-line child-module stanza. All three Update Log entries are in the phase doc, and the `grep` proof is in the file.

**Executor:** nvidia/GLM-5.3-Flash-FP4-Local

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.16s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.17s


TEST
_still_counted ... ok
test function_length_spans_signature_to_closing_brace ... ok
test duplicate_names_get_a_suffix ... ok
test functions_in_a_test_module_are_not_measured ... ok
test impl_and_trait_methods_are_measured_with_their_type ... ok
test exempt_files_exist ... ok
test allowlist_has_no_stale_entries ... ok
test test_files_fit_the_cap ... ok
test production_files_fit_the_cap ... ok
test functions_fit_the_cap ... ok

test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.28s


running 34 tests
test daemon_ping_status_loop ... ignored
test g3_tool_policy_deny_merged_and_enforced ... ok
test cancel_request_roundtrip ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g4_briefing_injection_block_format ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g5_depth_limit_enforced ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ipc_ask_round_trip ... ok
test ghost_config_parsing ... ok
test ipc_tool_call_response_round_trip ... ok
test g5_child_inherits_depth_and_parent ... ok
test ipc_session_info_round_trip ... ok
test resume_run_request_roundtrip ... ok
test minimal_config_parsing ... ok
test schedule_store_persistence ... ok
test config_pricing_round_trip ... ok
test window_switch_does_not_corrupt_chat ... ignored
test g4_briefing_masking_applied ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test g4_briefing_injects_on_next_run ... ok
test event_log_append_read ... ok
test event_log_entry_format ... ok
test g6_agent_namespace_field_persisted ... ok
test g4_briefing_read_and_clear ... ok
test g6_agent_config_roundtrip ... ok
test session_index_persistence ... ok
test session_jsonl_round_trip ... ok
test webhook_alert_below_threshold_discarded ... ok
test webhook_alert_unrankable_severity_passes_gate ... ok
test webhook_alert_to_event_log ... ok
test webhook_alert_no_severity_passes_gate ... ok
test g5_mailbox_write_and_read ... ok

test result: ok. 32 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.09s


running 12 tests
test webhook_ghost_e2e_http ... ignored
test held_port_cannot_be_rebound ... ok
test webhook_ports_differ_between_environments ... ok
test stub_returns_canned_response_via_make_client ... ok
test webhook_ghost_e2e_deterministic ... ok
test webhook_ghost_records_its_ssh_target_resolution ... ok
test an_empty_session_starts_no_tmux_server ... ok
test config_contains_webhook_and_stub_url ... ok
test daemon_boots_in_throwaway_root ... ok
test daemon_installs_no_tmux_hooks ... ok
test default_server_unchanged ... ok
test daemon_webhook_returns_200 ... ok

test result: ok. 11 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.31s


running 4 tests
test gate_catches_an_abbreviated_write ... ok
test gate_catches_a_chained_write ... ok
test gate_catches_a_multi_line_write ... ok
test source_issues_no_tmux_write_verbs ... ok

test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.16s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-93da9e6ee623fb89)
kill: (1141518): No such process
kill: (-1142228): No such process
     Running unittests src/main.rs (target/debug/deps/daemoneye-e8cf52a7c32204b4)
     Running tests/bug_tracker.rs (target/debug/deps/bug_tracker-e3c81dbbd815c973)
     Running tests/doc_truth.rs (target/debug/deps/doc_truth-13504e43862292bc)
     Running tests/hygiene.rs (target/debug/deps/hygiene-5888a12c4886b465)
     Running tests/integration.rs (target/debug/deps/integration-9d2755b5d0d87df4)
     Running tests/isolation.rs (target/debug/deps/isolation-aff2e96eb7fd44a5)
     Running tests/tmux_read_only.rs (target/debug/deps/tmux_read_only-ed33cf3a0b1b3008)
   Doc-tests daemoneye

```

**Files changed:**

- `CLAUDE.md` — +1 -0
- `docs/dev/milestones/M31-code-hygiene/README.md` — +1 -1
- `docs/dev/milestones/M31-code-hygiene/phase-11-ipc-types.md` — +75 -1
- `src/ipc.rs` — +3 -246
- `src/ipc_types.rs` — +251 -0
- `tests/hygiene_allowlist.txt` — +0 -1

**Commit:** f23f6559d168f7c15fc86166b8e38797d89585c2

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).
