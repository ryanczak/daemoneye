# Phase 33 — Move `run_conversation_loop` out of `stream.rs`

**Status:** review
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

### Update — 2026-10-28 22:03 (started)

Executor: executor (phase 33). Status flipped to `in-progress`; beginning
with Spec 1, the script as written.

### Findings — 2026-10-28

`grep -nE '\.(unwrap|expect)\(|panic!|let _ =|\.ok\(\)|unwrap_or_default|unwrap_or_else' src/daemon/stream/conversation.rs src/daemon/stream/endings.rs`:
four hits, all in `conversation.rs`; `endings.rs` is clean.

- `run_conversation_loop`, line 42: `session_id.clone().unwrap_or_default()`
  — `unwrap_or_default`, low. The chat run log's session falls back to an
  empty string when the ask carried no session id.
- `run_conversation_loop`, line 141: `.unwrap_or_default()` —
  `unwrap_or_default`, low. The loaded-tools snapshot falls back to an
  empty set when the registry is unavailable.
- `run_conversation_loop`, line 154: `let _ = ai_tx.send(…)` — `let _ =`,
  low. The chat task's error event is dropped when the receiver is gone,
  which is fine at teardown.
- `run_conversation_loop`, line 251: `let _ = host;` — `let _ =`, low.
  Deliberate drop of a named host on a background command (row 08b, sre.toml).

No `.unwrap()`/`.expect()`/`panic!`/`.ok()`/`unwrap_or_else` hits. #85 is
recorded already and not repeated. Callees not looked up, per spec.

Ran the Spec 1 script as written, once, unmodified. Its output:

```
run_conversation_loop -> stream/conversation.rs; 43 conversions -> PendingCall::from_tool_event; 3 endings
  410 src/daemon/stream.rs
  781 src/daemon/stream/conversation.rs
  103 src/daemon/stream/endings.rs
 1294 total
```

No assert fired. `cargo fmt --all` ran twice after it, as in the earlier
moves. Line counts match the dry run's expectation (`410 …/stream.rs`,
`781 …/conversation.rs`, `103 …/endings.rs`).

### Update — 2026-10-28 22:08 (cuts done, order note)

The Findings entry above landed before the `(cuts done)` entry because the
spec's five-tool-call window for findings (decision 7) expires before the
`(cuts done)` entry's edit-tool call was available. Both are append-only
and complete; no content was edited after writing.

### Update — 2026-10-28 22:15 (end-to-end verification)

Ran the spec's End-to-end block once, on the first run. The transcript is
`/tmp/e2e-m31-33.txt`, pasted byte-exact below. Every expected line is
present: the three line counts (`410`/`781`/`103`), `stream_file_entries=0`,
the re-pathed `run_conversation_loop 775`, `segments_present=1`,
`conversions=43`, `loop_pushes=2`, hygiene `ok.`, clippy clean,
`fmt_exit=0`, `passed=2587 failed=0`. No flake re-run was needed.

```
410 src/daemon/stream.rs
781 src/daemon/stream/conversation.rs
103 src/daemon/stream/endings.rs
1294 total
stream_file_entries=0
fn src/daemon/stream/conversation.rs run_conversation_loop 775
segments_present=1
conversions=43
loop_pushes=2
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in <t>s
    Finished `dev` profile [unoptimized + debuginfo] target(s) in <t>s
fmt_exit=0
passed=2587 failed=0
```

### Update — 2026-10-28 22:09 (complete, server-authored)

**Summary:** **Summary:** Executed phase 33 exactly as specified: ran the Spec 1 script unmodified (all asserts passed, output `run_conversation_loop -> stream/conversation.rs; 43 conversions -> PendingCall::from_tool_event; 3 endings`), producing `stream/conversation.rs` (781 lines, loop 775), `stream/endings.rs` (103), `stream.rs` 1613 → 410, and the new `PendingCall::from_tool_event` in the exempt `pending.rs`; the allowlist file entry was deleted and the function entry re-pathed to 775, and `CLAUDE.md` gained the two new rows plus the step-7 retarget. Findings grep on the two new files returned four hits, all in `conversation.rs` and all low (two `unwrap_or_default`, two `let _ =`), logged in their own `### Findings — 2026-10-28` entry. The End-to-end block passed on the first run — `stream_file_entries=0`, `segments_present=1`, `conversions=43`, `loop_pushes=2`, hygiene `ok.`, clippy clean, `fmt_exit=0`, `passed=2587 failed=0` — pasted byte-exact into the `(end-to-end verification)` Update Log entry, and all four pre-commit greps printed 1. Committed as `refactor:` (8888d434); the working tree is clean and the phase status remains `in-progress` for server bookkeeping.

**Notes for review:** One ordering deviation — the `### Findings` entry landed in the Update Log *before* the `(cuts done)` entry, because the spec's five-tool-call window for findings expired before the `(cuts done)` entry's edit-tool call was available; both entries are complete and append-only, and I recorded the ordering in a short `(cuts done, order note)` entry. No flake re-run was needed. The `MAX_CHANNEL_CLOSED_RETRIES` constant moved to module level in `stream.rs` (410 lines, above the 775-line function cap), so the later phase that splits the loop under the function cap still has `stream.rs` itself within the file cap.

**Executor:** nvidia/GLM-5.3-Flash-FP4-Local

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
   Compiling daemoneye v2.3.0 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 11.82s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.16s


TEST
easured ... ok
test cfg_all_test_counts_as_test_only ... ok
test function_length_spans_signature_to_closing_brace ... ok
test production_lines_exclude_an_inline_test_module ... ok
test impl_and_trait_methods_are_measured_with_their_type ... ok
test allowlist_has_no_stale_entries ... ok
test exempt_files_exist ... ok
test functions_fit_the_cap ... ok
test production_files_fit_the_cap ... ok
test test_files_fit_the_cap ... ok

test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.30s


running 34 tests
test daemon_ping_status_loop ... ignored
test cancel_request_roundtrip ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g4_briefing_injection_block_format ... ok
test g5_child_inherits_depth_and_parent ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g5_depth_limit_enforced ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ghost_config_parsing ... ok
test ipc_ask_round_trip ... ok
test ipc_tool_call_response_round_trip ... ok
test resume_run_request_roundtrip ... ok
test minimal_config_parsing ... ok
test ipc_session_info_round_trip ... ok
test schedule_store_persistence ... ok
test config_pricing_round_trip ... ok
test window_switch_does_not_corrupt_chat ... ignored
test g4_briefing_masking_applied ... ok
test event_log_append_read ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test event_log_entry_format ... ok
test g4_briefing_read_and_clear ... ok
test g4_briefing_injects_on_next_run ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test session_index_persistence ... ok
test session_jsonl_round_trip ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_below_threshold_discarded ... ok
test webhook_alert_no_severity_passes_gate ... ok
test webhook_alert_to_event_log ... ok
test webhook_alert_unrankable_severity_passes_gate ... ok

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
test daemon_boots_in_throwaway_root ... ok
test daemon_installs_no_tmux_hooks ... ok
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
kill: (2435236): No such process
kill: (-2435880): No such process
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

- `CLAUDE.md` — +5 -3
- `docs/dev/milestones/M31-code-hygiene/README.md` — +1 -1
- `docs/dev/milestones/M31-code-hygiene/phase-33-conversation-loop.md` — +72 -1
- `src/ai/types/pending.rs` — +549 -0
- `src/daemon/stream/conversation.rs` — +781 -0
- `src/daemon/stream/endings.rs` — +103 -0
- `src/daemon/stream.rs` — +10 -1455
- `tests/hygiene_allowlist.txt` — +1 -2

**Commit:** 8888d434d88496d7e31128ca9e31669b5c5027ca

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).
