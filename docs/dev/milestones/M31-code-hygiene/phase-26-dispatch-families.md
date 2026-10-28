# Phase 26 — Move `execute_tool_call`'s simple arms into per-family modules

**Status:** todo
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
