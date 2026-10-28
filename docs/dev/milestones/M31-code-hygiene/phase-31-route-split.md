# Phase 31 — Split `execute_tool_call` into policy, snapshot and route

**Status:** done
**Milestone:** M31 — Code hygiene
**Drafted:** 2026-10-28

## Goal

`execute_tool_call` (`src/daemon/executor/mod.rs`) is 250 lines. The
family moves and the two `run_terminal_command` splits left it as a
130-line prelude and a 104-line match. This phase splits it in three,
and moves two of the parts into a new child module, `route.rs`:

| Function | Where | What it holds | Lines |
|---|---|---|---|
| `execute_tool_call` | `mod.rs` | the ghost-policy guard, the tool-policy refusal, `ToolStarted`, the call to `route`, `ToolFinished` | 93 |
| `load_snapshot` | `route.rs` | the one-acquisition read of the session entry into `DispatchSnapshot` | 28 |
| `route` | `route.rs` | the snapshot's fields, `is_ghost`, the memory namespaces and artifact context, then the family match | 147 |

The move into `route.rs` is what brings the file under its cap.
`mod.rs` goes from 944 to 788 production lines, and `execute_tool_call`
from 250 to 93. **Both allowlist entries are deleted**, not lowered:
the hygiene gate fails with "remove it" once an offender fits.
`executor/mod.rs` leaves the file list, and `execute_tool_call` leaves
the function list.

**What differs from a move.** All five were dry-run:

1. **The two policy checks read the snapshot through `snap.`.** The
   ghost-policy guard becomes
   `if snap.is_ghost_shell && snap.ghost_policy.is_none()`. The
   tool-policy check becomes `if let Some(policy) = &snap.tool_policy`.
   The `DispatchSnapshot` destructure now happens in `route`, which owns
   the snapshot. Every other line of both checks is unchanged.
2. **`execute_tool_call` destructures `SessionCtx` as
   `{ session_id, sessions, .. }`**, because `chat_pane` is now used only
   in `route`, which destructures its own copy. `SessionCtx` is `Copy`.
3. **The memory namespaces and the artifact context are built in
   `route`, after `ToolStarted` is sent.** They were built before it.
   The work is a session-store read and, for a ghost, an agent config
   load. Neither writes a frame or a log line, so nothing observable
   changes order.
4. **The two-line "Delegation depth tracking" comment is dropped.** It
   says only that `spawn_depth` and `effective_parent_job_id` are
   already loaded, which the destructure beside it now shows. Dropping
   it gives `route` three lines of headroom under the cap.
5. **The `?` stays in `execute_tool_call`.** `route` returns the match's
   `anyhow::Result<ToolCallOutcome>`, and the caller keeps
   `let outcome = route::route(…).await?;`. The early `?` exit before
   `ToolFinished` is the same as before.

`ToolFinished` still follows every `ToolStarted`.
`silent_tool_early_exit_sends_tool_finished` covers that, and it runs in
the suite.

## Architecture references

- `docs/dev/milestones/M31-code-hygiene/README.md`: decisions 3, 5, 6, 7.
- `docs/dev/milestones/M31-code-hygiene/phase-30-background-split.md`: the
  previous split of this function.

## Spec

1. **Run the script below as written, once.** It does the whole change:
   - the new `route.rs` and its `mod` line;
   - the shortened `execute_tool_call`;
   - deleting the two allowlist entries and adding the `CLAUDE.md` row;
   - then `cargo fmt --all`, twice.

   It was dry-run on a scratch worktree of `a09e9224`: clippy was clean, the
   full suite passed 2587, and the End-to-end block printed every expected
   line. Do not hand-edit anything it touches. Do not dry-run it again in
   a worktree of your own. Do not `sed -i` a source file. It is a Python
   heredoc inside a shell script, so copy it **without adding
   indentation**. The script asserts its anchors and prints
   `execute_tool_call split: load_snapshot + route`. If an assert fires,
   stop and report it in the Update Log.
2. **Log it at once.** Append a `### Update — <date> (cuts done)` entry
   with the patch or write tool, quoting the script's output. The stall
   governor counts only edit-tool calls as progress (rexyMCP issue #14).
3. **Findings** (decision 7) go in **their own
   `### Findings — <date>` entry, within five tool calls.** Run exactly
   this command, and open no other file:

   `grep -nE '\.(unwrap|expect)\(|panic!|let _ =|\.ok\(\)|unwrap_or_default|unwrap_or_else' src/daemon/executor/route.rs`

   Write each hit with its function, kind and severity, or write `none`.
   Do not look up callees: a finding names what this file does. #75 and
   #84 are recorded already, so do not repeat them.
4. **Run the End-to-end block, then paste its output into the Update Log
   with an edit tool**, as a `### Update — <date> (end-to-end
   verification)` entry holding `/tmp/e2e-m31-31.txt` in a fenced block.
   **Paste only output you have already seen.**

   **Known flake:** `read_shell_screen_lines_tails_the_visible_rows`
   (`FINDINGS.md` #65). If the block shows `passed=2586 failed=1`, run it
   once more and paste both outputs. Any other failure is real.

   **Before you commit, all four of these must print 1:**
   - on the output file, `grep -c '^segments_present=1$'`,
     `grep -c '^routed=1$'` and `grep -c '^allowlist_entries_left=0$'`;
   - on this doc, `grep -c '^### Findings — '`.
5. **Commit** as `refactor:`.

### The script

```bash
python3 - <<'PY'
F = 'src/daemon/executor/mod.rs'
src = open(F).read().split('\n')


def find(text, start=0, end=None, prefix=False):
    end = len(src) if end is None else end
    hits = [i for i in range(start, end)
            if (src[i].startswith(text) if prefix else src[i] == text)]
    assert hits, text
    return hits[0]


s = find('pub async fn execute_tool_call<W, R>(')
body = find('{', s)                                   # the body's opening brace
cd0 = find('    let SessionCtx {', body)
cd1 = find('    } = ctx;', cd0)
pf = find('    // ── Pre-fetch session state in one acquisition', cd1, prefix=True)
sm0 = find('    let snap: DispatchSnapshot = match session_id {', pf)
sm1 = find('    };', sm0)
sd0 = find('    let DispatchSnapshot {', sm1)
sd1 = find('    } = snap;', sd0)
g0 = find('    // Defensive guard: a ghost shell entry must always have a ghost_config.', sd1)
ig = find('    let is_ghost = ghost_policy.is_some();', g0)
dl = find('    // ── Delegation depth tracking', ig, prefix=True)
tp = find('    // ── Tool Policy Enforcement (agent-level)', dl, prefix=True)
ns = find('    // ── Build namespace list for memory operations', tp, prefix=True)
fb = find('    // Emit ToolStarted for silent tools so the client can show a history entry', ns)
m0 = find('    let outcome = match call {', fb)
m1 = find('    }?;', m0)
assert src[ig + 1] == '' and src[tp - 1] == '' and src[fb - 1] == ''
assert src[dl + 1].startswith('    // (spawn_depth and effective_parent_job_id already loaded') and dl + 3 == tp
assert all(src[i] != '' for i in range(sd0, sd1))

ctx_dest = ['    let SessionCtx {', '        session_id,', '        sessions,', '        ..', '    } = ctx;']
assert ' '.join(src[cd0:cd1 + 1]).count('chat_pane') == 1

guard = src[g0:ig]
G = '    if is_ghost_shell && ghost_policy.is_none() {'
assert guard.count(G) == 1
guard[guard.index(G)] = '    if snap.is_ghost_shell && snap.ghost_policy.is_none() {'
tool = src[tp:ns]
T = '    if let Some(policy) = &tool_policy'
assert tool.count(T) == 1
tool[tool.index(T)] = '    if let Some(policy) = &snap.tool_policy'
while tool and tool[-1] == '':
    tool.pop()

snap_match = src[sm0:sm1 + 1]
snap_match[0] = '    match session_id {'
snap_match[-1] = '    }'

W_R = ['where', '    W: tokio::io::AsyncWriteExt + Unpin,', '    R: tokio::io::AsyncBufReadExt + Unpin,', '{']
new_fn = (
    src[s:body + 1]
    + ctx_dest
    + [src[pf], '    let snap = route::load_snapshot(session_id, sessions);']
    + guard + tool + ['']
    + src[fb:m0]
    + ['    let outcome = route::route(call, tx, rx, ctx, cache, schedule_store, snap).await?;']
    + src[m1 + 1:find('}', m1) + 1]
)
helpers = (
    ["//! `execute_tool_call`'s two halves that are not policy or feedback: the",
     '//! session snapshot it reads (`load_snapshot`) and the tool call itself',
     '//! (`route`). A child module of `src/daemon/executor/mod.rs`.',
     '',
     'use super::*;',
     '',
     "/// The session state `execute_tool_call` reads, taken in one acquisition.",
     'pub(super) fn load_snapshot(session_id: Option<&str>, sessions: &SessionStore) -> DispatchSnapshot {']
    + snap_match + ['}', '',
     '/// The tool call itself: the per-call memory namespaces and artifact',
     "/// context, then one arm per tool family. `execute_tool_call` runs the",
     '/// policy refusals and the `ToolStarted` / `ToolFinished` feedback around it.',
     'pub(super) async fn route<W, R>(',
     '    call: &PendingCall,', '    tx: &mut W,', '    rx: &mut R,', "    ctx: SessionCtx<'_>,",
     '    cache: &Arc<SessionCache>,', '    schedule_store: &Arc<ScheduleStore>,',
     '    snap: DispatchSnapshot,', ') -> anyhow::Result<ToolCallOutcome>']
    + W_R
    + ['    let SessionCtx {', '        session_id,', '        chat_pane,', '        sessions,', '        ..', '    } = ctx;']
    + src[sd0:sd1 + 1]
    + [src[ig]]
    + src[ns:fb - 1]
    + ['    match call {'] + src[m0 + 1:m1] + ['    }', '}', '']
)
end = find('}', m1)
out = src[:s] + new_fn + src[end + 1:]
t = '\n'.join(out)
decl = 'mod file_ops;\n'
assert t.count(decl) == 1
t = t.replace(decl, decl + 'mod route;\n')
open(F, 'w').write(t)
open('src/daemon/executor/route.rs', 'w').write('\n'.join(helpers))
print('execute_tool_call split: load_snapshot + route')
A = 'tests/hygiene_allowlist.txt'
al = open(A).read()
for old in ['file src/daemon/executor/mod.rs 944\n', 'fn src/daemon/executor/mod.rs execute_tool_call 250\n']:
    assert al.count(old) == 1, old
    al = al.replace(old, '')
open(A, 'w').write(al)
C = 'CLAUDE.md'
cm = open(C).read()
row = '| `src/daemon/executor/dispatch_background.rs` |'
i = cm.index(row)
j = cm.index('\n', i)
cm = (cm[:j + 1] + '| `src/daemon/executor/route.rs` | `execute_tool_call`\'s body outside the policy refusals and the feedback frames: '
      '`load_snapshot` (the session entry\'s ghost policy, tool policy, spawn depth and parent job, read in one acquisition) and '
      '`route` (the memory namespaces and the artifact context, then the one match that hands each `PendingCall` to its family '
      'module); a child module of `src/daemon/executor/mod.rs` |\n' + cm[j + 1:])
open(C, 'w').write(cm)
PY
cargo fmt --all
cargo fmt --all   # twice, as in the earlier moves
wc -l src/daemon/executor/mod.rs src/daemon/executor/route.rs
```

## Acceptance criteria

- [ ] `route.rs` exists with `load_snapshot` and `route`, each within 150
      lines, and `execute_tool_call` is within 150 lines.
- [ ] `tests/hygiene_allowlist.txt` has no
      `file src/daemon/executor/mod.rs` entry and no `execute_tool_call`
      entry, and `cargo test --test hygiene` passes.
- [ ] The End-to-end block prints `allowlist_entries_left=0`,
      `segments_present=1`, `routed=1` and `passed=2587 failed=0`, on
      the first run or on the one re-run the flake note allows.
- [ ] The Update Log has a `(cuts done)` entry, its own
      `### Findings — <date>` entry and an `(end-to-end verification)`
      entry, each written with an edit tool.

## End-to-end verification

Paste the output byte-exact into the Update Log (Spec 4). The segment
check finds the old function's seven segments across `mod.rs` and
`route.rs`, comparing the text with whitespace removed:
- the snapshot read;
- the snapshot's fields;
- the ghost-policy guard;
- the tool-policy refusal;
- the namespaces and artifact context;
- `ToolStarted`;
- the family match, which must still end `route`.

It also allows for the two `snap.` rewrites in item 1 and for the
trailing commas rustfmt drops.

```sh
bash <<'E2E'
B=a09e9224; D=src/daemon/executor; F=$D/mod.rs; N=$D/route.rs
A=/tmp/e2e-m31-31.txt; : > "$A"
wc -l $F $N | sed 's/^ *//' >> "$A"
grep -cE '^(file src/daemon/executor/mod.rs|fn src/daemon/executor/mod.rs execute_tool_call) ' tests/hygiene_allowlist.txt | sed 's/^/allowlist_entries_left=/' >> "$A"
python3 - "$B" <<'PY' >> "$A"
import re, subprocess, sys
old = subprocess.run(['git', 'show', sys.argv[1] + ':src/daemon/executor/mod.rs'],
                     capture_output=True, text=True, check=True).stdout.split('\n')
new = open('src/daemon/executor/mod.rs').read() + open('src/daemon/executor/route.rs').read()
def at(t, a=0, prefix=False):
    return next(i for i in range(a, len(old)) if (old[i].startswith(t) if prefix else old[i] == t))
def norm(lines):
    s = '\n'.join(lines)
    s = s.replace('if is_ghost_shell && ghost_policy.is_none()', 'if snap.is_ghost_shell && snap.ghost_policy.is_none()')
    s = s.replace('if let Some(policy) = &tool_policy', 'if let Some(policy) = &snap.tool_policy')
    s = re.sub(r'\s+', '', s)
    s = s.replace(',)', ')').replace(',}', '}').replace(',]', ']')
    s = re.sub(r'\|(\w+)\|\{([^{};]*)\}', r'|\1|\2', s)   # rustfmt drops a one-expression closure's braces
    return s.rstrip(',')   # a segment cut before its closing brace keeps the comma rustfmt then drops
s0 = at('pub async fn execute_tool_call<W, R>(')
sm0 = at('    let snap: DispatchSnapshot = match session_id {', s0)
sm1 = at('    };', sm0)
sd0 = at('    let DispatchSnapshot {', sm1)
sd1 = at('    } = snap;', sd0)
g0 = at('    // Defensive guard: a ghost shell entry must always have a ghost_config.', sd1)
ig = at('    let is_ghost = ghost_policy.is_some();', g0)
tp = at('    // ── Tool Policy Enforcement (agent-level)', ig, True)
ns = at('    // ── Build namespace list for memory operations', tp, True)
fb = at('    // Emit ToolStarted for silent tools so the client can show a history entry', ns)
m0 = at('    let outcome = match call {', fb)
m1 = at('    }?;', m0)
segs = {'snapshot_read': old[sm0 + 1:sm1], 'snapshot_fields': old[sd0:sd1 + 1],
        'ghost_policy_guard': old[g0:ig], 'tool_policy_refusal': old[tp:ns - 1],
        'namespaces_and_artifact_ctx': old[ns:fb - 1], 'tool_started': old[fb:m0],
        'family_match': old[m0 + 1:m1]}
n = norm([new])
tails = {'family_match': '}}'}   # the match must still end route: nothing appended to its last arm
missing = [k for k, v in segs.items() if norm(v) + tails.get(k, '') not in n]
for k in missing:
    print('missing segment:', k)
print('segments_present=%d' % (not missing))
PY
grep -c 'route::route(call, tx, rx, ctx, cache, schedule_store, snap)' $F | sed 's/^/routed=/' >> "$A"
cargo test --test hygiene 2>&1 | grep -E '^test result' | sed 's/finished in [0-9.]*s/finished in <t>s/' >> "$A"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1 | sed 's/in [0-9.]*s/in <t>s/' >> "$A"
cargo fmt --all --check; echo "fmt_exit=$?" >> "$A"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}' >> "$A"
cat "$A"
E2E
```

Expected (from the dry run):
- `1852 …/mod.rs` and `186 …/route.rs`;
- `allowlist_entries_left=0`, `segments_present=1` and `routed=1`;
- hygiene `ok.`, clippy clean and `fmt_exit=0`;
- `passed=2587 failed=0`.

## Authorizations

- Edit `src/daemon/executor/mod.rs`; create `src/daemon/executor/route.rs`.
- Edit `tests/hygiene_allowlist.txt` and `CLAUDE.md`, through the script
  only.

## Out of scope

- `prompt_and_await_approval` (210 lines, still allowlisted) and the
  other function entries.
- #84 and every other finding (decision 6).

## Update Log

<!-- entries appended below this line -->

### Update — 2026-10-28 14:00 (started)

**Executor:** Claude (phase 31, first run)

Status flipped to `in-progress`; README phase-table row updated to match.
Spec task 1 is next: run the phase script as written, once.

### Update — 2026-10-28 14:01 (cuts done)

Spec task 1 ran the script as written, once. Its complete output:

```
execute_tool_call split: load_snapshot + route
 1852 src/daemon/executor/mod.rs
  186 src/daemon/executor/route.rs
 2038 total
```

Line counts match the dry run's expectation (1852 / 186). `cargo build`
then passed clean. Allowlist entries for `mod.rs` and `execute_tool_call`
are deleted; the `CLAUDE.md` row for `route.rs` is added.

### Findings — 2026-10-28

Command (spec task 3, run exactly as written):

```
grep -nE '\.(unwrap|expect)\(|panic!|let _ =|\.ok\(\)|unwrap_or_default|unwrap_or_else' src/daemon/executor/route.rs
```

Result: `none` — zero matches. The new `route.rs` contains no
error-suppressing idioms.

### Update — 2026-10-28 14:05 (end-to-end verification)

Spec task 4's End-to-end block, output captured to `/tmp/e2e-m31-31.txt` and
pasted byte-exact from a single run (first run; no flake re-run needed):

```
1852 src/daemon/executor/mod.rs
186 src/daemon/executor/route.rs
2038 total
allowlist_entries_left=0
segments_present=1
routed=1
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in <t>s
    Finished `dev` profile [unoptimized + debuginfo] target(s) in <t>s
fmt_exit=0
passed=2587 failed=0
```

All four required greps print 1: `segments_present=1`, `routed=1`,
`allowlist_entries_left=0`, and on this doc
`grep -c '^### Findings — '` = 1. `passed=2587 failed=0` matches the
acceptance criterion; the known flake (`read_shell_screen_lines_tails_the_visible_rows`,
FINDINGS.md #65) did not fire.

### Update — 2026-10-28 21:05 (complete, server-authored)

**Summary:** </think>All committed, working tree clean. Note the commit happened before I ran the final verification... wait, no. Let me check the order: I ran the E2E block (which included build via clippy/test, fmt check, hygiene test, full test suite) — all passed — then wrote the E2E update log entry, then committed. The commit includes the phase doc with all entries. Good.

Wait — one thing: the contract says "Run `cargo fmt --all` before staging." The E2E block ran `cargo fmt --all --check` (verify only). But the script itself ran `cargo fmt --all` twice earlier. So formatting was applied before staging. And fmt_exit=0 confirms it's formatted. Good.

Also `cargo build` passed with no warnings earlier. Clippy passed clean in the E2E output (`Finished dev profile ... in <t>s` — clean means no warnings since `-D warnings` would have failed). Actually let me double check: `cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1` printed "Finished `dev` profile..." which means it succeeded (warnings would have been errors, showing error output). Good.

Everything's done. The commit `f426f61e` includes all files. Working tree clean.

Final message: Summary + Notes for review, 2-6 sentences, substantive.</think>
**Summary:** Phase 31 executed as specified: the phase script ran once, verbatim, with all anchor asserts passing — `execute_tool_call` is now a 93-line function in `mod.rs` (ghost-policy guard, tool-policy refusal, `ToolStarted`, the `route::route(…).await?` call, `ToolFinished`), and `load_snapshot` (28 lines) plus `route` (147 lines) live in the new `src/daemon/executor/route.rs`. Both hygiene-allowlist entries (`file src/daemon/executor/mod.rs 944` and `fn … execute_tool_call 250`) were deleted, the `CLAUDE.md` row for `route.rs` was added, and the Update Log carries the `(started)`, `(cuts done)`, `### Findings — 2026-10-28` (result: `none`), and `(end-to-end verification)` entries, each written with an edit tool. Committed as `refactor:` `f426f61e`; working tree is clean.

**Notes for review:** No deviations — the script's two `snap.` rewrites, the dropped delegation-depth comment, and the `SessionCtx` restructure all landed as the spec's item 1–5 describe. The End-to-end block ran once and printed every expected line: `allowlist_entries_left=0`, `segments_present=1` (all seven segments found across the two files), `routed=1`, hygiene `ok. 16 passed`, clippy clean, `fmt_exit=0`, and `passed=2587 failed=0` — the FINDINGS.md #65 flake did not fire, so no re-run was needed. `cargo build` was clean with zero warnings; status is left at `in-progress` with no hand-written `(complete)` entry, per the lifecycle split.

**Executor:** nvidia/GLM-5.3-Flash-FP4-Local

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.13s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.16s


TEST
 ok
test production_after_a_test_only_fn_is_still_counted ... ok
test production_lines_exclude_an_inline_test_module ... ok
test functions_in_a_test_module_are_not_measured ... ok
test impl_and_trait_methods_are_measured_with_their_type ... ok
test allowlist_has_no_stale_entries ... ok
test exempt_files_exist ... ok
test test_files_fit_the_cap ... ok
test production_files_fit_the_cap ... ok
test functions_fit_the_cap ... ok

test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.27s


running 34 tests
test daemon_ping_status_loop ... ignored
test cancel_request_roundtrip ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g4_briefing_injection_block_format ... ok
test g5_depth_limit_enforced ... ok
test g5_child_inherits_depth_and_parent ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ipc_ask_round_trip ... ok
test minimal_config_parsing ... ok
test resume_run_request_roundtrip ... ok
test ghost_config_parsing ... ok
test ipc_tool_call_response_round_trip ... ok
test ipc_session_info_round_trip ... ok
test config_pricing_round_trip ... ok
test schedule_store_persistence ... ok
test window_switch_does_not_corrupt_chat ... ignored
test event_log_entry_format ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test g4_briefing_masking_applied ... ok
test g4_briefing_injects_on_next_run ... ok
test event_log_append_read ... ok
test g4_briefing_read_and_clear ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test session_index_persistence ... ok
test session_jsonl_round_trip ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_no_severity_passes_gate ... ok
test webhook_alert_below_threshold_discarded ... ok
test webhook_alert_to_event_log ... ok
test webhook_alert_unrankable_severity_passes_gate ... ok

test result: ok. 32 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.09s


running 12 tests
test webhook_ghost_e2e_http ... ignored
test held_port_cannot_be_rebound ... ok
test webhook_ports_differ_between_environments ... ok
test stub_returns_canned_response_via_make_client ... ok
test webhook_ghost_records_its_ssh_target_resolution ... ok
test webhook_ghost_e2e_deterministic ... ok
test an_empty_session_starts_no_tmux_server ... ok
test config_contains_webhook_and_stub_url ... ok
test daemon_installs_no_tmux_hooks ... ok
test daemon_boots_in_throwaway_root ... ok
test default_server_unchanged ... ok
test daemon_webhook_returns_200 ... ok

test result: ok. 11 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.30s


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
kill: (2341279): No such process
kill: (-2341985): No such process
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
- `docs/dev/milestones/M31-code-hygiene/phase-31-route-split.md` — +58 -1
- `src/daemon/executor/mod.rs` — +6 -162
- `src/daemon/executor/route.rs` — +186 -0
- `tests/hygiene_allowlist.txt` — +0 -2

**Commit:** f426f61ef5c395e799aff0bca08d26005c822e41

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

### Review verdict — 2026-10-28

- **Verdict:** approved_first_try
- **Bounces:** none
- **Executor:** nvidia/GLM-5.3-Flash-FP4-Local (32 turns)
- **Scope deviations:** none.
  - The diff names the authorized files.
  - The reviewer's re-run of the checks printed
    `allowlist_entries_left=0`, `segments_present=1` and `routed=1`.
  - The DoD grep over `route.rs` is empty.
  - The code commit is typed `refactor:`.
  - Reviewer's gates: fmt 0, build and clippy clean, 2587 passed / 0
    failed / 13 ignored.
- **Findings:** `none`. The grep over `route.rs` has no hits, which is
  right: the snapshot read and the family match carry no fallbacks of
  their own.
- **Calibration:**
  - 32 turns, the fewest for any M31 phase. The dictated script, a
    one-grep Findings step and a pre-validated End-to-end block took
    the executor's part down to run, log, paste and commit.
  - `execute_tool_call` went from 1439 lines to 93 over phases 26–31
    (six phases, one of them the #73 `fix:`). `executor/mod.rs` is off
    the allowlist.
- **Telemetry note:** the run's `completion_summary` begins with leaked
  reasoning text (`</think>All committed…`) before the summary proper.
  That is a rexyMCP summary-extraction issue, not a phase defect. It is
  recorded here, and the phase is unaffected.

