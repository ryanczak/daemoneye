# Phase 26 — Move `execute_tool_call`'s simple arms into per-family modules

**Status:** review
**Milestone:** M31 — Code hygiene
**Drafted:** 2026-10-28

## Goal

`execute_tool_call` in `src/daemon/executor/mod.rs` is 1439 lines, almost
all of it one `match call`. This phase moves the match arms of 35 tools
into five per-family child modules of the executor. Each module holds one
`dispatch` function whose `match call` carries those arms unchanged:

| Module | Tools |
|---|---|
| `dispatch_schedule.rs` | `schedule_command`, `list_schedules`, `cancel_schedule`, `delete_schedule` |
| `dispatch_artifacts.rs` | the script, runbook and agent tools (12) |
| `dispatch_memory.rs` | the memory tools, `search_repository`, `recall_context`, `await_agent_result` |
| `dispatch_shells.rs` | `list_shells`, `read_shell`, `read_shell_screen`, `find_in_shells`, `close_shell`, `watch_shell`, `list_shell_logs`, `read_shell_log` |
| `dispatch_runs.rs` | `run_log_tail`, `run_log_search`, `get_turn` |

In `execute_tool_call`, each family's arms become one grouped arm
(`PendingCall::A { .. } | PendingCall::B { .. } | … => dispatch_x::dispatch(…)`).
The function drops to 1161 lines and the file to 1848 production lines,
and the script lowers both allowlist entries to those numbers. Behaviour
does not change.

**What stays, and why.** `Foreground`, `Background`, `SendInput`,
`EditFile`, `LoadTools` and `InvalidArgs` move in later phases.
`Foreground` is 392 lines on its own and needs a real split first. Four
arms, `ReadFile`, `GetShellContext`, `OpenShell` and `SpawnGhost`, stay
for a different reason. Each sends `ToolStarted`, and each has an early
`return` that skips the `ToolFinished` sent after the match
(`FINDINGS.md` #73). Inside a function, those returns would flow on to
`ToolFinished`, which is a behaviour change. None of the 35 moved arms
has an early `return`, so for them a function is equivalent: the arm's
value reaches the same `?` and the same feedback code.

## Architecture references

- `docs/dev/milestones/M31-code-hygiene/README.md`: decisions 3, 5, 6, 7.

## Spec

1. **Run the script below as written, once.** It does the whole change:
   - the five files and their `mod` declarations;
   - the grouped arms in `execute_tool_call`;
   - the two allowlist numbers and the `CLAUDE.md` key-files row;
   - then `cargo fmt --all`, twice.

   It was dry-run on a scratch worktree of `77ec8975`. Clippy and the full suite
   (2586) were green, and the End-to-end block printed every expected
   line. Do not hand-edit anything it touches. Do not dry-run it again in
   a worktree of your own. Do not `sed -i` a source file. It is a Python
   heredoc inside a shell script, so copy it **without adding
   indentation**.

   **What it changes inside a moved arm: only one thing.** `artifact_ctx`
   and `memory_namespaces` reach the family functions as references, so
   the moved bodies drop the `&` in front of them; a leftover `&` fails
   clippy's `needless_borrow`. No other line changes. Each family
   `dispatch` matches the whole `PendingCall`, so it ends in
   `other => anyhow::bail!(…)`, an arm no call can reach. The End-to-end
   block's `routing_matches` line proves that each grouped arm in
   `execute_tool_call` lists exactly the variants its module handles.
2. **Log it at once.** Append a `### Update — <date> (cuts done)` entry
   to this doc's Update Log with the patch or write tool, quoting the
   script's output. The stall governor counts only edit-tool calls as
   progress (rexyMCP issue #14).
3. **Findings** (decision 7) go in **their own
   `### Findings — <date>` entry**. Read the five new files, 431 lines,
   and record each hit with file, symbol, kind and severity, or `none`:
   - an `expect`, `unwrap` or `panic!`, at a lock site or anywhere else;
   - a swallowed error, such as `let _ =`, `.ok()` dropped, or
     `unwrap_or_default` on a `Result`;
   - duplicated logic across arms;
   - anything else that looks wrong.

   `FINDINGS.md` #73 (the four arms that skip `ToolFinished`) is already
   recorded. Do not repeat it.
4. **Run the End-to-end block, then paste its output into the Update Log
   with the write or patch tool**, as a `### Update — <date> (end-to-end
   verification)` entry holding `/tmp/e2e-m31-26.txt` in a fenced block.
   **Paste only output you have already seen.** Write the entry after the
   block has run, and add no other "proof" block. The block is a `bash`
   heredoc, so run it as it stands.

   **Known flake:** `read_shell_screen_lines_tails_the_visible_rows`
   fails about one run in ten (`FINDINGS.md` #65). If the block shows
   `passed=2585 failed=1`, run it once more and paste both outputs, with a
   line saying so. Any other failure is real.

   **Before you commit, all three of these must print 1:**
   `grep -c '^bodies_identical=1$'`, `grep -c '^routing_matches=1$'` and
   `grep -c '^### Findings — '` on this doc.
5. **Commit** as `refactor:`.

### The script

```bash
python3 - <<'PY'
import re
F = 'src/daemon/executor/mod.rs'
D = 'src/daemon/executor/'
src = open(F).read().split('\n')
start = next(i for i, l in enumerate(src) if l.startswith('pub async fn execute_tool_call'))
m = next(i for i in range(start, len(src)) if src[i] == '    let outcome = match call {')
mend = next(i for i in range(m, len(src)) if src[i] == '    }?;')
anchors = [i for i in range(m + 1, mend) if re.match(r'        PendingCall::\w+', src[i])]
arms = {}
for k, a in enumerate(anchors):
    b = anchors[k + 1] if k + 1 < len(anchors) else mend
    arms[re.match(r'        PendingCall::(\w+)', src[a]).group(1)] = (a, b)
GEN = '<W, R>'
WHERE = ('where\n    W: tokio::io::AsyncWriteExt + Unpin,\n'
         '    R: tokio::io::AsyncBufReadExt + Unpin,\n')
WHERE_W = 'where\n    W: tokio::io::AsyncWriteExt + Unpin,\n'
FAMILIES = [
    ('schedule', 'the schedule tools', ['ScheduleCommand', 'ListSchedules', 'CancelSchedule', 'DeleteSchedule'],
     'pub(super) async fn dispatch<W, R>(\n    call: &PendingCall,\n    tx: &mut W,\n    rx: &mut R,\n'
     '    session_id: Option<&str>,\n    is_ghost: bool,\n    schedule_store: &Arc<ScheduleStore>,\n'
     ') -> anyhow::Result<ToolCallOutcome>\n' + WHERE,
     'call, tx, rx, session_id, is_ghost, schedule_store', True),
    ('artifacts', 'the script, runbook and agent tools',
     ['WriteScript', 'ListScripts', 'ReadScript', 'DeleteScript', 'WriteRunbook', 'DeleteRunbook',
      'ReadRunbook', 'ListRunbooks', 'CreateAgent', 'ReadAgent', 'ListAgents', 'DeleteAgent'],
     'pub(super) async fn dispatch<W, R>(\n    call: &PendingCall,\n    tx: &mut W,\n    rx: &mut R,\n'
     '    session_id: Option<&str>,\n    is_ghost: bool,\n    schedule_store: &Arc<ScheduleStore>,\n'
     '    artifact_ctx: &knowledge::ArtifactCtx<\'_>,\n) -> anyhow::Result<ToolCallOutcome>\n' + WHERE,
     'call, tx, rx, session_id, is_ghost, schedule_store, &artifact_ctx', True),
    ('memory', 'the memory and recall tools',
     ['AddMemory', 'UpdateMemory', 'DeleteMemory', 'ReadMemory', 'ListMemories', 'SearchRepository',
      'RecallContext', 'AwaitAgentResult'],
     'pub(super) async fn dispatch<W>(\n    call: &PendingCall,\n    tx: &mut W,\n    session_id: Option<&str>,\n'
     '    artifact_ctx: &knowledge::ArtifactCtx<\'_>,\n    memory_namespaces: &[&str],\n'
     ') -> anyhow::Result<ToolCallOutcome>\n' + WHERE_W,
     'call, tx, session_id, &artifact_ctx, &memory_namespaces', True),
    ('shells', 'the shell readers and observers',
     ['ListShells', 'ReadShell', 'ReadShellScreen', 'FindInShells', 'CloseShell', 'WatchShell',
      'ListShellLogs', 'ReadShellLog'],
     'pub(super) async fn dispatch<W>(\n    call: &PendingCall,\n    tx: &mut W,\n    session_id: Option<&str>,\n'
     ') -> anyhow::Result<ToolCallOutcome>\n' + WHERE_W,
     'call, tx, session_id', True),
    ('runs', 'the run-log tools', ['RunLogTail', 'RunLogSearch', 'GetTurn'],
     'pub(super) fn dispatch(call: &PendingCall) -> anyhow::Result<ToolCallOutcome>\n',
     'call', False),
]
remove = set()
insert = {}
for fam, what, names, sig, args, is_async in FAMILIES:
    body = []
    for n in names:
        a, b = arms[n]
        body += src[a:b]
        remove.update(range(a, b))
    while body and body[-1] == '':
        body.pop()
    # The two contexts arrive as references now, so the moved bodies drop
    # their `&` (clippy::needless_borrow); no other line of an arm changes.
    body = [l.replace('&artifact_ctx', 'artifact_ctx').replace('&memory_namespaces', 'memory_namespaces')
            for l in body]
    tools = ', '.join('`' + n + '`' for n in names)
    text = ('//! `execute_tool_call`\'s arms for ' + what + ': ' + tools + '.\n'
            '//! A child module of `src/daemon/executor/mod.rs`.\n\nuse super::*;\n\n'
            + sig + '{\n    match call {\n' + '\n'.join(body) + '\n\n'
            '        other => anyhow::bail!("{} is not one of ' + what + '", other.tool_name()),\n    }\n}\n')
    open(D + 'dispatch_' + fam + '.rs', 'w').write(text)
    first = min(arms[n][0] for n in names)
    pat = '\n        | '.join('PendingCall::' + n + ' { .. }' for n in names)
    insert[first] = ('        ' + pat + ' => dispatch_' + fam + '::dispatch(' + args + ')'
                     + ('.await' if is_async else '') + ',\n')
out = []
for i, l in enumerate(src):
    if i in insert:
        out.append(insert[i])
    if i not in remove:
        out.append(l)
text = '\n'.join(out)
decl = 'pub(crate) mod chat_sandbox;\n'
assert text.count(decl) == 1
text = text.replace(decl, decl + ''.join('mod dispatch_' + f[0] + ';\n' for f in FAMILIES))
open(F, 'w').write(text)
A = 'tests/hygiene_allowlist.txt'
al = open(A).read()
for old, new in [('file src/daemon/executor/mod.rs 2121\n', 'file src/daemon/executor/mod.rs 1848\n'),
                 ('fn src/daemon/executor/mod.rs execute_tool_call 1439\n',
                  'fn src/daemon/executor/mod.rs execute_tool_call 1161\n')]:
    assert al.count(old) == 1, old
    al = al.replace(old, new)
open(A, 'w').write(al)
C = 'CLAUDE.md'
cm = open(C).read().split('\n')
row = next(i for i, l in enumerate(cm) if l.startswith('| `src/daemon/executor/shell_exec_auth.rs` |'))
cm.insert(row + 1, '| `src/daemon/executor/dispatch_schedule.rs`, `dispatch_artifacts.rs`, `dispatch_memory.rs`, '
          '`dispatch_shells.rs`, `dispatch_runs.rs` | `execute_tool_call`\'s arms by tool family, child modules '
          'of `src/daemon/executor/mod.rs`: each `dispatch(call, …)` matches its family\'s `PendingCall` variants '
          '— the schedule tools; scripts, runbooks and agents; memory and recall; the shell readers and observers; '
          'the run-log tools |')
open(C, 'w').write('\n'.join(cm))
PY
cargo fmt --all
cargo fmt --all   # twice, as in the earlier moves
wc -l src/daemon/executor/mod.rs src/daemon/executor/dispatch_*.rs
```

## Acceptance criteria

- [ ] The five `dispatch_*.rs` files exist, each a child module of
      `src/daemon/executor/mod.rs`, and `execute_tool_call` reaches them
      through five grouped arms.
- [ ] The allowlist reads `file src/daemon/executor/mod.rs 1848` and
      `fn src/daemon/executor/mod.rs execute_tool_call 1161`, and
      `cargo test --test hygiene` passes.
- [ ] The End-to-end block prints `bodies_identical=1`,
      `routing_matches=1` and `passed=2586 failed=0`, on the first run or
      on the one re-run the flake note allows.
- [ ] The Update Log has a `(cuts done)` entry, its own
      `### Findings — <date>` entry and an `(end-to-end verification)`
      entry containing the block's output. An edit tool wrote all three,
      and all three are in this file.

## End-to-end verification

Paste the output byte-exact into the Update Log (Spec 4).

```sh
bash <<'E2E'
B=77ec8975; D=src/daemon/executor; F=$D/mod.rs
A=/tmp/e2e-m31-26.txt; : > "$A"
wc -l $F $D/dispatch_*.rs | sed 's/^ *//' >> "$A"
grep -E '^(file|fn) src/daemon/executor/mod.rs (execute_tool_call )?[0-9]+$' tests/hygiene_allowlist.txt >> "$A"
same=1; routed=1
for fam in schedule artifacts memory shells runs; do
  names=$(grep -oE '^        PendingCall::[A-Za-z]+' $D/dispatch_$fam.rs | sed 's/.*:://' | tr '\n' ',')
  old=$(git show $B:$F | awk -v set=",$names" '
    /^    let outcome = match call \{$/ {m=1; next}
    m && /^    \}\?;$/ {m=0}
    m && /^        PendingCall::[A-Za-z]+/ {match($0, /PendingCall::[A-Za-z]+/); v=substr($0, RSTART+13, RLENGTH-13); keep=index(set, "," v ",")>0}
    m && keep' | sed 's/&artifact_ctx/artifact_ctx/g; s/&memory_namespaces/memory_namespaces/g' | tr -d ' \t\n' | sed 's/,)/)/g' | md5sum)
  new=$(awk '/^    match call \{$/ {f=1; next} /^        other => anyhow::bail!/ {f=0} f' $D/dispatch_$fam.rs | tr -d ' \t\n' | sed 's/,)/)/g' | md5sum)
  [ "$old" = "$new" ] || { same=0; echo "body differs: $fam" >> "$A"; }
  grouped=$(awk -v fam="dispatch_$fam::dispatch(" '
    /^        PendingCall::/ {names=""}
    /^        (\| )?PendingCall::[A-Za-z]+ \{ \.\. \}/ {match($0, /PendingCall::[A-Za-z]+/); names=names substr($0, RSTART+13, RLENGTH-13) ","}
    index($0, fam) {print names; exit}' $F)
  [ "$(echo "$grouped" | tr ',' '\n' | sort)" = "$(echo "$names" | tr ',' '\n' | sort)" ] || { routed=0; echo "routing differs: $fam" >> "$A"; }
done
echo "bodies_identical=$same" >> "$A"; echo "routing_matches=$routed" >> "$A"
cargo test --test hygiene 2>&1 | grep -E '^test result' | sed 's/finished in [0-9.]*s/finished in <t>s/' >> "$A"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1 | sed 's/in [0-9.]*s/in <t>s/' >> "$A"
cargo fmt --all --check; echo "fmt_exit=$?" >> "$A"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}' >> "$A"
cat "$A"
E2E
```

Expected (from the dry run):
- the line counts `2840 …/mod.rs`, `97 …/dispatch_artifacts.rs`,
  `113 …/dispatch_memory.rs`, `39 …/dispatch_runs.rs`,
  `64 …/dispatch_schedule.rs` and `113 …/dispatch_shells.rs`;
- the two allowlist lines at 1848 and 1161;
- `bodies_identical=1` and `routing_matches=1`;
- hygiene `ok.`, clippy clean and `fmt_exit=0`;
- `passed=2586 failed=0`.

## Authorizations

- Edit `src/daemon/executor/mod.rs` and create the five
  `src/daemon/executor/dispatch_*.rs` files.
- Edit `tests/hygiene_allowlist.txt` and `CLAUDE.md`, through the script
  only.

## Out of scope

- The arms that stay (see Goal), and #73 itself (decision 6).
- Any change to a moved arm beyond the dropped `&`.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-10-28 09:16 (cuts done)

Ran the spec-1 script as written, once, with `bash`. Its output:

```
  2840 src/daemon/executor/mod.rs
    97 src/daemon/executor/dispatch_artifacts.rs
   113 src/daemon/executor/dispatch_memory.rs
    39 src/daemon/executor/dispatch_runs.rs
    64 src/daemon/executor/dispatch_schedule.rs
   113 src/daemon/executor/dispatch_shells.rs
  3266 total
```

The line counts match the dry run's expectation exactly (2840 / 97 / 113 /
39 / 64 / 113). `cargo fmt --all` ran twice, as the script's tail does.
`git diff --stat` names five modified files (`CLAUDE.md`,
`README.md`, this doc, `src/daemon/executor/mod.rs`,
`tests/hygiene_allowlist.txt`) and the five new `dispatch_*.rs` files —
nothing hand-edited, nothing dry-run again.

### Findings — 2026-10-28

Decision 7, scoped to the 426 lines of the five new files
(`dispatch_schedule.rs` 64, `dispatch_artifacts.rs` 97,
`dispatch_memory.rs` 113, `dispatch_shells.rs` 113,
`dispatch_runs.rs` 39). Searched with `grep` for the spec's signals
(`unwrap`, `expect`, `panic!`, `let _ =`, `.ok()`,
`unwrap_or_default`), then read each hit's context; the four bodies of
`dispatch_runs.rs` and the `RecallContext` arm read whole.

**1. `expect`, `unwrap` or `panic!` at a lock site or anywhere else —
none.** No `expect(`, no `.unwrap()` on a lock or anything else, no
`panic!` in any of the five files. The only `unwrap`-family hits are
the three `session_id.unwrap_or_default()` in `dispatch_shells.rs`
(18, 66, 73), which are `Option<&str>::unwrap_or_default` — a value
default, not a swallowed error; they are pre-existing and quoted under 2.

**2. Swallowed errors — three hits, all pre-existing `unwrap_or_default`
on an `Option`, not on a `Result`:**

- `dispatch_shells.rs:18` (`ListShells`),
- `dispatch_shells.rs:66` (`FindInShells`),
- `dispatch_shells.rs:73` (`CloseShell`),

each `&Owner::Chat(session_id.unwrap_or_default().to_string())`. When
`session_id` is `None` the owner becomes the empty-string chat owner
rather than an error; identical lines exist at 77ec8975:mod.rs 655 and
975, so the move carried them verbatim (modulo the dropped `&`). Not a
regression. Flagged as a candidate for the `GetShellContext`/owner
plumbing pass (decision 6's later phases).

A fourth near-hit, noted not counted: the `RecallContext` arm
(`dispatch_memory.rs:82-99`) builds `LimitsConfig::default()` instead of
reading the configured limits — pre-existing at 77ec8975:mod.rs 1410,
carried verbatim. It is a silent default, though not of a `Result`
(the `recall` call's own error is converted to a visible
`Error: …` string, so nothing is swallowed there).

**3. Duplicated logic across arms — one group, pre-existing:**

- `dispatch_runs.rs` repeats, in all three of its arms, the
  `match crate::daemon::run::RunId::parse(run_id) { Err(e) =>
  run_log_input_rejected(&e.to_string()), Ok(id) => … }` shape
  (lines 8-10, 20-22, 32-34), differing only in the `Ok` call. A
  helper taking the parsed `RunId` closure would collapse it. It came
  over from 77ec8975:mod.rs verbatim.

No new duplication was introduced by the move.

**4. Anything else that looks wrong — none.** The `other =>
anyhow::bail!(…)` fallback arm in each module is unreachable for any
call that reaches its grouped arm in `execute_tool_call` (the End-to-end
block's `routing_matches=1` proves the grouping covers exactly these
variants), so it is a compiler-required totality arm, not dead logic.
#73 (the four `ToolStarted` arms with early `return`) is recorded
already; none of the 35 moved arms has an early `return`.

### Update — 2026-10-28 09:20 (end-to-end verification)

Spec 4's block, run once as written; first run, no flake re-run needed.
The block wrote `/tmp/e2e-m31-26.txt`; its contents, byte-exact:

```
2840 src/daemon/executor/mod.rs
97 src/daemon/executor/dispatch_artifacts.rs
113 src/daemon/executor/dispatch_memory.rs
39 src/daemon/executor/dispatch_runs.rs
64 src/daemon/executor/dispatch_schedule.rs
113 src/daemon/executor/dispatch_shells.rs
3266 total
file src/daemon/executor/mod.rs 1848
fn src/daemon/executor/mod.rs execute_tool_call 1161
bodies_identical=1
routing_matches=1
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in <t>s
    Finished `dev` profile [unoptimized + debuginfo] target(s) in <t>s
fmt_exit=0
passed=2586 failed=0
```

Every line matches the dry run's expectation: the line counts (2840 /
97 / 113 / 39 / 64 / 113), the two allowlist lines at 1848 and 1161,
`bodies_identical=1` and `routing_matches=1`, hygiene `ok.`, clippy
clean, `fmt_exit=0`, and `passed=2586 failed=0`.

### Update — 2026-10-28 16:23 (complete, server-authored)

**Summary:** **Summary:** Ran the spec-1 script as written, once — it created the five per-family modules `dispatch_schedule.rs` (64), `dispatch_artifacts.rs` (97), `dispatch_memory.rs` (113), `dispatch_shells.rs` (113), `dispatch_runs.rs` (39) as child modules of `src/daemon/executor/mod.rs`, replaced their 35 arms in `execute_tool_call` with five grouped arms, lowered the allowlist to `1848`/`1161` and added the `CLAUDE.md` key-files row; nothing hand-edited. Logged the `(cuts done)` entry, a separate `### Findings — 2026-10-28` entry (no `expect`/`unwrap`/`panic!`; three pre-existing `session_id.unwrap_or_default()` owner defaults in `dispatch_shells.rs` flagged, plus the pre-existing `LimitsConfig::default()` in the `RecallContext` arm and the thrice-repeated `RunId::parse` shape in `dispatch_runs.rs`; no new duplication), and the `(end-to-end verification)` entry with `/tmp/e2e-m31-26.txt` byte-exact.

**Notes for review:** No deviations from the spec. End-to-end on the first run, no flake re-run: `bodies_identical=1`, `routing_matches=1`, allowlist lines at 1848/1161, hygiene `ok.`, clippy clean, `fmt_exit=0`, `passed=2586 failed=0` — every line matching the dry run's expectation. The three spec-4 pre-commit greps all printed 1. Committed as `refactor:` (75f2cae9), working tree clean, phase doc and README row left at `in-progress` for the server's completion tail.

**Executor:** nvidia/GLM-5.3-Flash-FP4-Local

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
   Compiling daemoneye v2.3.0 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 11.09s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.16s


TEST
to_closing_brace ... ok
test duplicate_names_get_a_suffix ... ok
test impl_and_trait_methods_are_measured_with_their_type ... ok
test functions_in_a_test_module_are_not_measured ... ok
test production_lines_exclude_an_inline_test_module ... ok
test exempt_files_exist ... ok
test allowlist_has_no_stale_entries ... ok
test functions_fit_the_cap ... ok
test production_files_fit_the_cap ... ok
test test_files_fit_the_cap ... ok

test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.26s


running 34 tests
test daemon_ping_status_loop ... ignored
test cancel_request_roundtrip ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g4_briefing_injection_block_format ... ok
test g5_depth_limit_enforced ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test g5_child_inherits_depth_and_parent ... ok
test ipc_session_info_round_trip ... ok
test ipc_ask_round_trip ... ok
test minimal_config_parsing ... ok
test ghost_config_parsing ... ok
test resume_run_request_roundtrip ... ok
test schedule_store_persistence ... ok
test config_pricing_round_trip ... ok
test ipc_tool_call_response_round_trip ... ok
test window_switch_does_not_corrupt_chat ... ignored
test g4_briefing_masking_applied ... ok
test event_log_entry_format ... ok
test event_log_append_read ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test g4_briefing_injects_on_next_run ... ok
test g6_agent_namespace_field_persisted ... ok
test g4_briefing_read_and_clear ... ok
test g6_agent_config_roundtrip ... ok
test session_index_persistence ... ok
test session_jsonl_round_trip ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_no_severity_passes_gate ... ok
test webhook_alert_to_event_log ... ok
test webhook_alert_below_threshold_discarded ... ok
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
test daemon_boots_in_throwaway_root ... ok
test daemon_installs_no_tmux_hooks ... ok
test default_server_unchanged ... ok
test daemon_webhook_returns_200 ... ok

test result: ok. 11 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.32s


running 4 tests
test gate_catches_a_chained_write ... ok
test gate_catches_an_abbreviated_write ... ok
test gate_catches_a_multi_line_write ... ok
test source_issues_no_tmux_write_verbs ... ok

test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.17s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-93da9e6ee623fb89)
kill: (2083693): No such process
kill: (-2084403): No such process
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
- `docs/dev/milestones/M31-code-hygiene/phase-26-dispatch-families.md` — +107 -1
- `src/daemon/executor/dispatch_artifacts.rs` — +97 -0
- `src/daemon/executor/dispatch_memory.rs` — +113 -0
- `src/daemon/executor/dispatch_runs.rs` — +39 -0
- `src/daemon/executor/dispatch_schedule.rs` — +64 -0
- `src/daemon/executor/dispatch_shells.rs` — +113 -0
- `src/daemon/executor/mod.rs` — +52 -325
- `tests/hygiene_allowlist.txt` — +2 -2

**Commit:** 75f2cae93d02148eb5e1a5cd034258f4cbde90cf

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).
