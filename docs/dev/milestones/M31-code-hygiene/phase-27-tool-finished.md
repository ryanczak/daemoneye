# Phase 27 — Silent tools that exit early still send `ToolFinished`

**Status:** todo
**Milestone:** M31 — Code hygiene
**Drafted:** 2026-10-28

## Goal

Fix `FINDINGS.md` #73. `execute_tool_call` (`src/daemon/executor/mod.rs`)
sends `ToolStarted` before its `match call` and `ToolFinished` after it, for
every tool in `PendingCall::should_emit_tool_feedback`. Four of those
tools' arms contain `return Ok(…)`, which leaves the function before
`ToolFinished`:
- `ReadFile` and `OpenShell`: three error and refusal paths each;
- `SpawnGhost`: the depth refusal;
- `GetShellContext`: an invalid `scope`, `scope: "shells"`, and every call
  from outside tmux.

The chat client then loses the call's panel, or commits it with no ✓/✗ or
timing label.

The fix: each of these arms becomes a labeled block, and each early
`return Ok(…)` becomes `break 'arm Ok(…)`. The arm then evaluates to that
value, which reaches the same `?` and the same `ToolFinished` as every
other arm. Nothing else changes. An `Err` from `?` inside an arm still
ends the call, as it does for every tool.

This is the one recorded exception to M31's no-behaviour-change rule, and
it is a PE decision (README Notes, "#73 settled"). It lands as its own
`fix:` commit with a test, so the next phase can move these arms as a
pure move.

## Architecture references

- `docs/dev/milestones/M31-code-hygiene/FINDINGS.md`: #73.
- `docs/dev/milestones/M31-code-hygiene/README.md`: Notes, "#73 settled"
  (the PE decision), and decision 7.

## Spec

1. **Write the test first** — `silent_tool_early_exit_sends_tool_finished`,
   a `#[tokio::test]` in `src/daemon/executor/mod.rs`'s `mod tests`, next
   to `get_shell_context_without_chat_pane_is_shells_only`.
   - Use that test's harness: hermetic `HOME`, the registry seam, duplex
     pipes and `chat_pane: None`. It drives `PendingCall::GetShellContext`
     down the outside-tmux early exit.
   - After the call, read everything the daemon wrote to the client side
     of the pipe. It is newline-delimited JSON `Response`s.
   - Assert that a `ToolStarted` and then a `ToolFinished`, both with the
     call's id, arrive in that order.
   - Gotcha: drop the daemon's write half before reading to the end, or
     the read never returns.
2. **Prove it red.** Run the new test against the unfixed tree, then
   append a `### Update — <date> (red)` entry to the Update Log with an
   edit tool, quoting its failure: the `test result:` line and the assert
   message.
3. **Apply the fix by running the script below, once.** It turns each of the
   four arms into a labeled block and rewrites their early returns. It
   prints `replaced 10`, then runs `cargo fmt --all`. The script was
   dry-run on a scratch worktree: clippy was clean, and the existing suite
   still passed 2586. Do not hand-edit the four arms, and do not `sed -i`.
   The script is in **The fix script** below. It is a Python heredoc, so
   copy it **without adding indentation**.
4. **Findings** (decision 7) go in **their own `### Findings — <date>`
   entry**, for the four arms you touched (about 170 lines). Record each
   hit, with symbol, kind and severity, or write `none`:
   - an `expect`, `unwrap` or `panic!`;
   - a swallowed error;
   - logic duplicated between `ReadFile` and `OpenShell`;
   - anything else that looks wrong.

   #73 itself is not a finding.
5. **Run the End-to-end block, then paste its output into the Update Log
   with an edit tool**, as a `### Update — <date> (end-to-end
   verification)` entry. Paste only output you have already seen.

   **Known flake:** `read_shell_screen_lines_tails_the_visible_rows`
   (`FINDINGS.md` #65). If the block shows `passed=2586 failed=1`, run it
   once more and paste both outputs. Any other failure is real.
6. **Commit as `fix:`**, with the code and the test in one commit.

### The fix script

```bash
python3 - <<'PY'
import re
F='src/daemon/executor/mod.rs'
src=open(F).read().split('\n')
m=next(i for i,l in enumerate(src) if l=='    let outcome = match call {')
mend=next(i for i in range(m,len(src)) if src[i]=='    }?;')
anchors=[i for i in range(m+1,mend) if re.match(r'        (\| )?PendingCall::\w+', src[i]) and not src[i].startswith('        | ')]
n=0
for v in ['ReadFile','GetShellContext','OpenShell','SpawnGhost']:
    a=next(i for i in anchors if re.match(r'        PendingCall::'+v+r'\b', src[i]))
    b=next((i for i in anchors if i>a), mend)
    op=next(i for i in range(a,b) if src[i].endswith('=> {'))
    src[op]=src[op][:-len('{')]+"'arm: {"
    for i in range(op,b):
        if 'return Ok(' in src[i]:
            src[i]=src[i].replace('return Ok(',"break 'arm Ok(",1); n+=1
print('replaced', n)
open(F,'w').write('\n'.join(src))
PY
cargo fmt --all
```

## Acceptance criteria

- [ ] `silent_tool_early_exit_sends_tool_finished` exists, the `(red)`
      entry shows it failing on the unfixed tree, and it passes after the
      fix.
- [ ] The four arms open with `'arm: {`, and none of them contains
      `return Ok(`. The End-to-end block prints `labeled_arms=4` and
      `breaks=10`.
- [ ] The End-to-end block prints `passed=2587 failed=0` (the 2586 before
      plus the new test), on the first run or on the one re-run the flake
      note allows. Hygiene, clippy and fmt are clean.
- [ ] The Update Log has `(red)`, `### Findings — <date>` and
      `(end-to-end verification)` entries, each written with an edit tool.

## End-to-end verification

```sh
bash <<'E2E'
F=src/daemon/executor/mod.rs; A=/tmp/e2e-m31-27.txt; : > "$A"
echo "labeled_arms=$(grep -c "=> 'arm: {$" $F)" >> "$A"
echo "breaks=$(grep -c "break 'arm Ok(" $F)" >> "$A"
cargo test --lib silent_tool_early_exit_sends_tool_finished 2>&1 | grep -E '^test result' | sed 's/finished in [0-9.]*s/finished in <t>s/' >> "$A"
cargo test --test hygiene 2>&1 | grep -E '^test result' | sed 's/finished in [0-9.]*s/finished in <t>s/' >> "$A"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1 | sed 's/in [0-9.]*s/in <t>s/' >> "$A"
cargo fmt --all --check; echo "fmt_exit=$?" >> "$A"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}' >> "$A"
cat "$A"
E2E
```

Expected:
- `labeled_arms=4` and `breaks=10`;
- `1 passed` for the new test;
- hygiene `ok.`, clippy clean and `fmt_exit=0`;
- `passed=2587 failed=0`.

## Authorizations

- Edit `src/daemon/executor/mod.rs`: the four arms, through the script
  only, plus the new test in `mod tests`.

## Out of scope

- Moving any arm out of `execute_tool_call`. The next phase does that.
- Any other tool's feedback, or the client's handling of an orphaned
  `ToolStarted`.

## Update Log

<!-- entries appended below this line -->
