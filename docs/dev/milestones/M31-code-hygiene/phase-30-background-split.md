# Phase 30 — Split `run_terminal_command`'s background arm into stages

**Status:** todo
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
