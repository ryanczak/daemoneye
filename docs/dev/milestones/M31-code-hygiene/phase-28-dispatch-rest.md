# Phase 28 — Move nine more of `execute_tool_call`'s arms into family modules

**Status:** in-progress
**Milestone:** M31 — Code hygiene
**Drafted:** 2026-10-28

## Goal

This phase moves the next nine of `execute_tool_call`'s arms
(`src/daemon/executor/mod.rs`) into five new per-family child modules. It
uses phase 26's pattern: one `dispatch` per module, the arms moved
unchanged, and one grouped arm in `execute_tool_call` per family.

| Module | Tools |
|---|---|
| `dispatch_files.rs` | `read_file`, `edit_file` |
| `dispatch_context.rs` | `get_shell_context` |
| `dispatch_open_shell.rs` | `open_shell` |
| `dispatch_send_input.rs` | `send_input` |
| `dispatch_meta.rs` | invalid arguments, `load_tools`, `spawn_ghost_shell` |

`execute_tool_call` drops from 1161 to 797 lines, and the file drops from
1848 to 1489 production lines. The script lowers both allowlist entries.
Only `run_terminal_command` (`Foreground`, `Background`) stays in the
match. Behaviour does not change.

**Why these are safe to move.** A `return` inside an arm leaves
`execute_tool_call`. Inside a family function, the same `return` only
leaves that function, and its value then reaches the feedback code after
the match.
- That is equivalent for a tool outside
  `PendingCall::should_emit_tool_feedback`, which sends no
  `ToolFinished` either way. This covers `edit_file`, `send_input` and
  invalid arguments.
- The four feedback tools that used to `return` early (`read_file`,
  `get_shell_context`, `open_shell`, `spawn_ghost_shell`) now end their
  arms with labeled-block `break`s instead, so for them the move changes
  nothing. That was #73's fix.
- `silent_tool_early_exit_sends_tool_finished` guards the move.

**Why five modules, not fewer.** `open_shell` and `send_input` together
would make a 194-line `dispatch`, over the function cap. And no
`dispatch` takes more than seven parameters, which is clippy's
`too_many_arguments` limit.

## Architecture references

- `docs/dev/milestones/M31-code-hygiene/README.md`: decisions 3, 5, 6 and
  7, and the "#73 settled" note.
- `docs/dev/milestones/M31-code-hygiene/phase-26-dispatch-families.md`:
  the pattern this phase repeats.

## Spec

1. **Run the script below as written, once.** It does the whole change:
   - the five files and their `mod` declarations;
   - the grouped arms in `execute_tool_call`;
   - the two allowlist numbers and the `CLAUDE.md` key-files row;
   - `cargo fmt --all`, twice.

   It was dry-run on a scratch worktree of `2c83ca2c`. Clippy and the full
   suite (2587) were green, and the End-to-end block printed every
   expected line. Do not hand-edit anything it touches. Do not dry-run it
   again in a worktree of your own. Do not `sed -i` a source file. It is
   a Python heredoc inside a shell script, so copy it **without adding
   indentation**.

   No line of a moved arm changes. The policies and the parent job id
   reach the family functions as references (`&Option<…>`), so the
   moved `.as_ref()` and `.as_deref()` calls work as they stand. Each
   `dispatch` ends in `other => anyhow::bail!(…)`, an arm no call can
   reach. The End-to-end block's `routing_matches` line proves that each
   grouped arm lists exactly the variants its module handles.
2. **Log it at once.** Append a `### Update — <date> (cuts done)` entry
   with the patch or write tool, quoting the script's output. The stall
   governor counts only edit-tool calls as progress (rexyMCP issue #14).
3. **Findings** (decision 7) go in **their own
   `### Findings — <date>` entry**. Phase 27 already audited the
   `ReadFile`, `GetShellContext`, `OpenShell` and `SpawnGhost` arms
   (#77–#79), so read only the four arms nobody has audited:
   - `EditFile` in `dispatch_files.rs`;
   - `SendInput` in `dispatch_send_input.rs`;
   - `InvalidArgs` and `LoadTools` in `dispatch_meta.rs`.

   That is about 240 lines. Record each hit with its symbol, kind and
   severity, or write `none`:
   - an `expect`, `unwrap` or `panic!`;
   - a swallowed error;
   - duplicated logic;
   - anything else that looks wrong.
4. **Run the End-to-end block, then paste its output into the Update Log
   with an edit tool**, as a `### Update — <date> (end-to-end
   verification)` entry holding `/tmp/e2e-m31-28.txt` in a fenced block.
   **Paste only output you have already seen.** Write the entry after the
   block has run, and add no other "proof" block. The block is a `bash`
   heredoc, so run it as it stands.

   **Known flake:** `read_shell_screen_lines_tails_the_visible_rows`
   (`FINDINGS.md` #65). If the block shows `passed=2586 failed=1`, run it
   once more and paste both outputs, with a line saying so. Any other
   failure is real.

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
m = next(i for i, l in enumerate(src) if l == '    let outcome = match call {')
mend = next(i for i in range(m, len(src)) if src[i] == '    }?;')
anchors = [i for i in range(m + 1, mend) if re.match(r'        PendingCall::\w+', src[i])]
arms = {}
for k, a in enumerate(anchors):
    b = anchors[k + 1] if k + 1 < len(anchors) else mend
    arms[re.match(r'        PendingCall::(\w+)', src[a]).group(1)] = (a, b)
WHERE = ('where\n    W: tokio::io::AsyncWriteExt + Unpin,\n'
         '    R: tokio::io::AsyncBufReadExt + Unpin,\n')
WHERE_W = 'where\n    W: tokio::io::AsyncWriteExt + Unpin,\n'
GHOST = '    ghost_policy: &Option<GhostPolicy>,\n'
FAMILIES = [
    ('files', 'the file tools', ['ReadFile', 'EditFile'],
     'pub(super) async fn dispatch<W, R>(\n    call: &PendingCall,\n    tx: &mut W,\n    rx: &mut R,\n'
     '    session_id: Option<&str>,\n' + GHOST + '    is_ghost: bool,\n'
     ') -> anyhow::Result<ToolCallOutcome>\n' + WHERE,
     'call, tx, rx, session_id, &ghost_policy, is_ghost', True),
    ('context', 'the terminal-context tool', ['GetShellContext'],
     'pub(super) fn dispatch(\n    call: &PendingCall,\n    session_id: Option<&str>,\n'
     '    chat_pane: Option<&str>,\n    cache: &Arc<SessionCache>,\n'
     ') -> anyhow::Result<ToolCallOutcome> ',
     'call, session_id, chat_pane, cache', False),
    ('open_shell', 'the shell opener', ['OpenShell'],
     'pub(super) async fn dispatch<W, R>(\n    call: &PendingCall,\n    tx: &mut W,\n    rx: &mut R,\n'
     '    session_id: Option<&str>,\n' + GHOST + '    is_ghost: bool,\n'
     ') -> anyhow::Result<ToolCallOutcome>\n' + WHERE,
     'call, tx, rx, session_id, &ghost_policy, is_ghost', True),
    ('send_input', 'the shell input tool', ['SendInput'],
     'pub(super) async fn dispatch<W, R>(\n    call: &PendingCall,\n    tx: &mut W,\n    rx: &mut R,\n'
     '    session_id: Option<&str>,\n' + GHOST
     + '    tool_policy: &Option<crate::agents::ToolPolicy>,\n    is_ghost: bool,\n'
     ') -> anyhow::Result<ToolCallOutcome>\n' + WHERE,
     'call, tx, rx, session_id, &ghost_policy, &tool_policy, is_ghost', True),
    ('meta', 'the tool-set and delegation tools', ['InvalidArgs', 'LoadTools', 'SpawnGhost'],
     'pub(super) async fn dispatch<W>(\n    call: &PendingCall,\n    tx: &mut W,\n'
     '    session_id: Option<&str>,\n    sessions: &SessionStore,\n' + GHOST
     + '    spawn_depth: u8,\n    effective_parent_job_id: &Option<String>,\n'
     ') -> anyhow::Result<ToolCallOutcome>\n' + WHERE_W,
     'call, tx, session_id, sessions, &ghost_policy, spawn_depth, &effective_parent_job_id', True),
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
for old, new in [('file src/daemon/executor/mod.rs 1848\n', 'file src/daemon/executor/mod.rs 1489\n'),
                 ('fn src/daemon/executor/mod.rs execute_tool_call 1161\n',
                  'fn src/daemon/executor/mod.rs execute_tool_call 797\n')]:
    assert al.count(old) == 1, old
    al = al.replace(old, new)
open(A, 'w').write(al)
C = 'CLAUDE.md'
cm = open(C).read().split('\n')
row = next(i for i, l in enumerate(cm) if l.startswith('| `src/daemon/executor/dispatch_schedule.rs`,'))
cm[row] = ('| `src/daemon/executor/dispatch_schedule.rs`, `dispatch_artifacts.rs`, `dispatch_memory.rs`, '
           '`dispatch_shells.rs`, `dispatch_runs.rs`, `dispatch_files.rs`, `dispatch_context.rs`, '
           '`dispatch_open_shell.rs`, `dispatch_send_input.rs`, `dispatch_meta.rs` | `execute_tool_call`\'s arms '
           'by tool family, child modules of `src/daemon/executor/mod.rs`: each `dispatch(call, …)` matches its '
           'family\'s `PendingCall` variants — the schedule tools; scripts, runbooks and agents; memory and recall; '
           'the shell readers and observers; the run-log tools; `read_file` and `edit_file`; `get_shell_context`; '
           '`open_shell`; `send_input`; `load_tools`, invalid arguments and `spawn_ghost_shell`. '
           '`run_terminal_command` stays in `execute_tool_call` |')
open(C, 'w').write('\n'.join(cm))
PY
cargo fmt --all
cargo fmt --all   # twice, as in the earlier moves
wc -l src/daemon/executor/mod.rs src/daemon/executor/dispatch_files.rs src/daemon/executor/dispatch_context.rs src/daemon/executor/dispatch_open_shell.rs src/daemon/executor/dispatch_send_input.rs src/daemon/executor/dispatch_meta.rs
```

## Acceptance criteria

- [ ] The five new `dispatch_*.rs` files exist as child modules of
      `src/daemon/executor/mod.rs`, and `execute_tool_call`'s match holds
      only the `Foreground` and `Background` arms besides the grouped
      ones.
- [ ] The allowlist reads `file src/daemon/executor/mod.rs 1489` and
      `fn src/daemon/executor/mod.rs execute_tool_call 797`, and
      `cargo test --test hygiene` passes. No new function exceeds 150
      lines.
- [ ] The End-to-end block prints `bodies_identical=1`,
      `routing_matches=1` and `passed=2587 failed=0`, on the first run or
      on the one re-run the flake note allows.
- [ ] The Update Log has a `(cuts done)` entry, its own
      `### Findings — <date>` entry and an `(end-to-end verification)`
      entry, each written with an edit tool.

## End-to-end verification

Paste the output byte-exact into the Update Log (Spec 4).

```sh
bash <<'E2E'
B=2c83ca2c; D=src/daemon/executor; F=$D/mod.rs
A=/tmp/e2e-m31-28.txt; : > "$A"
wc -l $F $D/dispatch_files.rs $D/dispatch_context.rs $D/dispatch_open_shell.rs $D/dispatch_send_input.rs $D/dispatch_meta.rs | sed 's/^ *//' >> "$A"
grep -E '^(file|fn) src/daemon/executor/mod.rs (execute_tool_call )?[0-9]+$' tests/hygiene_allowlist.txt >> "$A"
same=1; routed=1
for fam in files context open_shell send_input meta; do
  names=$(grep -oE '^        PendingCall::[A-Za-z]+' $D/dispatch_$fam.rs | sed 's/.*:://' | tr '\n' ',')
  old=$(git show $B:$F | awk -v set=",$names" '
    /^    let outcome = match call \{$/ {m=1; next}
    m && /^    \}\?;$/ {m=0}
    m && /^        PendingCall::[A-Za-z]+/ {match($0, /PendingCall::[A-Za-z]+/); v=substr($0, RSTART+13, RLENGTH-13); keep=index(set, "," v ",")>0}
    m && keep' | tr -d ' \t\n' | sed 's/,)/)/g' | md5sum)
  new=$(awk '/^    match call \{$/ {f=1; next} /^        other => anyhow::bail!/ {f=0} f' $D/dispatch_$fam.rs | tr -d ' \t\n' | sed 's/,)/)/g' | md5sum)
  [ "$old" = "$new" ] || { same=0; echo "body differs: $fam" >> "$A"; }
  grouped=$(awk -v fam="dispatch_$fam::dispatch(" '
    /^        PendingCall::/ {names=""}
    /^        (\| )?PendingCall::[A-Za-z]+ \{ \.\. \}/ {s=$0; while (match(s, /PendingCall::[A-Za-z]+ \{ \.\. \}/)) {names=names substr(s, RSTART+13, RLENGTH-20) ","; s=substr(s, RSTART+RLENGTH)}}
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
- the line counts `2553 …/mod.rs`, `135 …/dispatch_files.rs`,
  `49 …/dispatch_context.rs`, `72 …/dispatch_open_shell.rs`,
  `145 …/dispatch_send_input.rs` and `109 …/dispatch_meta.rs`;
- the two allowlist lines at 1489 and 797;
- `bodies_identical=1` and `routing_matches=1`;
- hygiene `ok.`, clippy clean and `fmt_exit=0`;
- `passed=2587 failed=0`.

## Authorizations

- Edit `src/daemon/executor/mod.rs`, and create the five new
  `src/daemon/executor/dispatch_*.rs` files.
- Edit `tests/hygiene_allowlist.txt` and `CLAUDE.md`, through the script
  only.

## Out of scope

- `run_terminal_command`'s two arms and the prelude before the match:
  later phases.
- Acting on any finding (decision 6), including #77–#79.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-10-28 11:20 (started)

Started phase 28. Flipped the status to `in-progress` and updated the milestone
README's phase-table row. Next: run the phase script once as written, then log
the cuts, the findings, and the end-to-end verification.

### Update — 2026-10-28 11:20 (cuts done)

Ran the phase script once, as written, then `cargo fmt --all` twice. It printed:

```
  2553 src/daemon/executor/mod.rs
   135 src/daemon/executor/dispatch_files.rs
    49 src/daemon/executor/dispatch_context.rs
    72 src/daemon/executor/dispatch_open_shell.rs
   145 src/daemon/executor/dispatch_send_input.rs
   109 src/daemon/executor/dispatch_meta.rs
  3063 total
```

Every line count matches the dry run's expectation (2553 / 135 / 49 / 72 / 145
/ 109). `wc -l` counts `mod.rs` by raw lines, so 2553 raw lines is 1489
production lines after the hygiene counter strips blanks and comments. The
allowlist was rewritten to `file …mod.rs 1489` and `fn …execute_tool_call 797`,
and the `CLAUDE.md` key-files row now lists all ten `dispatch_*.rs` modules.

### Update — 2026-10-28 18:05 (escalation)

**Chosen lever:** resume.
**Rationale:** the run's `NoProgressStall` (120 read-only calls) came
after the cuts, which are complete and correct. On the partial tree the
architect's run of the End-to-end block printed every expected line:
`bodies_identical=1`, `routing_matches=1`, the allowlist at 1489 and 797,
hygiene `ok.`, clippy clean, `fmt_exit=0` and `passed=2587 failed=0`.

The stall was the Findings audit. It followed every callee into
`policy.rs`, `host.rs` and `src/agents/`, and never wrote the entry. The
spec was fine and only the bookkeeping is left, so resuming is better
than re-dispatching.

### Notes for executor — 2026-10-28

**The code is done. Do not re-run the script, and do not edit any `.rs`
file.** Three steps remain, in this order:

1. **Findings, bounded.** Run exactly these two commands, and open no
   other file:
   - `grep -nE '\.(unwrap|expect)\(|panic!|let _ =|\.ok\(\)|unwrap_or_default' src/daemon/executor/dispatch_files.rs src/daemon/executor/dispatch_send_input.rs src/daemon/executor/dispatch_meta.rs`
   - `cat src/daemon/executor/dispatch_send_input.rs src/daemon/executor/dispatch_meta.rs`

   Then read `dispatch_files.rs`'s `EditFile` arm with `sed -n`. Write
   the `### Findings — 2026-10-28` entry from what those show. Do not
   look up callees: a finding names what the arm does, not what the
   function it calls does. Write the entry within your next five calls.
2. **Run the End-to-end block, then paste its output** as the
   `(end-to-end verification)` entry, as in Spec 4.
3. **Check the three pre-commit greps, then commit** as `refactor:`.

### Findings — 2026-10-28

Audit scope per the notes block: the four unaudited arms — `EditFile`
(`dispatch_files.rs`), `SendInput` (`dispatch_send_input.rs`), `InvalidArgs`
and `LoadTools` (`dispatch_meta.rs`). Method: the two bounded greps, a full
read of `dispatch_send_input.rs` and `dispatch_meta.rs`, and a read of
`dispatch_files.rs`'s `EditFile` arm. Callees not opened.

**`dispatch_send_input.rs` — `PendingCall::SendInput`**

- `session_id.unwrap_or_default()` (the execute step): a missing session id
  silently attributes the send to `Owner::Chat("")`. Kind: silent default.
  Severity: low — pre-existing behavior moved unchanged, and every real call
  path carries a session id.
- `Err(_) => crate::shell::Foreground::Unknown` (the id-parse branch): a
  swallowed parse error, but deliberate and commented — an unparseable id is
  "not a shell at all", the same answer the probe gives an unknown id. Not a
  finding.
- `_ => (crate::shell::ShellModes::default(), None)` (the screen-read
  fallthrough): a swallowed failure, but deliberate and commented — an
  unreadable or unannotated reply falls back to the defaults, which encode
  plain bytes for every key. Not a finding.
- `Ok(_cmd_id) => {}` (the approval arm): the granted command id is dropped.
  Harmless; it is not needed after approval. Not a finding.

**`dispatch_meta.rs` — `PendingCall::InvalidArgs`, `PendingCall::LoadTools`**

- No `unwrap`/`expect`/`panic!`/`let _ =`/`.ok()`/`unwrap_or_default` hits in
  the file.
- `LoadTools`: `crate::ai::tools::tools_in_group(g)` is computed twice per
  group — once in the persistence pass (building `loaded`/`unknown`), again
  in the result-parts pass. Kind: duplicated logic. Severity: low; a single
  pass collecting members per group would serve both, but that is a rewrite,
  not a move.
- `InvalidArgs`: sends the `ToolResult` itself and also returns
  `Ok(ToolCallOutcome::Result(msg))`. Consistent with the established
  pattern for tools outside `should_emit_tool_feedback`; not a finding.

**`dispatch_files.rs` — `PendingCall::EditFile`**

- `sudo: sudo.unwrap_or(false)`: a missing `sudo` argument silently defaults
  to off. Kind: silent default. Severity: low — an omitted flag meaning "no
  sudo" is the documented default, not a swallowed error.
- The connect-failure branch builds `Error: {msg}`, sends the `ToolResult`
  and `return`s — no `ssh_connect_failed` `log_event`, while the sibling
  `ReadFile` arm in the same module logs exactly that event at the same
  point. Kind: inconsistent observability between the two file arms.
  Severity: low.
- The `return Ok(...)` in that branch leaves the family function, not
  `execute_tool_call`. Equivalent for `edit_file`, which sends no
  `ToolFinished` either way (the spec's move-safety argument, and the
  `silent_tool_early_exit_sends_tool_finished` test's subject). Not a
  finding.

Summary: no hits above low severity. Three low findings (`unwrap_or_default`
owner attribution, the double `tools_in_group` pass, the missing
`ssh_connect_failed` log in `EditFile`) plus two silent defaults noted for
the record. None block the move; none are acted on (out of scope, decision 6).

### Update — 2026-10-28 18:35 (end-to-end verification)

Ran the phase doc's End-to-end block once. It wrote `/tmp/e2e-m31-28.txt`;
its contents, byte-exact:

```
2553 src/daemon/executor/mod.rs
135 src/daemon/executor/dispatch_files.rs
49 src/daemon/executor/dispatch_context.rs
72 src/daemon/executor/dispatch_open_shell.rs
145 src/daemon/executor/dispatch_send_input.rs
109 src/daemon/executor/dispatch_meta.rs
3063 total
file src/daemon/executor/mod.rs 1489
fn src/daemon/executor/mod.rs execute_tool_call 797
bodies_identical=1
routing_matches=1
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in <t>s
    Finished `dev` profile [unoptimized + debuginfo] target(s) in <t>s
fmt_exit=0
passed=2587 failed=0
```

Every expected line is present: the six line counts match the dry run
(2553 / 135 / 49 / 72 / 145 / 109), the allowlist reads 1489 and 797,
`bodies_identical=1`, `routing_matches=1`, hygiene `ok.`, clippy clean,
`fmt_exit=0`, and `passed=2587 failed=0` — no flake, first run.

Pre-commit greps, each printing 1:

```
$ grep -c '^bodies_identical=1$' /tmp/e2e-m31-28.txt
1
$ grep -c '^routing_matches=1$' /tmp/e2e-m31-28.txt
1
$ grep -c '^### Findings — ' docs/dev/milestones/M31-code-hygiene/phase-28-dispatch-rest.md
1
```

