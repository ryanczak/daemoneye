# Phase 29 — Split `run_terminal_command`'s foreground arm into stages

**Status:** in-progress
**Milestone:** M31 — Code hygiene
**Drafted:** 2026-10-28

## Goal

The `PendingCall::Foreground` arm of `execute_tool_call`
(`src/daemon/executor/mod.rs`) is 392 lines, and the 150-line function cap
means it cannot move whole. It moves into a new child module,
`dispatch_foreground.rs`, split into four functions at the arm's own
seams:

| Function | What it holds | Lines |
|---|---|---|
| `dispatch` | the ghost gate, the host parse, the ghost `hosts:` boundary, the approval prompt, the refusal fallback | 145 |
| `run_approved` | the owner, the post-approval host lookup, the local run | 100 |
| `run_on_named_host` | the permissive lookup, the connect, the run over ssh | 105 |
| `finish_run` | the result text, the stats, the `ToolResult` frame, the shell log line | 67 |

`execute_tool_call` drops from 797 to 409 lines, and the file drops from
1489 to 1102 production lines. The script lowers both allowlist entries.

**This is a split, not only a move.** Four things differ from a move. All
were dry-run, and none changes behaviour:

1. **The duplicated result block becomes `finish_run`.** The local and
   named arms each ended in the same 58-line `match run { … }`; the
   script asserts the two copies are identical before cutting. Both
   hosts now call `finish_run`, and the End-to-end block's
   `result_blocks=1` line checks that only one copy is left.
2. **`return Ok(ToolCallOutcome::Result(msg));` becomes
   `return Ok(msg);`** inside `run_approved` and `run_on_named_host`.
   Those functions return the result text, and `dispatch` wraps it in the
   same `ToolCallOutcome::Result`. Nothing ran after those returns
   before the change, and nothing does after it.
   `run_terminal_command` is not a feedback tool, so no `ToolFinished`
   is involved.
3. **`ApprovedRun` carries the call's fields** (`id`, `cmd`, `shell`,
   `timeout_secs`, `interactive`, `cmd_id`, `session_id`) between the
   stages. Each stage destructures it at the top, so the moved lines read
   the same names. It is `Copy` (all borrows and a `usize`), and that
   keeps every stage within clippy's seven-parameter limit.
4. **`dispatch` takes the `SessionCtx`** and reads `session_id` from it
   in its first line, as `execute_tool_call` does. The arm passes `ctx`
   to `refuse_foreground`, and `SessionCtx` is `Copy`.

No test drives this path through `execute_tool_call`. The gate tests call
the helpers directly (`FINDINGS.md` #83). The guard is therefore the
compiler, the full suite, and the End-to-end block's segment check, which
finds each of the arm's six original segments verbatim in the new file.

## Architecture references

- `docs/dev/milestones/M31-code-hygiene/README.md`: decisions 3, 5, 6, 7.
- `docs/dev/milestones/M31-code-hygiene/phase-28-dispatch-rest.md`: the
  family-module pattern this phase extends.

## Spec

1. **Run the script below as written, once.** It does the whole change:
   - the new file and its `mod` line;
   - the one-line arm left in `execute_tool_call`;
   - the two allowlist numbers and the `CLAUDE.md` rows;
   - then `cargo fmt --all`, twice.

   It was dry-run on a scratch worktree of `266862b4`: clippy was clean, the
   full suite passed 2587, and the End-to-end block printed every expected
   line. Do not hand-edit anything it touches. Do not dry-run it again in
   a worktree of your own. Do not `sed -i` a source file. It is a Python
   heredoc inside a shell script, so copy it **without adding
   indentation**. The script asserts its anchors and that the two result
   blocks are identical; if an assert fires, stop and report it in the
   Update Log.
2. **Log it at once.** Append a `### Update — <date> (cuts done)` entry
   with the patch or write tool, quoting the script's output. The stall
   governor counts only edit-tool calls as progress (rexyMCP issue #14).
3. **Findings** (decision 7) go in **their own
   `### Findings — <date>` entry, within five tool calls.** Run exactly
   this command, and open no other file:

   `grep -nE '\.(unwrap|expect)\(|panic!|let _ =|\.ok\(\)|unwrap_or_default|unwrap_or_else' src/daemon/executor/dispatch_foreground.rs`

   Write each hit with its function, kind and severity, or write `none`.
   Do not look up callees: a finding names what this file does.
   `FINDINGS.md` #82 (the doubled `host_entry_or_bare` check) and #83
   (no test drives this path) are recorded already. Do not repeat them.
4. **Run the End-to-end block, then paste its output into the Update Log
   with an edit tool**, as a `### Update — <date> (end-to-end
   verification)` entry holding `/tmp/e2e-m31-29.txt` in a fenced block.
   **Paste only output you have already seen.**

   **Known flake:** `read_shell_screen_lines_tails_the_visible_rows`
   (`FINDINGS.md` #65). If the block shows `passed=2586 failed=1`, run it
   once more and paste both outputs. Any other failure is real.

   **Before you commit, all three of these must print 1:**
   `grep -c '^segments_present=1$'`, `grep -c '^result_blocks=1$'` and
   `grep -c '^### Findings — '` on this doc.
5. **Commit** as `refactor:`.

### The script

```bash
python3 - <<'PY'
F = 'src/daemon/executor/mod.rs'
N = 'src/daemon/executor/dispatch_foreground.rs'
src = open(F).read().split('\n')


def find(text, start=0, end=None):
    end = len(src) if end is None else end
    hits = [i for i in range(start, end) if src[i] == text]
    assert hits, text
    return hits[0]


a0 = find('        PendingCall::Foreground {')
a1 = find('        PendingCall::Background { id, cmd, host, .. } => {', a0)
ok = find('                    Ok(cmd_id) => {', a0, a1)
ret = find('                        return Ok(ToolCallOutcome::Result(outcome));', ok, a1)
assert src[ret + 1] == '                    }'
out_m = find('                        let outcome = match &host {', ok, ret)
loc = find('                            crate::host::HostId::Local => {', out_m, ret)
named = find('                            crate::host::HostId::Named(name) => {', out_m, ret)
run_l = find('                                match run {', loc, named)
run_n = find('                                match run {', named, ret)
end_n = find('                            }', run_n, ret)          # closes the Named arm
assert src[end_n + 1] == '                        };' and end_n + 2 == ret
assert src[named - 2] == '                                }' and src[named - 1] == '                            }'
assert src[run_n:end_n] == src[run_l:named - 1], 'the two result blocks differ'

EARLY = 'return Ok(ToolCallOutcome::Result(msg));'


def early(lines):
    return [l.replace(EARLY, 'return Ok(msg);') for l in lines]


arm_head = src[a0:ok + 1]                       # pattern, gates, approval, `Ok(cmd_id) => {`
arm_tail = src[ret + 1:a1]                      # `}` of Ok, the approval match, refusal
approved_head = early(src[ok + 1:loc + 1])      # owner .. `HostId::Local => {`
local_run = src[loc + 1:run_l]                  # the local `let run = …;`
finish_body = src[run_l:named - 1]              # the shared `match run { … }`
named_body = early(src[named + 1:run_n])        # the named host's checks and `let run = …;`

W_R = ('where\n    W: tokio::io::AsyncWriteExt + Unpin,\n'
       '    R: tokio::io::AsyncBufReadExt + Unpin,\n')
text = '\n'.join([
    "//! `execute_tool_call`'s `run_terminal_command` foreground arm",
    '//! (`PendingCall::Foreground`), split into its stages: the gates and the',
    '//! approval (`dispatch`), the approved run (`run_approved`), the named-host',
    '//! path (`run_on_named_host`) and the reply (`finish_run`). A child module of',
    '//! `src/daemon/executor/mod.rs`.',
    '',
    'use super::*;',
    '',
    '/// The approved call, carried whole so each stage keeps a short parameter',
    '/// list. Every field is a borrow or a copy, so stages pass it by value.',
    '#[derive(Clone, Copy)]',
    "struct ApprovedRun<'a> {",
    "    id: &'a String,",
    "    cmd: &'a String,",
    "    shell: &'a Option<String>,",
    "    timeout_secs: &'a Option<u64>,",
    "    interactive: &'a bool,",
    '    cmd_id: usize,',
    "    session_id: Option<&'a str>,",
    '}',
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
    '    let session_id = ctx.session_id;',
    '    match call {',
    *arm_head,
    '                        let run = ApprovedRun {',
    '                            id,',
    '                            cmd,',
    '                            shell,',
    '                            timeout_secs,',
    '                            interactive,',
    '                            cmd_id,',
    '                            session_id,',
    '                        };',
    '                        let outcome = run_approved(run, host, tx, rx).await?;',
    '                        return Ok(ToolCallOutcome::Result(outcome));',
    *arm_tail,
    '        other => anyhow::bail!("{} is not run_terminal_command\'s foreground arm", other.tool_name()),',
    '    }',
    '}',
    '',
    '/// The approved run: the owner, the post-approval host lookup, then the',
    "/// local or named host's run. Returns the tool result's text.",
    'async fn run_approved<W, R>(',
    "    a: ApprovedRun<'_>,",
    '    host: crate::host::HostId,',
    '    tx: &mut W,',
    '    rx: &mut R,',
    ') -> anyhow::Result<String>',
    W_R + '{',
    '    let ApprovedRun {',
    '        id,',
    '        cmd,',
    '        shell,',
    '        timeout_secs,',
    '        interactive,',
    '        cmd_id,',
    '        session_id,',
    '    } = a;',
    *approved_head,
    *local_run,
    '                                finish_run(run, owner, a, tx).await?',
    '                            }',
    '                            crate::host::HostId::Named(name) => {',
    '                                run_on_named_host(a, name, &host, owner, max_per_owner, tx, rx).await?',
    '                            }',
    '                        };',
    '    Ok(outcome)',
    '}',
    '',
    "/// A named host's run: the permissive lookup, the connect, then the run",
    '/// over ssh. Returns the tool result\'s text.',
    'async fn run_on_named_host<W, R>(',
    "    a: ApprovedRun<'_>,",
    '    name: &str,',
    '    host: &crate::host::HostId,',
    '    owner: Owner,',
    '    max_per_owner: u32,',
    '    tx: &mut W,',
    '    rx: &mut R,',
    ') -> anyhow::Result<String>',
    W_R + '{',
    '    let ApprovedRun {',
    '        id,',
    '        cmd,',
    '        shell,',
    '        timeout_secs,',
    '        interactive,',
    '        cmd_id,',
    '        session_id,',
    '    } = a;',
    *named_body,
    '    finish_run(run, owner, a, tx).await',
    '}',
    '',
    "/// The reply for a finished run, local or named: the result text with its",
    '/// shell line, the stats, the `ToolResult` frame and the shell log line.',
    'async fn finish_run<W>(',
    '    run: anyhow::Result<(crate::shell::ShellId, shell_exec::ShellRunResult)>,',
    '    owner: Owner,',
    "    a: ApprovedRun<'_>,",
    '    tx: &mut W,',
    ') -> anyhow::Result<String>',
    'where\n    W: tokio::io::AsyncWriteExt + Unpin,\n{',
    '    let ApprovedRun {',
    '        id,',
    '        cmd,',
    '        interactive,',
    '        cmd_id,',
    '        session_id,',
    '        ..',
    '    } = a;',
    '    let text = ' + finish_body[0].strip(),
    *finish_body[1:],
    '    ;',
    '    Ok(text)',
    '}',
    '',
])
open(N, 'w').write(text)
call = ('        PendingCall::Foreground { .. } => {\n'
        '            dispatch_foreground::dispatch(call, tx, rx, ctx, &ghost_policy, is_ghost).await\n'
        '        }\n')
out = src[:a0] + [call] + src[a1 - 1:]          # keep the blank line before Background
t = '\n'.join(out)
decl = 'pub(crate) mod chat_sandbox;\n'
assert t.count(decl) == 1
t = t.replace(decl, decl + 'mod dispatch_foreground;\n')
open(F, 'w').write(t)
A = 'tests/hygiene_allowlist.txt'
al = open(A).read()
for old, new in [('file src/daemon/executor/mod.rs 1489\n', 'file src/daemon/executor/mod.rs 1102\n'),
                 ('fn src/daemon/executor/mod.rs execute_tool_call 797\n',
                  'fn src/daemon/executor/mod.rs execute_tool_call 409\n')]:
    assert al.count(old) == 1, old
    al = al.replace(old, new)
open(A, 'w').write(al)
C = 'CLAUDE.md'
cm = open(C).read()
old = "`load_tools`, invalid arguments and `spawn_ghost_shell`. `run_terminal_command` stays in `execute_tool_call` |"
assert cm.count(old) == 1
cm = cm.replace(old, "`load_tools`, invalid arguments and `spawn_ghost_shell`. `run_terminal_command`'s background arm stays in `execute_tool_call` |")
row = '| `src/daemon/executor/dispatch_schedule.rs`,'
i = cm.index(row)
j = cm.index('\n', i)
cm = (cm[:j + 1] + '| `src/daemon/executor/dispatch_foreground.rs` | `run_terminal_command`\'s foreground arm in stages: '
      '`dispatch` (the ghost and host gates, the approval prompt, the refusal fallback), `run_approved` (the owner and '
      'the post-approval host lookup, then the local run), `run_on_named_host` (the permissive lookup, the connect and '
      'the run over ssh) and `finish_run` (the result text, stats, `ToolResult` frame and shell log line, shared by both '
      'hosts); `ApprovedRun` carries the call\'s fields between them |\n' + cm[j + 1:])
open(C, 'w').write(cm)
PY
cargo fmt --all
cargo fmt --all   # twice, as in the earlier moves
wc -l src/daemon/executor/mod.rs src/daemon/executor/dispatch_foreground.rs
```

## Acceptance criteria

- [ ] `dispatch_foreground.rs` exists with `dispatch`, `run_approved`,
      `run_on_named_host` and `finish_run`, each within 150 lines.
      `execute_tool_call`'s `Foreground` arm is one call to
      `dispatch_foreground::dispatch`.
- [ ] The allowlist reads `file src/daemon/executor/mod.rs 1102` and
      `fn src/daemon/executor/mod.rs execute_tool_call 409`, and
      `cargo test --test hygiene` passes.
- [ ] The End-to-end block prints `segments_present=1`,
      `result_blocks=1` and `passed=2587 failed=0`, on the first run or on
      the one re-run the flake note allows.
- [ ] The Update Log has a `(cuts done)` entry, its own
      `### Findings — <date>` entry and an `(end-to-end verification)`
      entry, each written with an edit tool.

## End-to-end verification

Paste the output byte-exact into the Update Log (Spec 4). The segment
check compares the text with whitespace removed. It also allows for
three things rustfmt does after a dedent:
- it drops a trailing comma when it rejoins a wrapped call;
- it drops a one-expression closure's braces (`|id| { id.to_string() }`
  becomes `|id| id.to_string()`);
- it applies the one dictated rewrite in item 2 above.

```sh
bash <<'E2E'
B=266862b4; D=src/daemon/executor; F=$D/mod.rs; N=$D/dispatch_foreground.rs
A=/tmp/e2e-m31-29.txt; : > "$A"
wc -l $F $N | sed 's/^ *//' >> "$A"
grep -E '^(file|fn) src/daemon/executor/mod.rs (execute_tool_call )?[0-9]+$' tests/hygiene_allowlist.txt >> "$A"
python3 - "$B" <<'PY' >> "$A"
import re, subprocess, sys
old = subprocess.run(['git', 'show', sys.argv[1] + ':src/daemon/executor/mod.rs'],
                     capture_output=True, text=True, check=True).stdout.split('\n')
new = open('src/daemon/executor/dispatch_foreground.rs').read()
def at(t, a=0):
    return next(i for i in range(a, len(old)) if old[i] == t)
def norm(lines):
    s = re.sub(r'\s+', '', '\n'.join(lines).replace('return Ok(ToolCallOutcome::Result(msg));', 'return Ok(msg);'))
    s = s.replace(',)', ')').replace(',}', '}').replace(',]', ']')
    return re.sub(r'\|(\w+)\|\{([^{};]*)\}', r'|\1|\2', s)   # rustfmt drops a one-expression closure's braces
a0 = at('        PendingCall::Foreground {')
a1 = at('        PendingCall::Background { id, cmd, host, .. } => {', a0)
ok = at('                    Ok(cmd_id) => {', a0)
ret = at('                        return Ok(ToolCallOutcome::Result(outcome));', ok)
om = at('                        let outcome = match &host {', ok)
lo = at('                            crate::host::HostId::Local => {', om)
na = at('                            crate::host::HostId::Named(name) => {', om)
rl = at('                                match run {', lo)
rn = at('                                match run {', na)
segs = {'gates_and_approval': old[a0:ok + 1], 'refusal': old[ret + 2:a1 - 1],
        'approved_head': old[ok + 1:lo + 1], 'local_run': old[lo + 1:rl],
        'named_checks_and_run': old[na + 1:rn], 'result_block': old[rl:na - 1]}
n = norm([new])
missing = [k for k, v in segs.items() if norm(v) not in n]
for k in missing:
    print('missing segment:', k)
print('segments_present=%d' % (not missing))
print('result_blocks=%d' % new.count('shell_run_error_text('))
PY
cargo test --test hygiene 2>&1 | grep -E '^test result' | sed 's/finished in [0-9.]*s/finished in <t>s/' >> "$A"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1 | sed 's/in [0-9.]*s/in <t>s/' >> "$A"
cargo fmt --all --check; echo "fmt_exit=$?" >> "$A"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}' >> "$A"
cat "$A"
E2E
```

Expected (from the dry run):
- `2166 …/mod.rs` and `447 …/dispatch_foreground.rs`;
- the two allowlist lines at 1102 and 409;
- `segments_present=1` and `result_blocks=1`;
- hygiene `ok.`, clippy clean and `fmt_exit=0`;
- `passed=2587 failed=0`.

## Authorizations

- Edit `src/daemon/executor/mod.rs`; create
  `src/daemon/executor/dispatch_foreground.rs`.
- Edit `tests/hygiene_allowlist.txt` and `CLAUDE.md`, through the script
  only.

## Out of scope

- The `Background` arm and the code before the match: the next phases.
- #82 (drop the second `host_entry_or_bare` call), #83 (a test for this
  path), and every other finding (decision 6).

## Update Log

<!-- entries appended below this line -->

### Update — 2026-10-28 12:08 (started)

**Executor:** executor (this run).

**Progress:** Flipped the phase and milestone-README status to `in-progress`.
Next: run the Spec 1 script as written, once, then log its output.

### Update — 2026-10-28 12:17 (cuts done)

**What I ran:** the Spec 1 script, once, as written, via
`bash /tmp/run_phase29.sh` (the heredoc copied verbatim, unindented).

**Script output (the `wc -l` tail; all Python asserts passed silently):**

```
  2166 src/daemon/executor/mod.rs
   447 src/daemon/executor/dispatch_foreground.rs
  2613 total
```

Both counts match the dry run's expected `2166 …/mod.rs` and
`447 …/dispatch_foreground.rs`. Next: the Spec 3 findings grep, then the
Spec 4 end-to-end block.

### Findings — 2026-10-28

`grep -nE '\.(unwrap|expect)\(|panic!|let _ =|\.ok\(\)|unwrap_or_default|unwrap_or_else' src/daemon/executor/dispatch_foreground.rs`

- L49 `.unwrap_or_default()` — `dispatch`, config-load fallback
  (`Config::load().unwrap_or_default().sandbox.enabled`), in the ghost gate.
  Idiomatic default-on-unreadable-config, pre-existing (moved verbatim).
  Severity: **low**.
- L189 `.unwrap_or_default()` — `run_approved`,
  `Owner::Chat(session_id.unwrap_or_default().to_string())`. Owner string
  degrades to `""` when the session has no id; pre-existing (moved verbatim).
  Severity: **low**.
- L191 `.unwrap_or_default()` — `run_approved`, the second
  `Config::load()` fallback (`shells.max_per_owner`). Same as L49.
  Severity: **low**.
- L423 `.unwrap_or_else(|| "s?".to_string())` — `finish_run`, the shell-id
  fallback in the `Err` arm of the run match, used only to word the
  shell-error text. Intentional cosmetic fallback, pre-existing (moved
  verbatim). Severity: **low**.

All four are moved-verbatim lines from the old arm, not new calls. None is an
error-suppressing idiom in a new production path.

### Update — 2026-10-28 12:21 (end-to-end verification)

Ran the Spec 4 block once (`bash` heredoc as written); first run, no flake
re-run needed. Output redirected to `/tmp/e2e-m31-29.txt` and pasted
byte-exact:

```
2166 src/daemon/executor/mod.rs
447 src/daemon/executor/dispatch_foreground.rs
2613 total
file src/daemon/executor/mod.rs 1102
fn src/daemon/executor/mod.rs execute_tool_call 409
segments_present=1
result_blocks=1
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in <t>s
    Finished `dev` profile [unoptimized + debuginfo] target(s) in <t>s
fmt_exit=0
passed=2587 failed=0
```

`segments_present=1` — all six original arm segments found verbatim
(whitespace-normalised, with the three allowed rustfmt rewrites) in
`dispatch_foreground.rs`. `result_blocks=1` — one `shell_run_error_text(`
call remains, in the shared `finish_run`. Allowlist at 1102 / 409; hygiene
test `ok.`; clippy clean; `fmt_exit=0`; full suite `passed=2587 failed=0`,
matching the dry run.
