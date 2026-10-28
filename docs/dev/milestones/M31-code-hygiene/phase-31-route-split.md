# Phase 31 — Split `execute_tool_call` into policy, snapshot and route

**Status:** in-progress
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
