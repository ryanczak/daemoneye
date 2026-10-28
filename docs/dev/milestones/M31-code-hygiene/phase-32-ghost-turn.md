# Phase 32 — Move `do_ghost_turn` out of `ghost.rs`

**Status:** todo
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
