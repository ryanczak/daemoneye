# Phase 30 — Split `run_terminal_command`'s background arm into stages

**Status:** done
**Milestone:** M31 — Code hygiene
**Drafted:** 2026-10-28

## Goal

The `PendingCall::Background` arm of `execute_tool_call`
(`src/daemon/executor/mod.rs`), the `wait: false` path, is 163 lines. It
moves into a new child module, `dispatch_background.rs`, in two
functions:

| Function | What it holds | Lines |
|---|---|---|
| `dispatch` | the chat gate, the approval prompt, the ghost route through `foreground::run_background` | 72 |
| `start_approved` | the owner, the chat sandbox or a plain dedicated shell, the dispatch reply and the completion watcher, or the start-error reply | 121 |

`execute_tool_call` drops from 409 to 250 lines and the file from 1102 to
944 production lines. The script lowers both allowlist entries. The
code before the match (about 130 lines) is the next phase.

**What differs from a move.** Two things, both dry-run:

1. **The approved branch's two tail returns lose `return`.**
   `return Ok(ToolCallOutcome::Result(text));` and the same for `msg`
   become `Ok(ToolCallOutcome::Result(text))` and `…(msg))`. In
   `start_approved` they end the function, where clippy's
   `needless_return` would reject the keyword. The value is the same.
   `dispatch` returns it with `return start_approved(…).await;` from the
   `Ok(cmd_id)` arm, as the old arm returned from inside it.
2. **`dispatch` takes the `SessionCtx`** and destructures `session_id`
   and `sessions` from it, as `execute_tool_call` does. It passes `ctx`
   whole to `run_background`, as the old arm did.

`run_terminal_command` is not a feedback tool, so `ToolFinished` is not
involved.

## Architecture references

- `docs/dev/milestones/M31-code-hygiene/README.md`: decisions 3, 5, 6, 7.
- `docs/dev/milestones/M31-code-hygiene/phase-29-foreground-split.md`: the
  same split, applied to the foreground arm.

## Spec

1. **Run the script below as written, once.** It does the whole change:
   - the new file and its `mod` line;
   - the one-line arm left in `execute_tool_call`;
   - the two allowlist numbers and the `CLAUDE.md` rows;
   - then `cargo fmt --all`, twice.

   It was dry-run on a scratch worktree of `3003b8f8`: clippy was clean, the
   full suite passed 2587, and the End-to-end block printed every expected
   line. Do not hand-edit anything it touches. Do not dry-run it again in
   a worktree of your own. Do not `sed -i` a source file. It is a Python
   heredoc inside a shell script, so copy it **without adding
   indentation**. The script asserts its anchors and prints
   `tail returns rewritten: 2`. If an assert fires, stop and report it in
   the Update Log.
2. **Log it at once.** Append a `### Update — <date> (cuts done)` entry
   with the patch or write tool, quoting the script's output. The stall
   governor counts only edit-tool calls as progress (rexyMCP issue #14).
3. **Findings** (decision 7) go in **their own
   `### Findings — <date>` entry, within five tool calls.** Run exactly
   this command, and open no other file:

   `grep -nE '\.(unwrap|expect)\(|panic!|let _ =|\.ok\(\)|unwrap_or_default|unwrap_or_else' src/daemon/executor/dispatch_background.rs`

   Write each hit with its function, kind and severity, or write `none`.
   Do not look up callees: a finding names what this file does. These
   are recorded already, so do not repeat them:
   - #75: `Owner::Chat(unwrap_or_default)`;
   - #84: `Config::load().unwrap_or_default()`.
4. **Run the End-to-end block, then paste its output into the Update Log
   with an edit tool**, as a `### Update — <date> (end-to-end
   verification)` entry holding `/tmp/e2e-m31-30.txt` in a fenced block.
   **Paste only output you have already seen.**

   **Known flake:** `read_shell_screen_lines_tails_the_visible_rows`
   (`FINDINGS.md` #65). If the block shows `passed=2586 failed=1`, run it
   once more and paste both outputs. Any other failure is real.

   **Before you commit, all three of these must print 1:**
   `grep -c '^segments_present=1$'` and `grep -c '^routed=1$'` on the
   output file, and `grep -c '^### Findings — '` on this doc.
5. **Commit** as `refactor:`.

### The script

```bash
python3 - <<'PY'
F = 'src/daemon/executor/mod.rs'
N = 'src/daemon/executor/dispatch_background.rs'
src = open(F).read().split('\n')


def find(text, start=0, end=None):
    end = len(src) if end is None else end
    hits = [i for i in range(start, end) if src[i] == text]
    assert hits, text
    return hits[0]


a0 = find('        PendingCall::Background { id, cmd, host, .. } => {')
a1 = find('        PendingCall::ScheduleCommand { .. }', a0)
ok = find('                    Ok(cmd_id) => {', a0, a1)
ms = find('                        match started {', ok, a1)
me = find('                        }', ms, a1)            # closes `match started`
assert src[me + 1] == '                    }'           # closes `Ok(cmd_id) => {`
assert src[a1 - 2] == '        }' and src[a1 - 1] == ''
RET = {'                                return Ok(ToolCallOutcome::Result(text));':
           '                                Ok(ToolCallOutcome::Result(text))',
       '                                return Ok(ToolCallOutcome::Result(msg));':
           '                                Ok(ToolCallOutcome::Result(msg))'}
body = src[ok + 1:me + 1]
n = 0
for i, l in enumerate(body):
    if l in RET:
        body[i] = RET[l]
        n += 1
assert n == 2, n
print('tail returns rewritten:', n)

W_R = ('where\n    W: tokio::io::AsyncWriteExt + Unpin,\n'
       '    R: tokio::io::AsyncBufReadExt + Unpin,\n')
text = '\n'.join([
    "//! `execute_tool_call`'s `run_terminal_command` background arm",
    '//! (`PendingCall::Background`, `wait: false`), in two stages: the chat',
    "//! gate, the approval and the ghost route (`dispatch`), and the approved",
    "//! start (`start_approved`). A child module of `src/daemon/executor/mod.rs`.",
    '',
    'use super::*;',
    '',
    'pub(super) async fn dispatch<W, R>(',
    '    call: &PendingCall,',
    '    tx: &mut W,',
    '    rx: &mut R,',
    "    ctx: SessionCtx<'_>,",
    '    ghost_policy: &Option<GhostPolicy>,',
    '    is_ghost: bool,',
    ') -> anyhow::Result<ToolCallOutcome>',
    W_R + '{',
    '    let SessionCtx {',
    '        session_id,',
    '        sessions,',
    '        ..',
    '    } = ctx;',
    '    match call {',
    *src[a0:ok + 1],
    '                        return start_approved(id, cmd, cmd_id, session_id, sessions, tx).await;',
    *src[me + 1:a1 - 1],
    '        other => anyhow::bail!("{} is not run_terminal_command\'s background arm", other.tool_name()),',
    '    }',
    '}',
    '',
    "/// The approved start: the owner, the chat sandbox or the plain dedicated",
    '/// shell, then the dispatch reply and the completion watcher, or the',
    '/// start-error reply.',
    'async fn start_approved<W>(',
    '    id: &str,',
    '    cmd: &str,',
    '    cmd_id: usize,',
    "    session_id: Option<&str>,",
    '    sessions: &SessionStore,',
    '    tx: &mut W,',
    ') -> anyhow::Result<ToolCallOutcome>',
    'where\n    W: tokio::io::AsyncWriteExt + Unpin,\n{',
    *body,
    '}',
    '',
])
open(N, 'w').write(text)
call = ('        PendingCall::Background { .. } => {\n'
        '            dispatch_background::dispatch(call, tx, rx, ctx, &ghost_policy, is_ghost).await\n'
        '        }')
out = src[:a0] + [call] + src[a1 - 1:]
t = '\n'.join(out)
decl = 'mod dispatch_artifacts;\n'
assert t.count(decl) == 1
t = t.replace(decl, decl + 'mod dispatch_background;\n')
open(F, 'w').write(t)
A = 'tests/hygiene_allowlist.txt'
al = open(A).read()
for old, new in [('file src/daemon/executor/mod.rs 1102\n', 'file src/daemon/executor/mod.rs 944\n'),
                 ('fn src/daemon/executor/mod.rs execute_tool_call 409\n',
                  'fn src/daemon/executor/mod.rs execute_tool_call 250\n')]:
    assert al.count(old) == 1, old
    al = al.replace(old, new)
open(A, 'w').write(al)
C = 'CLAUDE.md'
cm = open(C).read()
old = " `run_terminal_command`'s background arm stays in `execute_tool_call` |"
assert cm.count(old) == 1
cm = cm.replace(old, " |")
row = '| `src/daemon/executor/dispatch_foreground.rs` |'
i = cm.index(row)
j = cm.index('\n', i)
cm = (cm[:j + 1] + '| `src/daemon/executor/dispatch_background.rs` | `run_terminal_command`\'s background arm '
      '(`wait: false`) in two stages: `dispatch` (the chat gate, the approval prompt, then the ghost route through '
      '`foreground::run_background`) and `start_approved` (the chat sandbox or a plain dedicated shell, the dispatch '
      'reply and the completion watcher, or the start-error reply) |\n' + cm[j + 1:])
open(C, 'w').write(cm)
PY
cargo fmt --all
cargo fmt --all   # twice, as in the earlier moves
wc -l src/daemon/executor/mod.rs src/daemon/executor/dispatch_background.rs
```

## Acceptance criteria

- [ ] `dispatch_background.rs` exists with `dispatch` and
      `start_approved`, each within 150 lines. `execute_tool_call`'s
      `Background` arm is one call to `dispatch_background::dispatch`.
- [ ] The allowlist reads `file src/daemon/executor/mod.rs 944` and
      `fn src/daemon/executor/mod.rs execute_tool_call 250`, and
      `cargo test --test hygiene` passes.
- [ ] The End-to-end block prints `segments_present=1`, `routed=1` and
      `passed=2587 failed=0`, on the first run or on the one re-run the
      flake note allows.
- [ ] The Update Log has a `(cuts done)` entry, its own
      `### Findings — <date>` entry and an `(end-to-end verification)`
      entry, each written with an edit tool.

## End-to-end verification

Paste the output byte-exact into the Update Log (Spec 4). The segment
check finds the arm's three original segments in the new file, comparing
the text with whitespace removed. It also allows for three things:
- rustfmt dropping a trailing comma after a dedent;
- rustfmt dropping a one-expression closure's braces;
- the one dictated rewrite in item 1 above.

```sh
bash <<'E2E'
B=3003b8f8; D=src/daemon/executor; F=$D/mod.rs; N=$D/dispatch_background.rs
A=/tmp/e2e-m31-30.txt; : > "$A"
wc -l $F $N | sed 's/^ *//' >> "$A"
grep -E '^(file|fn) src/daemon/executor/mod.rs (execute_tool_call )?[0-9]+$' tests/hygiene_allowlist.txt >> "$A"
python3 - "$B" <<'PY' >> "$A"
import re, subprocess, sys
old = subprocess.run(['git', 'show', sys.argv[1] + ':src/daemon/executor/mod.rs'],
                     capture_output=True, text=True, check=True).stdout.split('\n')
new = open('src/daemon/executor/dispatch_background.rs').read()
def at(t, a=0):
    return next(i for i in range(a, len(old)) if old[i] == t)
def norm(lines):
    s = '\n'.join(lines)
    s = re.sub(r'return (Ok\(ToolCallOutcome::Result\((text|msg)\)\));', r'\1', s)
    s = re.sub(r'\s+', '', s)
    s = s.replace(',)', ')').replace(',}', '}').replace(',]', ']')
    return re.sub(r'\|(\w+)\|\{([^{};]*)\}', r'|\1|\2', s)   # rustfmt drops a one-expression closure's braces
a0 = at('        PendingCall::Background { id, cmd, host, .. } => {')
a1 = at('        PendingCall::ScheduleCommand { .. }', a0)
ok = at('                    Ok(cmd_id) => {', a0)
ms = at('                        match started {', ok)
me = at('                        }', ms)
segs = {'gate_and_approval': old[a0:ok + 1], 'approved_start': old[ok + 1:me + 1],
        'ghost_route': old[me + 2:a1 - 2]}
n = norm([new])
missing = [k for k, v in segs.items() if norm(v) not in n]
for k in missing:
    print('missing segment:', k)
print('segments_present=%d' % (not missing))
PY
grep -c 'dispatch_background::dispatch(' $F | sed 's/^/routed=/' >> "$A"
cargo test --test hygiene 2>&1 | grep -E '^test result' | sed 's/finished in [0-9.]*s/finished in <t>s/' >> "$A"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1 | sed 's/in [0-9.]*s/in <t>s/' >> "$A"
cargo fmt --all --check; echo "fmt_exit=$?" >> "$A"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}' >> "$A"
cat "$A"
E2E
```

Expected (from the dry run):
- `2008 …/mod.rs` and `204 …/dispatch_background.rs`;
- the two allowlist lines at 944 and 250;
- `segments_present=1` and `routed=1`;
- hygiene `ok.`, clippy clean and `fmt_exit=0`;
- `passed=2587 failed=0`.

## Authorizations

- Edit `src/daemon/executor/mod.rs`; create
  `src/daemon/executor/dispatch_background.rs`.
- Edit `tests/hygiene_allowlist.txt` and `CLAUDE.md`, through the script
  only.

## Out of scope

- `execute_tool_call`'s code before the match: the next phase.
- #84 and every other finding (decision 6).

## Update Log

<!-- entries appended below this line -->

### Update — 2026-10-28 12:44 (started)

**Executor:** Claude (executor run, phase 30).

Status flipped to `in-progress` and the milestone README's phase-30 row
updated to match. Running the phase script next.

### Update — 2026-10-28 12:45 (cuts done)

The phase script ran once, as written, unmodified. It asserted every
anchor and printed:

```
tail returns rewritten: 2
 2008 src/daemon/executor/mod.rs
  204 src/daemon/executor/dispatch_background.rs
 2212 total
```

2008 and 204 are the `wc -l` counts rustfmt leaves after the two
`cargo fmt --all` passes; the production-line allowlist entries are 944
and 250 as specified. The new `mod dispatch_background;` line sits
directly under `mod dispatch_artifacts;`, the `Background` arm in
`execute_tool_call` is the single `dispatch_background::dispatch(...)`
call, the two allowlist numbers are 944/250, and the `CLAUDE.md`
background-arm row now points at `dispatch_background.rs` (the old
"stays in `execute_tool_call`" row text replaced). No source file was
touched by hand.

### Findings — 2026-10-28

`grep -nE '\.(unwrap|expect)\(|panic!|let _ =|\.ok\(\)|unwrap_or_default|unwrap_or_else' src/daemon/executor/dispatch_background.rs`

- 95 `start_approved`: `Owner::Chat(session_id.unwrap_or_default())` —
  unwrap_or_default — recorded, #75.
- 97 `start_approved`: `Config::load().unwrap_or_default()` for
  `max_per_owner` — unwrap_or_default — recorded, #84.
- 101 `start_approved`: `Config::load().unwrap_or_default()` for the
  sandbox flag — unwrap_or_default — recorded, #84.
- 107 `start_approved`: `session_id.unwrap_or_default()` passed as the
  sandboxed start's session id — unwrap_or_default — same family as #75
  (dead value behind a gate that guarantees `session_id.is_some()`).
- 151 `start_approved`: `session_id.map(...).unwrap_or_default()` feeding
  `await_notify_and_close` — unwrap_or_default — same family as #75.
- 156 `start_approved`: `let _ = close_shell_with(...)` inside the
  spawned closer — discarded result — same family as the recorded
  fire-and-forget closers (#75's arm); the failure it drops is the
  best-effort shell close on background completion.
- 168 `start_approved`: `session_id.map(...).unwrap_or_default()` feeding
  `await_and_notify` — unwrap_or_default — same family as #75.
- 178 `start_approved`: `registry_lookup(&owner, ...).unwrap_or_else(`
  `"s?"`)` in the start-error reply — unwrap_or_else fallback string —
  new (not in the recorded list): the error path fabricates a placeholder
  shell id `s?` when the registry has no entry for the owner. Severity:
  low, model-visible only as a cosmetic id in the error text, but it is
  a silent fallback on a lookup that should not miss.

All hits are in `start_approved`; `dispatch` is clean. No `unwrap()`,
`expect()`, `panic!` or `.ok()` in the file.

### Update — 2026-10-28 12:49 (end-to-end verification)

`bash /tmp/e2e-m31-30.sh` — the phase doc's End-to-end block, run once,
unmodified. First run: `segments_present=1`, `routed=1`, `passed=2587
failed=0` — the known flake (#65) did not appear. `/tmp/e2e-m31-30.txt`:

```
2008 src/daemon/executor/mod.rs
204 src/daemon/executor/dispatch_background.rs
2212 total
file src/daemon/executor/mod.rs 944
fn src/daemon/executor/mod.rs execute_tool_call 250
segments_present=1
routed=1
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in <t>s
    Finished `dev` profile [unoptimized + debuginfo] target(s) in <t>s
fmt_exit=0
passed=2587 failed=0
```

Pre-commit gate checks, all three printing 1: `grep -c
'^segments_present=1$' /tmp/e2e-m31-30.txt` → 1; `grep -c '^routed=1$'
/tmp/e2e-m31-30.txt` → 1; `grep -c '^### Findings — '` on this doc → 1.

### Update — 2026-10-28 19:51 (complete, server-authored)

**Summary:** **Summary:** Phase 30 executed as specified: the phase script ran once, unmodified, asserted every anchor, printed `tail returns rewritten: 2`, and produced `src/daemon/executor/dispatch_background.rs` (204 lines, `dispatch` + `start_approved`) with `execute_tool_call`'s `Background` arm reduced to the single `dispatch_background::dispatch(...)` call; `mod.rs` is 2008 wc-lines / 944 production lines and `execute_tool_call` 250. Allowlist now reads `file …mod.rs 944` / `fn …execute_tool_call 250` and the `CLAUDE.md` row points at the new module. The End-to-end block ran once, first try clean: `segments_present=1`, `routed=1`, hygiene `ok. 16 passed`, clippy clean, `fmt_exit=0`, `passed=2587 failed=0` — the #65 flake did not appear, so no re-run was needed. All three pre-commit greps printed 1; committed as `dc366c6f` (`refactor:`), tree clean, status left `in-progress` for the server's completion tail.

**Notes for review:**
- Findings entry (within five calls, per decision 7): eight grep hits, all in `start_approved`; `dispatch` is clean. Six are the recorded #75/#84 families (three `session_id.unwrap_or_default()` variants plus the `let _ =` on the spawned closer's `close_shell_with`). One hit is **new**: line 178, `registry_lookup(&owner, …).unwrap_or_else(|| "s?")` in the start-error reply — a silent placeholder shell id when the registry misses the owner; low severity (cosmetic in the error text), not fixed per decision 6.
- The script's `wc -l` output (2008/204) differs from the allowlist's production-line numbers (944/250) because rustfmt expanded a few multi-line constructs in the moved body — the allowlist counts are the ones the hygiene gate enforces, and they match the spec.
- The end-to-end transcript is the file `/tmp/e2e-m31-30.txt` captured by the phase's own block and pasted verbatim in the `(end-to-end verification)` Update Log entry; it lives outside the repo, so it is not in the commit — only the doc entry and the code are.

**Executor:** nvidia/GLM-5.3-Flash-FP4-Local

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
   Compiling daemoneye v2.3.0 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 11.63s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.16s


TEST
est function_length_spans_signature_to_closing_brace ... ok
test production_after_a_test_only_fn_is_still_counted ... ok
test production_lines_exclude_an_inline_test_module ... ok
test impl_and_trait_methods_are_measured_with_their_type ... ok
test allowlist_has_no_stale_entries ... ok
test exempt_files_exist ... ok
test test_files_fit_the_cap ... ok
test production_files_fit_the_cap ... ok
test functions_fit_the_cap ... ok

test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.28s


running 34 tests
test daemon_ping_status_loop ... ignored
test cancel_request_roundtrip ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g5_child_inherits_depth_and_parent ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g4_briefing_injection_block_format ... ok
test ghost_config_parsing ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test ipc_ask_round_trip ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ipc_session_info_round_trip ... ok
test g5_depth_limit_enforced ... ok
test ipc_tool_call_response_round_trip ... ok
test minimal_config_parsing ... ok
test resume_run_request_roundtrip ... ok
test schedule_store_persistence ... ok
test config_pricing_round_trip ... ok
test window_switch_does_not_corrupt_chat ... ignored
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test g4_briefing_masking_applied ... ok
test event_log_entry_format ... ok
test g4_briefing_read_and_clear ... ok
test event_log_append_read ... ok
test g6_agent_config_roundtrip ... ok
test g4_briefing_injects_on_next_run ... ok
test g6_agent_namespace_field_persisted ... ok
test session_index_persistence ... ok
test session_jsonl_round_trip ... ok
test webhook_alert_to_event_log ... ok
test webhook_alert_below_threshold_discarded ... ok
test webhook_alert_no_severity_passes_gate ... ok
test webhook_alert_unrankable_severity_passes_gate ... ok
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
test daemon_installs_no_tmux_hooks ... ok
test daemon_boots_in_throwaway_root ... ok
test default_server_unchanged ... ok
test daemon_webhook_returns_200 ... ok

test result: ok. 11 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.30s


running 4 tests
test gate_catches_a_chained_write ... ok
test gate_catches_a_multi_line_write ... ok
test gate_catches_an_abbreviated_write ... ok
test source_issues_no_tmux_write_verbs ... ok

test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.17s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-93da9e6ee623fb89)
kill: (2291754): No such process
kill: (-2292456): No such process
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

- `CLAUDE.md` — +2 -1
- `docs/dev/milestones/M31-code-hygiene/README.md` — +1 -1
- `docs/dev/milestones/M31-code-hygiene/phase-30-background-split.md` — +85 -1
- `src/daemon/executor/dispatch_background.rs` — +204 -0
- `src/daemon/executor/mod.rs` — +3 -161
- `tests/hygiene_allowlist.txt` — +2 -2

**Commit:** dc366c6f02372b49c6e54caae7edd4443f2d8fa6

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).

### Review verdict — 2026-10-28

- **Verdict:** approved_first_try
- **Bounces:** none
- **Executor:** nvidia/GLM-5.3-Flash-FP4-Local (51 turns)
- **Scope deviations:** none.
  - The diff names the authorized files.
  - The reviewer's re-run of the checks printed `segments_present=1` and
    `routed=1`.
  - `dispatch` is 72 lines and `start_approved` 121, both within the
    cap.
  - The DoD grep over `dispatch_background.rs` is empty.
  - The code commit is typed `refactor:`.
  - Reviewer's gates: fmt 0, build and clippy clean, 2587 passed / 0
    failed / 13 ignored.
- **Findings:** eight hits, all moved verbatim, and the executor named
  the known families. Two corrections:
  - The `let _ = close_shell_with(…)` in the spawned closer is not
    #75's family. It drops a best-effort close result without a log
    line, which is the shape of #77, so it is carried there.
  - The `"s?"` fallback in the start-error text is cosmetic. Phase 29
    left the same idiom uncarried in `finish_run`, so it is not carried
    here either.
- **Calibration:** 51 turns, the fewest for a split in M31. The
  dictated-script pattern with a one-grep Findings step is now settled
  for production splits.
- **Cosmetic carry-over:** the two `format!` literals keep their old
  continuation indentation. A `\` continuation strips leading
  whitespace and rustfmt does not reflow string literals, so the text
  and the output are unchanged.

