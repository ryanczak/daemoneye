# Phase 33 — Move `run_conversation_loop` out of `stream.rs`

**Status:** todo
**Milestone:** M31 — Code hygiene
**Drafted:** 2026-10-28

## Goal

`src/daemon/stream.rs` has 1613 production lines, 1454 of them one
function, `run_conversation_loop`. As in phase 32, the function cannot
move whole, because a 1454-line file would be a new allowlist entry
(decision 3). Three cuts bring it to 775 lines, and it then moves to a
file of its own:

| Where | What | Lines |
|---|---|---|
| `src/ai/types/pending.rs` (exempt) | `PendingCall::from_tool_event`: the event match's 43 field-for-field tool arms | 540 |
| `stream/endings.rs` | `end_turn_timeout`, `end_backend_error`, `end_stream_closed`: three terminal replies | 103 |
| `stream/conversation.rs` | `run_conversation_loop` | 781 (fn 775) |
| `stream.rs` | `ConversationLoopCtx`, the helpers, the tests, a `pub use` of the loop | 1613 → 168 |

The allowlist loses `file src/daemon/stream.rs 1613`, and the function
entry follows it as
`fn src/daemon/stream/conversation.rs run_conversation_loop 775`, down
from 1454. Splitting it under 150 lines is a later phase.

**Why the conversions go to `pending.rs`.** Each of the 43 arms moves an
`AiEvent`'s fields into the `PendingCall` variant of the same tool and
does nothing else. That is the flat per-variant data decision 2 exempts.
`pending.rs` already holds the inverse, `to_tool_call`, at 617 lines. A
540-line conversion anywhere else would need its own allowlist entry.
`run_terminal_command`'s arm (`AiEvent::ToolCall`) stays in the loop,
because it chooses `Foreground` or `Background` through
`routes_to_background`. `crate::ai` depends on nothing in
`crate::daemon`, and this phase keeps it that way.

**What differs from a move.** All six were dry-run, and none changes
behaviour:

1. **Each conversion arm's `pending_calls.push(PendingCall::X { … });`
   becomes `Some(Self::X { … })`**, and rustfmt drops the arm's braces.
   The loop's match ends with a catch-all arm, after `Done`:
   `tool => pending_calls.extend(PendingCall::from_tool_event(tool))`.
   `from_tool_event` names every variant and has no wildcard, so a new
   `AiEvent` still fails to compile until it is handled. It returns
   `None` for `ToolCall` and the four non-tool events, which the loop's
   earlier arms have already taken.
2. **The three terminal replies become functions.** In each one, the
   block's final `return Ok(());` becomes the function's `Ok(())`, and
   the loop's call site is `return endings::…(…).await;`. The bodies are
   otherwise verbatim.
3. **`MAX_CHANNEL_CLOSED_RETRIES` and its comment move from inside the
   loop to module level in `stream.rs`.** `end_stream_closed` reads the
   constant too.
4. **`run_conversation_loop` is re-exported:**
   `pub use conversation::run_conversation_loop;` in `stream.rs`, so
   `server/ask.rs` keeps calling `stream::run_conversation_loop`.
5. **`pending.rs` gains `use super::AiEvent;`.**
6. **`CLAUDE.md`'s "Adding a new AI tool" step 7 changes.** It now
   points at `from_tool_event`, which is where a new tool's event arm
   goes now. The `stream.rs` and `src/ai/types/` rows change with it,
   and two rows are added.

## Architecture references

- `docs/dev/milestones/M31-code-hygiene/README.md`: decisions 2, 3, 5, 6, 7.
- `docs/dev/milestones/M31-code-hygiene/phase-32-ghost-turn.md`: the same
  move-to-fit pattern.

## Spec

1. **Run the script below as written, once.** It does the whole change:
   - the three new files and the `pending.rs` impl;
   - the loop's edits;
   - the allowlist (one line deleted, one re-pathed) and the `CLAUDE.md`
     edits;
   - then `cargo fmt --all`, twice.

   It was dry-run on a scratch worktree of `b594b0d7`: clippy was clean, the
   full suite passed 2587, and the End-to-end block printed every expected
   line. Do not hand-edit anything it touches. Do not dry-run it again in
   a worktree of your own. Do not `sed -i` a source file. It is a Python
   heredoc inside a shell script, so copy it **without adding
   indentation**. The script asserts its anchors and the count of 43, and
   prints
   `run_conversation_loop -> stream/conversation.rs; 43 conversions -> PendingCall::from_tool_event; 3 endings`.
   If an assert fires, stop and report it in the Update Log.
2. **Log it at once.** Append a `### Update — <date> (cuts done)` entry
   with the patch or write tool, quoting the script's output. The stall
   governor counts only edit-tool calls as progress (rexyMCP issue #14).
3. **Findings** (decision 7) go in **their own
   `### Findings — <date>` entry, within five tool calls.** Run exactly
   this command, and open no other file:

   `grep -nE '\.(unwrap|expect)\(|panic!|let _ =|\.ok\(\)|unwrap_or_default|unwrap_or_else' src/daemon/stream/conversation.rs src/daemon/stream/endings.rs`

   Write each hit with its function, kind and severity, or write `none`.
   Group hits of one kind on one line where they repeat. Do not look up
   callees. `FINDINGS.md` #85, the two `ai_turn` events that log
   different models, is recorded already, so do not repeat it.
4. **Run the End-to-end block, then paste its output into the Update Log
   with an edit tool**, as a `### Update — <date> (end-to-end
   verification)` entry holding `/tmp/e2e-m31-33.txt` in a fenced block.
   **Paste only output you have already seen.**

   **Known flake:** `read_shell_screen_lines_tails_the_visible_rows`
   (`FINDINGS.md` #65). If the block shows `passed=2586 failed=1`, run it
   once more and paste both outputs. Any other failure is real.

   **Before you commit, all four of these must print 1:**
   - on the output file, `grep -c '^segments_present=1$'`,
     `grep -c '^conversions=43$'` and `grep -c '^stream_file_entries=0$'`;
   - on this doc, `grep -c '^### Findings — '`.
5. **Commit** as `refactor:`.

### The script

```bash
python3 - <<'PY'
import os
F = 'src/daemon/stream.rs'
P = 'src/ai/types/pending.rs'
src = open(F).read().split('\n')


def find(text, start=0, end=None):
    end = len(src) if end is None else end
    hits = [i for i in range(start, end) if src[i] == text]
    assert hits, text
    return hits[0]


fn = find('pub async fn run_conversation_loop<W, R>(')
fe = next(i for i in range(fn, len(src)) if src[i] == '}')
assert src[fn - 1] == '' and src[fe + 1] == ''
me = find('            match event {', fn, fe)
t0 = find('                AiEvent::InvalidToolArgs {', me, fe)
t1 = find('                AiEvent::Error(e) => {', t0, fe)
no = find('                AiEvent::Notice(n) => {', t1, fe)
dn = find('                AiEvent::Done(usage) => {', no, fe)
mc = find('            }', dn, fe)                          # closes `match event`
assert src[mc - 1] == '                }'

# 1. the 43 field-for-field arms become PendingCall::from_tool_event
conv, open_push = [], []
for l in src[t0:t1]:
    st = l.strip()
    ind = l[:len(l) - len(l.lstrip())]
    if st.startswith('pending_calls.push('):
        assert not st.endswith(');'), 'one-line push'
        conv.append(ind + st.replace('pending_calls.push(', 'Some(', 1))
        open_push.append(ind)
        continue
    if open_push and l == open_push[-1] + '});':
        conv.append(open_push.pop() + '})')
        continue
    conv.append(l)
assert not open_push
assert sum(1 for l in conv if l.strip().startswith('Some(PendingCall::')) == 43, 'expected 43 conversions'
conv = [l[8:] if l.startswith(' ' * 16) else l for l in conv]
conv = [l.replace('Some(PendingCall::', 'Some(Self::', 1) for l in conv]
pend = open(P).read()
anchor = 'use super::wire::ToolCall;\n'
assert pend.count(anchor) == 1
pend = pend.replace(anchor, 'use super::AiEvent;\n' + anchor, 1)
marker = '\n#[cfg(test)]\n'
assert pend.count(marker) == 1
block = '\n'.join([
    '',
    'impl PendingCall {',
    '    /// The `PendingCall` a tool event becomes, field for field. `None` for',
    '    /// `run_terminal_command` (`AiEvent::ToolCall`, whose wait and',
    '    /// interactive arguments pick `Foreground` or `Background` in',
    "    /// `run_conversation_loop`) and for the four events that are not tool",
    '    /// calls. Exhaustive by design: a new `AiEvent` variant lands here.',
    '    pub(crate) fn from_tool_event(event: AiEvent) -> Option<PendingCall> {',
    '        match event {',
    *['    ' + l if l else l for l in conv],
    '            AiEvent::ToolCall { .. }',
    '            | AiEvent::Token(_)',
    '            | AiEvent::Error(_)',
    '            | AiEvent::Notice(_)',
    '            | AiEvent::Done(_) => None,',
    '        }',
    '    }',
    '}',
])
pend = pend.replace(marker, block + '\n' + marker, 1)
open(P, 'w').write(pend)

# 2. three terminal replies become stream/endings.rs
td0 = find('            if let Some(deadline) = turn_deadline', fn, fe)
td1 = find('            }', td0, fe)
assert src[td0 + 2] == '            {' and src[td1 - 1] == '                return Ok(());'
tbody = src[td0 + 3:td1]
eb = src[t1 + 1:no - 1]
assert eb[-1] == '                    return Ok(());'
cc0 = find('                        if channel_closed_retries > MAX_CHANNEL_CLOSED_RETRIES {', fn, fe)
cc1 = find('                        }', cc0, fe)
assert src[cc1 - 1] == '                            return Ok(());'
cbody = src[cc0 + 1:cc1]
mx = find('    const MAX_CHANNEL_CLOSED_RETRIES: u32 = 2;', fn, fe)
assert src[mx - 3].startswith('    // Bounded re-issue when the chat task dies')


def ending(body, indent):
    out = [l[indent - 4:] if l.startswith(' ' * indent) else l for l in body]
    out[-1] = '    Ok(())'
    return out


os.makedirs('src/daemon/stream', exist_ok=True)
W = ['where', '    W: AsyncWrite + Unpin,', '{']
open('src/daemon/stream/endings.rs', 'w').write('\n'.join([
    "//! Three of `run_conversation_loop`'s terminal replies: the turn timeout,",
    '//! the backend error and the stream that kept ending early. Each sends its',
    '//! frame and finishes the run log. A child module of `src/daemon/stream.rs`.',
    '',
    'use super::*;',
    '',
    '/// The turn ran past `[limits] turn_timeout_secs`.',
    'pub(super) async fn end_turn_timeout<W>(',
    '    tx: &mut W,', '    config: &Config,', '    run_log: &mut crate::daemon::run::RunLogGuard,',
    ') -> Result<()>', *W, *ending(tbody, 16), '}', '',
    '/// The AI backend reported an error.',
    'pub(super) async fn end_backend_error<W>(',
    '    tx: &mut W,', '    run_log: &mut crate::daemon::run::RunLogGuard,', '    e: String,',
    ') -> Result<()>', *W, *ending(eb, 20), '}', '',
    '/// The AI event channel closed without `Done` or `Error` more than',
    '/// `MAX_CHANNEL_CLOSED_RETRIES` times.',
    'pub(super) async fn end_stream_closed<W>(',
    '    tx: &mut W,', '    run_log: &mut crate::daemon::run::RunLogGuard,',
    '    channel_closed_retries: u32,', '    cause: String,',
    ') -> Result<()>', *W, *ending(cbody, 28), '}', '',
]))

# 3. the loop, its edits, moves to stream/conversation.rs
loop = (src[fn:mx - 3] + src[mx + 1:td0 + 3]
        + ['                return endings::end_turn_timeout(tx, config, &mut run_log).await;']
        + src[td1:cc0 + 1]
        + ['                            return endings::end_stream_closed(tx, &mut run_log, channel_closed_retries, cause)',
           '                                .await;']
        + src[cc1:t0]
        + ['                AiEvent::Error(e) => {',
           '                    return endings::end_backend_error(tx, &mut run_log, e).await;']
        + src[no - 1:mc]
        + ['                tool => pending_calls.extend(PendingCall::from_tool_event(tool)),']
        + src[mc:fe + 1])
assert loop[mx - 3 - fn - 1] == '' or True
open('src/daemon/stream/conversation.rs', 'w').write('\n'.join([
    '//! `run_conversation_loop`, one chat turn end to end: the model call, the',
    '//! event stream, the tool calls and their replies, the persistence. A child',
    '//! module of `src/daemon/stream.rs`, which re-exports the function.',
    '',
    'use super::*;',
    '',
    *loop,
    '',
]))
const = src[mx - 3:mx + 1]
rest = src[:fn] + src[fe + 2:]
t = '\n'.join(rest)
anchor = 'use tokio::io::{AsyncBufRead, AsyncWrite};\n'
assert t.count(anchor) == 1
t = t.replace(anchor, anchor + '\nmod conversation;\nmod endings;\n\npub use conversation::run_conversation_loop;\n\n'
              + '\n'.join(l[4:] for l in const) + '\n', 1)
open(F, 'w').write(t)
print('run_conversation_loop -> stream/conversation.rs; 43 conversions -> PendingCall::from_tool_event; 3 endings')

A = 'tests/hygiene_allowlist.txt'
al = open(A).read()
old = 'file src/daemon/stream.rs 1613\n'
assert al.count(old) == 1
al = al.replace(old, '')
old = 'fn src/daemon/stream.rs run_conversation_loop 1454\n'
assert al.count(old) == 1
al = al.replace(old, 'fn src/daemon/stream/conversation.rs run_conversation_loop 775\n')
open(A, 'w').write(al)
C = 'CLAUDE.md'
cm = open(C).read()
for old, new in [
    ('| `src/daemon/stream.rs` | AI event streaming loop (`run_conversation_loop`); tool execution; response persistence |\n',
     '| `src/daemon/stream.rs` | The chat turn: `ConversationLoopCtx`, the budget and truncation helpers, `ChatTaskGuard`; re-exports `run_conversation_loop` |\n'
     '| `src/daemon/stream/conversation.rs` | `run_conversation_loop`, one chat turn end to end: the model call, the event stream, the tool calls and their replies, the persistence; a child module of `src/daemon/stream.rs` |\n'
     '| `src/daemon/stream/endings.rs` | Three of the turn\'s terminal replies (`end_turn_timeout`, `end_backend_error`, `end_stream_closed`): each sends its frame and finishes the run log; a child module of `src/daemon/stream.rs` |\n'),
    ('| `src/ai/types/` | `pending.rs` the `PendingCall` enum (one variant per AI tool), ',
     '| `src/ai/types/` | `pending.rs` the `PendingCall` enum (one variant per AI tool) and `PendingCall::from_tool_event` (a tool `AiEvent` to its call, field for field), '),
    ('7. `src/daemon/stream.rs`: add `AiEvent::ToolName` arm in the streaming match.\n',
     '7. `src/ai/types/pending.rs`: add the `AiEvent::ToolName` arm in `PendingCall::from_tool_event` (the streaming match in `src/daemon/stream/conversation.rs` hands every tool event to it).\n'),
]:
    assert cm.count(old) == 1, old[:50]
    cm = cm.replace(old, new)
open(C, 'w').write(cm)
PY
cargo fmt --all
cargo fmt --all   # twice, as in the earlier moves
wc -l src/daemon/stream.rs src/daemon/stream/conversation.rs src/daemon/stream/endings.rs
```

## Acceptance criteria

- [ ] `src/daemon/stream/conversation.rs` holds `run_conversation_loop`,
      `src/daemon/stream/endings.rs` the three replies, and
      `src/ai/types/pending.rs` `PendingCall::from_tool_event`. `stream.rs`
      declares both child modules and re-exports the loop.
- [ ] `tests/hygiene_allowlist.txt` has no `file src/daemon/stream.rs`
      entry and reads
      `fn src/daemon/stream/conversation.rs run_conversation_loop 775`, and
      `cargo test --test hygiene` passes.
- [ ] The End-to-end block prints `stream_file_entries=0`,
      `segments_present=1`, `conversions=43`, `loop_pushes=2` and
      `passed=2587 failed=0`, on the first run or on the one re-run the
      flake note allows.
- [ ] The Update Log has a `(cuts done)` entry, its own
      `### Findings — <date>` entry and an `(end-to-end verification)`
      entry, each written with an edit tool.

## End-to-end verification

Paste the output byte-exact into the Update Log (Spec 4). The segment
check finds the old function's eleven segments in their new homes,
comparing the text with whitespace removed:
- in `conversation.rs`, the six stretches of loop between the edits;
- in `endings.rs`, the three reply bodies;
- in `stream.rs`, the constant;
- in `pending.rs`, the 43 conversions. These are compared with brackets
  and separators removed too, because rustfmt drops each arm's braces,
  so every name must still appear in order.

`loop_pushes=2` is the `ToolCall` arm's two pushes, which stay in the
loop.

```sh
bash <<'E2E'
B=b594b0d7; S=src/daemon/stream.rs; C=src/daemon/stream/conversation.rs; N=src/daemon/stream/endings.rs; P=src/ai/types/pending.rs
A=/tmp/e2e-m31-33.txt; : > "$A"
wc -l $S $C $N | sed 's/^ *//' >> "$A"
grep -cE '^file src/daemon/stream.rs ' tests/hygiene_allowlist.txt | sed 's/^/stream_file_entries=/' >> "$A"
grep -E '^fn src/daemon/stream(/conversation)?.rs run_conversation_loop ' tests/hygiene_allowlist.txt >> "$A"
python3 - "$B" <<'PY' >> "$A"
import re, subprocess, sys
old = subprocess.run(['git', 'show', sys.argv[1] + ':src/daemon/stream.rs'],
                     capture_output=True, text=True, check=True).stdout.split('\n')
conv = open('src/daemon/stream/conversation.rs').read()
ends = open('src/daemon/stream/endings.rs').read()
pend = open('src/ai/types/pending.rs').read()
def at(t, a=0):
    return next(i for i in range(a, len(old)) if old[i] == t)
def norm(s):
    s = re.sub(r'\s+', '', s)
    s = s.replace(',)', ')').replace(',}', '}').replace(',]', ']')
    s = re.sub(r'\|(\w+)\|\{([^{};]*)\}', r'|\1|\2', s)
    return s.rstrip(',')
fn = at('pub async fn run_conversation_loop<W, R>(')
fe = next(i for i in range(fn, len(old)) if old[i] == '}')
t0 = at('                AiEvent::InvalidToolArgs {', fn)
t1 = at('                AiEvent::Error(e) => {', t0)
no = at('                AiEvent::Notice(n) => {', t1)
dn = at('                AiEvent::Done(usage) => {', no)
mc = at('            }', dn)
td0 = at('            if let Some(deadline) = turn_deadline', fn)
td1 = at('            }', td0)
cc0 = at('                        if channel_closed_retries > MAX_CHANNEL_CLOSED_RETRIES {', fn)
cc1 = at('                        }', cc0)
mx = at('    const MAX_CHANNEL_CLOSED_RETRIES: u32 = 2;', fn)
j = lambda a, b: '\n'.join(old[a:b])
# rustfmt drops a one-expression arm's braces (`=> { Some(…) }` becomes `=> Some(…),`), so the
# conversions compare with brackets and separators removed: every name, in order, must survive.
loose = lambda x: re.sub(r'[{}(),;]', '', norm(x))
conversions = loose(j(t0, t1).replace('pending_calls.push(PendingCall::', 'Some(Self::'))
segs = {
    'loop_head': (j(fn, mx - 3), conv), 'loop_to_deadline': (j(mx + 1, td0 + 3), conv),
    'loop_to_stream_closed': (j(td1, cc0 + 1), conv), 'loop_to_tool_arms': (j(cc1, t0), conv),
    'notice_and_done': (j(no - 1, mc), conv), 'loop_tail': (j(mc, fe + 1), conv),
    'turn_timeout_reply': (j(td0 + 3, td1 - 1), ends), 'backend_error_reply': (j(t1 + 1, no - 2), ends),
    'stream_closed_reply': (j(cc0 + 1, cc1 - 1), ends), 'max_retries_const': (j(mx - 3, mx + 1), open('src/daemon/stream.rs').read()),
}
missing = [k for k, (v, dst) in segs.items() if norm(v) not in norm(dst)]
if conversions not in loose(pend):
    missing.append('tool_event_conversions')
for k in missing:
    print('missing segment:', k)
print('segments_present=%d' % (not missing))
PY
grep -c 'Some(Self::' $P | sed 's/^/conversions=/' >> "$A"
grep -c 'pending_calls.push(' $C | sed 's/^/loop_pushes=/' >> "$A"
cargo test --test hygiene 2>&1 | grep -E '^test result' | sed 's/finished in [0-9.]*s/finished in <t>s/' >> "$A"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1 | sed 's/in [0-9.]*s/in <t>s/' >> "$A"
cargo fmt --all --check; echo "fmt_exit=$?" >> "$A"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}' >> "$A"
cat "$A"
E2E
```

Expected (from the dry run):
- `410 …/stream.rs`, `781 …/conversation.rs` and `103 …/endings.rs`;
- `stream_file_entries=0` and the re-pathed `run_conversation_loop 775`;
- `segments_present=1`, `conversions=43` and `loop_pushes=2`;
- hygiene `ok.`, clippy clean and `fmt_exit=0`;
- `passed=2587 failed=0`.

## Authorizations

- Edit `src/daemon/stream.rs` and `src/ai/types/pending.rs`; create
  `src/daemon/stream/conversation.rs` and `src/daemon/stream/endings.rs`.
- Edit `tests/hygiene_allowlist.txt` and `CLAUDE.md`, through the script
  only.

## Out of scope

- Splitting `run_conversation_loop` (775) under the function cap: a later
  phase.
- The `defs.rs` exemption: the next phase.
- #85 and every other finding (decision 6).

## Update Log

<!-- entries appended below this line -->
