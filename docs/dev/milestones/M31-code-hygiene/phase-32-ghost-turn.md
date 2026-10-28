# Phase 32 — Move `do_ghost_turn` out of `ghost.rs`

**Status:** review
**Milestone:** M31 — Code hygiene
**Drafted:** 2026-10-28

## Goal

`src/daemon/ghost.rs` has 1677 production lines, 897 of them one
function, `do_ghost_turn`. This phase makes `ghost.rs` a file with two
child modules and brings it under the cap:

| File | What it holds | Production lines |
|---|---|---|
| `ghost.rs` | `GhostManager`, `trigger_ghost_turn`, the helpers, the tests | 1677 → 782 |
| `ghost/turn.rs` | `do_ghost_turn`: the client, the turn loop, the completion bookkeeping | 782 (new) |
| `ghost/prompt.rs` | `build_ghost_prompt`: the ghost's system prompt | 152 (new) |

Decision 3 says nothing new joins the allowlist, so `do_ghost_turn`
cannot move whole: the new file would be 897 lines. Its 126-line prompt
assembly becomes `build_ghost_prompt` (138 lines) in `prompt.rs`, and
the rest moves to `turn.rs`. The allowlist loses
`file src/daemon/ghost.rs 1677`, and `do_ghost_turn`'s entry follows the
function: `fn src/daemon/ghost/turn.rs do_ghost_turn 775`, down from 896.
Its decomposition under 150 lines is a later phase.

**What differs from a move.** All four were dry-run, and none changes
behaviour:

1. **The prompt assembly becomes a function.** The block from
   `let prompt_name = …` to the `let system = format!(…);` moves verbatim
   into `build_ghost_prompt(session_id, sessions, config, tmux_session)`,
   which returns `GhostPrompt { system, max_ghost_turns, steps }`. Those
   are the three values the loop reads from that block; the others
   (`approved_scripts`, `run_with_sudo`, `ssh_target`, …) are used only
   inside the `format!`. `do_ghost_turn` destructures the struct where
   the block was.
2. **`do_ghost_turn` becomes `pub(super)`**, and `trigger_ghost_turn`
   calls it as `turn::do_ghost_turn(…)`. Nothing outside `ghost.rs`
   calls it.
3. **`#[allow(clippy::too_many_arguments)]` moves with the function.**
   It is the attribute the function already carries in `ghost.rs` (nine
   parameters), moved verbatim, not a new one. Reviewers grepping the
   new files for `#[allow` will find it; that is expected.
4. **`ghost.rs` gains `mod prompt;` and `mod turn;`** after its `use`
   block. Both child modules start with `use super::*;` and reach the
   parent's private helpers and imports that way, as the `dispatch_*`
   modules do.

The test module stays in `ghost.rs`. It references none of the moved
code.

**Headroom is thin.** `ghost.rs` and `turn.rs` are 782 lines each under
an 800 cap, and `build_ghost_prompt` is 138 under 150. The script's
counts are exact; do not add lines to either file.

## Architecture references

- `docs/dev/milestones/M31-code-hygiene/README.md`: decisions 3, 5, 6, 7.
- `docs/dev/milestones/M31-code-hygiene/phase-29-foreground-split.md`: the
  split-and-move pattern this phase follows.

## Spec

1. **Run the script below as written, once.** It does the whole change:
   - `src/daemon/ghost/prompt.rs`, `src/daemon/ghost/turn.rs` and the two
     `mod` lines;
   - the call-site rename in `trigger_ghost_turn`;
   - the allowlist (one line deleted, one re-pathed) and the two
     `CLAUDE.md` rows;
   - then `cargo fmt --all`, twice.

   It was dry-run on a scratch worktree of `26d99c07`: clippy was clean, the
   full suite passed 2587, and the End-to-end block printed every expected
   line. Do not hand-edit anything it touches. Do not dry-run it again in
   a worktree of your own. Do not `sed -i` a source file. It is a Python
   heredoc inside a shell script, so copy it **without adding
   indentation**. The script asserts its anchors and prints
   `do_ghost_turn moved; prompt assembly is build_ghost_prompt`. If an
   assert fires, stop and report it in the Update Log.
2. **Log it at once.** Append a `### Update — <date> (cuts done)` entry
   with the patch or write tool, quoting the script's output. The stall
   governor counts only edit-tool calls as progress (rexyMCP issue #14).
3. **Findings** (decision 7) go in **their own
   `### Findings — <date>` entry, within five tool calls.** Run exactly
   this command, and open no other file:

   `grep -nE '\.(unwrap|expect)\(|panic!|let _ =|\.ok\(\)|unwrap_or_default|unwrap_or_else' src/daemon/ghost/turn.rs src/daemon/ghost/prompt.rs`

   Write each hit with its function, kind and severity, or write `none`.
   Group hits of one kind on one line where they repeat. Do not look up
   callees: a finding names what these files do.
4. **Run the End-to-end block, then paste its output into the Update Log
   with an edit tool**, as a `### Update — <date> (end-to-end
   verification)` entry holding `/tmp/e2e-m31-32.txt` in a fenced block.
   **Paste only output you have already seen.**

   **Known flake:** `read_shell_screen_lines_tails_the_visible_rows`
   (`FINDINGS.md` #65). If the block shows `passed=2586 failed=1`, run it
   once more and paste both outputs. Any other failure is real.

   **Before you commit, all four of these must print 1:**
   - on the output file, `grep -c '^segments_present=1$'`,
     `grep -c '^routed=1$'` and `grep -c '^ghost_file_entries=0$'`;
   - on this doc, `grep -c '^### Findings — '`.
5. **Commit** as `refactor:`.

### The script

```bash
python3 - <<'PY'
import os
F = 'src/daemon/ghost.rs'
src = open(F).read().split('\n')


def find(text, start=0, prefix=False):
    hits = [i for i in range(start, len(src))
            if (src[i].startswith(text) if prefix else src[i] == text)]
    assert hits, text
    return hits[0]


s = find('async fn do_ghost_turn(')
a = s - 1
assert src[a] == '#[allow(clippy::too_many_arguments)]' and src[a - 1] == ''
e = next(i for i in range(s, len(src)) if src[i] == '}')
assert src[e + 1] == '' and src[e + 2] == '#[cfg(test)]', 'do_ghost_turn is not the last production item'
p0 = find('    let prompt_name = config.ai.prompt.clone();', s)
p1 = find('    let system = format!(', p0)
p2 = find('    );', p1)
assert src[p2 + 1] == '' and src[p2 + 2].startswith('    // Ghost shells never have a real IPC client')
assert src[p0 - 1] == ''
call = find('    let result = do_ghost_turn(')

head = src[a:p0]
head[1] = 'pub(super) async fn do_ghost_turn('
prompt_block = src[p0:p2 + 1]
rest = src[p2 + 1:e + 1]

os.makedirs('src/daemon/ghost', exist_ok=True)
open('src/daemon/ghost/prompt.rs', 'w').write('\n'.join([
    "//! A ghost run's system prompt: the runbook's policy lines, its seeded",
    "//! steps and the static rules, assembled once per run by `do_ghost_turn`.",
    '//! A child module of `src/daemon/ghost.rs`.',
    '',
    'use super::*;',
    '',
    '/// What `build_ghost_prompt` hands back: the system prompt and the two',
    '/// values the turn loop reads from the same runbook config.',
    'pub(super) struct GhostPrompt {',
    '    pub system: String,',
    '    pub max_ghost_turns: usize,',
    '    pub steps: Vec<String>,',
    '}',
    '',
    'pub(super) fn build_ghost_prompt(',
    '    session_id: &str,',
    '    sessions: &SessionStore,',
    '    config: &Config,',
    '    tmux_session: &str,',
    ') -> GhostPrompt {',
    *prompt_block,
    '    GhostPrompt {',
    '        system,',
    '        max_ghost_turns,',
    '        steps,',
    '    }',
    '}',
    '',
]))
open('src/daemon/ghost/turn.rs', 'w').write('\n'.join([
    "//! `do_ghost_turn`, the ghost run's turn loop: the model call, the tool",
    '//! calls, the wrap-up turn and the completion bookkeeping. A child module',
    '//! of `src/daemon/ghost.rs`.',
    '',
    'use super::*;',
    '',
    *head,
    '    let prompt::GhostPrompt {',
    '        system,',
    '        max_ghost_turns,',
    '        steps,',
    '    } = prompt::build_ghost_prompt(session_id, sessions, config, &tmux_session);',
    *rest,
    '',
]))
src[call] = '    let result = turn::do_ghost_turn('
out = src[:a] + src[e + 2:]
t = '\n'.join(out)
anchor = 'use crate::tmux::cache::SessionCache;\n'
assert t.count(anchor) == 1
t = t.replace(anchor, anchor + '\nmod prompt;\nmod turn;\n')
open(F, 'w').write(t)
print('do_ghost_turn moved; prompt assembly is build_ghost_prompt')
A = 'tests/hygiene_allowlist.txt'
al = open(A).read()
old = 'file src/daemon/ghost.rs 1677\n'
assert al.count(old) == 1
al = al.replace(old, '')
old = 'fn src/daemon/ghost.rs do_ghost_turn 896\n'
assert al.count(old) == 1
al = al.replace(old, 'fn src/daemon/ghost/turn.rs do_ghost_turn 775\n')
open(A, 'w').write(al)
C = 'CLAUDE.md'
cm = open(C).read()
row = '| `src/daemon/ghost.rs` |'
i = cm.index(row)
j = cm.index('\n', i)
cm = (cm[:j + 1]
      + "| `src/daemon/ghost/turn.rs` | `do_ghost_turn`, the ghost run's turn loop: the model call, the tool calls, the wrap-up turn and the completion bookkeeping; a child module of `src/daemon/ghost.rs` |\n"
      + "| `src/daemon/ghost/prompt.rs` | `build_ghost_prompt` — a ghost run's system prompt (the runbook's policy lines, its seeded steps, the static rules), returned as `GhostPrompt` with the `max_ghost_turns` and `steps` the loop reads; a child module of `src/daemon/ghost.rs` |\n"
      + cm[j + 1:])
open(C, 'w').write(cm)
PY
cargo fmt --all
cargo fmt --all   # twice, as in the earlier moves
wc -l src/daemon/ghost.rs src/daemon/ghost/turn.rs src/daemon/ghost/prompt.rs
```

## Acceptance criteria

- [ ] `src/daemon/ghost/turn.rs` holds `do_ghost_turn` and
      `src/daemon/ghost/prompt.rs` holds `build_ghost_prompt` and
      `GhostPrompt`; `ghost.rs` declares both as child modules.
- [ ] `tests/hygiene_allowlist.txt` has no `file src/daemon/ghost.rs`
      entry and reads `fn src/daemon/ghost/turn.rs do_ghost_turn 775`,
      and `cargo test --test hygiene` passes.
- [ ] The End-to-end block prints `ghost_file_entries=0`,
      `segments_present=1`, `routed=1` and `passed=2587 failed=0`, on
      the first run or on the one re-run the flake note allows.
- [ ] The Update Log has a `(cuts done)` entry, its own
      `### Findings — <date>` entry and an `(end-to-end verification)`
      entry, each written with an edit tool.

## End-to-end verification

Paste the output byte-exact into the Update Log (Spec 4). The segment
check finds the old function's three segments in the new files,
comparing the text with whitespace removed: the signature and session
load (with the `pub(super)` rewrite allowed) in `turn.rs`, the prompt
assembly in `prompt.rs`, and the client, loop and completion in
`turn.rs`.

```sh
bash <<'E2E'
B=26d99c07; G=src/daemon/ghost.rs; T=src/daemon/ghost/turn.rs; P=src/daemon/ghost/prompt.rs
A=/tmp/e2e-m31-32.txt; : > "$A"
wc -l $G $T $P | sed 's/^ *//' >> "$A"
grep -cE '^file src/daemon/ghost.rs ' tests/hygiene_allowlist.txt | sed 's/^/ghost_file_entries=/' >> "$A"
grep -E '^fn src/daemon/ghost(/turn)?.rs do_ghost_turn ' tests/hygiene_allowlist.txt >> "$A"
python3 - "$B" <<'PY' >> "$A"
import re, subprocess, sys
old = subprocess.run(['git', 'show', sys.argv[1] + ':src/daemon/ghost.rs'],
                     capture_output=True, text=True, check=True).stdout.split('\n')
turn = open('src/daemon/ghost/turn.rs').read()
prompt = open('src/daemon/ghost/prompt.rs').read()
def at(t, a=0):
    return next(i for i in range(a, len(old)) if old[i] == t)
def norm(lines):
    s = '\n'.join(lines).replace('\nasync fn do_ghost_turn(', '\npub(super) async fn do_ghost_turn(')
    s = re.sub(r'\s+', '', s)
    s = s.replace(',)', ')').replace(',}', '}').replace(',]', ']')
    return re.sub(r'\|(\w+)\|\{([^{};]*)\}', r'|\1|\2', s)
s = at('async fn do_ghost_turn(')
a = s - 1
e = next(i for i in range(s, len(old)) if old[i] == '}')
p0 = at('    let prompt_name = config.ai.prompt.clone();', s)
p1 = at('    let system = format!(', p0)
p2 = at('    );', p1)
segs = {'signature_and_session_load': (old[a:p0], turn), 'prompt_assembly': (old[p0:p2 + 1], prompt),
        'client_loop_and_completion': (old[p2 + 1:e + 1], turn)}
missing = [k for k, (v, dst) in segs.items() if norm(v) not in norm([dst])]
for k in missing:
    print('missing segment:', k)
print('segments_present=%d' % (not missing))
PY
grep -c 'turn::do_ghost_turn(' $G | sed 's/^/routed=/' >> "$A"
cargo test --test hygiene 2>&1 | grep -E '^test result' | sed 's/finished in [0-9.]*s/finished in <t>s/' >> "$A"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1 | sed 's/in [0-9.]*s/in <t>s/' >> "$A"
cargo fmt --all --check; echo "fmt_exit=$?" >> "$A"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}' >> "$A"
cat "$A"
E2E
```

Expected (from the dry run):
- `1419 …/ghost.rs`, `782 …/turn.rs` and `152 …/prompt.rs`;
- `ghost_file_entries=0` and `fn src/daemon/ghost/turn.rs do_ghost_turn 775`;
- `segments_present=1` and `routed=1`;
- hygiene `ok.`, clippy clean and `fmt_exit=0`;
- `passed=2587 failed=0`.

## Authorizations

- Edit `src/daemon/ghost.rs`; create `src/daemon/ghost/turn.rs` and
  `src/daemon/ghost/prompt.rs`.
- Edit `tests/hygiene_allowlist.txt` and `CLAUDE.md`, through the script
  only.

## Out of scope

- Splitting `do_ghost_turn` (775 lines) and `trigger_ghost_turn` (172)
  under the function cap: later phases.
- `stream.rs`: the next phase.
- Every finding (decision 6).

## Update Log

<!-- entries appended below this line -->

### Update — 2026-10-28 21:40 (started)

**Executor:** executor (headless run).
Flipped the phase status to `in-progress` and the README row to match. Now
running the Spec 1 script verbatim, once, then the Spec 2/3/4 log entries.

### Update — 2026-10-28 21:42 (cuts done)

Ran the Spec 1 script as written, once. Its full output:

```
do_ghost_turn moved; prompt assembly is build_ghost_prompt
 1419 src/daemon/ghost.rs
  782 src/daemon/ghost/turn.rs
  152 src/daemon/ghost/prompt.rs
 2353 total
```

All asserts passed; `cargo fmt --all` ran twice; the line counts match the
dry run exactly (1419 / 782 / 152). No hand edits to anything the script
touched.

### Findings — 2026-10-28

Spec 3 grep over `src/daemon/ghost/turn.rs` and `src/daemon/ghost/prompt.rs`
(4 hits; none new — all moved verbatim from the old `do_ghost_turn` /
prompt-assembly block in `ghost.rs`):

- `turn.rs:717` — in `do_ghost_turn`'s wrap-up bookkeeping, inside the
  closure that collects the assistant transcript from the session store:
  `unwrap_or_default()` on an `Option<Vec<String>>` — degrades to an empty
  transcript, which `uncovered_steps` then treats as "nothing covered".
  Kind: silent-default. Severity: low — it is the documented "ghost shells
  never have a real IPC client" path; a missing session degrades to the
  empty transcript rather than failing the wrap-up.
- `prompt.rs:50` — in `build_ghost_prompt`, `unwrap_or_else(|| ("none".to_string(), false, daemon_ceiling, None, false))`
  on the ghost-config lookup: a ghost session with no `ghost_config` falls
  back to defaults (`none` agent name, no sudo, the daemon turn ceiling, no
  SSH target, no auto-approve). Kind: silent-default. Severity: low —
  pre-existing behaviour carried verbatim; the fallback is the same shape
  the config defaults produce.
- `prompt.rs:82` — in `build_ghost_prompt`, `unwrap_or_default()` on the
  seeded-steps lookup: a session with no stored steps yields an empty list,
  and the prompt then omits the steps block entirely. Kind:
  silent-default. Severity: low — an empty steps block is a valid prompt
  shape (`steps_block` handles it explicitly two lines below).

No `unwrap()`, `expect()`, `panic!()`, `let _ =`, or `.ok()` hits.

### Update — 2026-10-28 21:52 (end-to-end verification)

Ran the Spec 4 End-to-end block once. Its output, byte-exact from
`/tmp/e2e-m31-32.txt`:

```
1419 src/daemon/ghost.rs
782 src/daemon/ghost/turn.rs
152 src/daemon/ghost/prompt.rs
2353 total
ghost_file_entries=0
fn src/daemon/ghost/turn.rs do_ghost_turn 775
segments_present=1
routed=1
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in <t>s
    Finished `dev` profile [unoptimized + debuginfo] target(s) in <t>s
fmt_exit=0
passed=2587 failed=0
```

No flake: `passed=2587 failed=0` on the first run. Every expected line from
the dry run is present: 1419/782/152, `ghost_file_entries=0`, the re-pathed
`fn src/daemon/ghost/turn.rs do_ghost_turn 775` allowlist entry,
`segments_present=1` (all three segments found, whitespace-normalised,
against `26d99c07`), `routed=1`, hygiene `ok.`, clippy clean, `fmt_exit=0`
and `passed=2587 failed=0`.

### Update — 2026-10-28 21:42 (complete, server-authored)

**Summary:** **Summary:** Phase 32 executed per spec: the Spec 1 script ran once as written, moved `do_ghost_turn` (with its existing `#[allow(clippy::too_many_arguments)]`) verbatim into `src/daemon/ghost/turn.rs` with `pub(super)` visibility, turned the 126-line prompt assembly into `build_ghost_prompt` returning `GhostPrompt { system, max_ghost_turns, steps }` in `src/daemon/ghost/prompt.rs`, added `mod prompt;` / `mod turn;` to `ghost.rs`, rerouted `trigger_ghost_turn` through `turn::do_ghost_turn`, deleted the `file src/daemon/ghost.rs 1677` allowlist line, re-pathed the function entry to `fn src/daemon/ghost/turn.rs do_ghost_turn 775`, and added both CLAUDE.md rows. All script asserts passed on the first run with zero hand edits to anything it touched. End-to-end verification (one run, no flake): `1419/782/152` line counts matching the dry run, `ghost_file_entries=0`, `segments_present=1`, `routed=1`, hygiene `ok. 16 passed`, clippy clean, `fmt_exit=0`, `passed=2587 failed=0` — all four pre-commit grep checks print 1, and the Update Log carries the `(cuts done)`, `### Findings — 2026-10-28`, and `(end-to-end verification)` entries plus the `(started)` entry. Committed as `b117da80` (`refactor:`), tree clean.

**Notes for review:** Findings (Spec 3, decision 7) are three silent-default `unwrap_or_default`/`unwrap_or_else` sites — `turn.rs:717` (empty transcript when the session lookup misses during wrap-up), `prompt.rs:50` (ghost-config fallback to defaults), `prompt.rs:82` (empty seeded-steps yields an omitted steps block) — all pre-existing behaviour carried verbatim out of the old block, none new, all rated low. Adjacent, out of scope: `do_ghost_turn` is now 775 lines and `trigger_ghost_turn` 172, both still over the function cap, deferred to later phases per the phase doc; `stream.rs` is the next phase. No deviations from the spec — no dependencies, no `unsafe`, no `#[allow]` added (the one present moved with the function, as authorized).

**Executor:** nvidia/GLM-5.3-Flash-FP4-Local

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.18s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.16s


TEST
ne_test_module ... ok
test duplicate_names_get_a_suffix ... ok
test production_after_a_test_only_fn_is_still_counted ... ok
test functions_in_a_test_module_are_not_measured ... ok
test impl_and_trait_methods_are_measured_with_their_type ... ok
test exempt_files_exist ... ok
test allowlist_has_no_stale_entries ... ok
test functions_fit_the_cap ... ok
test test_files_fit_the_cap ... ok
test production_files_fit_the_cap ... ok

test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.27s


running 34 tests
test daemon_ping_status_loop ... ignored
test cancel_request_roundtrip ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g4_briefing_injection_block_format ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g5_child_inherits_depth_and_parent ... ok
test g5_depth_limit_enforced ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ipc_ask_round_trip ... ok
test ipc_session_info_round_trip ... ok
test ipc_tool_call_response_round_trip ... ok
test ghost_config_parsing ... ok
test minimal_config_parsing ... ok
test resume_run_request_roundtrip ... ok
test schedule_store_persistence ... ok
test config_pricing_round_trip ... ok
test window_switch_does_not_corrupt_chat ... ignored
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test g4_briefing_masking_applied ... ok
test g4_briefing_injects_on_next_run ... ok
test event_log_entry_format ... ok
test event_log_append_read ... ok
test g4_briefing_read_and_clear ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test session_index_persistence ... ok
test session_jsonl_round_trip ... ok
test webhook_alert_no_severity_passes_gate ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_to_event_log ... ok
test webhook_alert_below_threshold_discarded ... ok
test webhook_alert_unrankable_severity_passes_gate ... ok

test result: ok. 32 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.09s


running 12 tests
test webhook_ghost_e2e_http ... ignored
test held_port_cannot_be_rebound ... ok
test webhook_ports_differ_between_environments ... ok
test stub_returns_canned_response_via_make_client ... ok
test webhook_ghost_e2e_deterministic ... ok
test webhook_ghost_records_its_ssh_target_resolution ... ok
test config_contains_webhook_and_stub_url ... ok
test an_empty_session_starts_no_tmux_server ... ok
test daemon_installs_no_tmux_hooks ... ok
test daemon_boots_in_throwaway_root ... ok
test default_server_unchanged ... ok
test daemon_webhook_returns_200 ... ok

test result: ok. 11 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.31s


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
kill: (2384427): No such process
kill: (-2385102): No such process
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

- `CLAUDE.md` — +2 -0
- `docs/dev/milestones/M31-code-hygiene/README.md` — +1 -1
- `docs/dev/milestones/M31-code-hygiene/phase-32-ghost-turn.md` — +77 -1
- `src/daemon/ghost/prompt.rs` — +152 -0
- `src/daemon/ghost/turn.rs` — +782 -0
- `src/daemon/ghost.rs` — +4 -899
- `tests/hygiene_allowlist.txt` — +1 -2

**Commit:** b117da8045e6c84934186c12aca53652a8f2a49c

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).
