# Phase 01c: prompt-echo-race — make the test gate green again

**Milestone:** M21 — Host model & SSH
**Status:** todo
**Depends on:** phase-01b (whose test this is)
**Estimated diff:** ~60 lines, test-only
**Tags:** language=rust, kind=bugfix, size=s

## Goal

`cargo test` is **deterministically red on master**. One test,
`shell::backend::tests::the_flag_clears_once_the_answer_is_submitted`, fails
every run of the full suite and passes every run in isolation. Until it is
green, no phase in this milestone can meet the Definition of Done, because
`STANDARDS.md` §1 requires `cargo test` to pass.

This phase makes the suite green **without weakening the security property the
test exists to prove**. It is dispatched ahead of row 04c for that reason.

## Architecture references

- `docs/security.md` § 2 — shell logs at rest are raw and `0600`; the scrub is
  what keeps a typed credential out of the cast log in the first place.
- `CLAUDE.md` § Key files, `src/shell/pty.rs` — `wrap_command` "split
  `DE_''BEG` literal so the echoed command never matches its own marker". That
  is the same class of bug as this one, already solved once in this codebase.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

Everything below was measured on **2026-09-20** at commit `73bc8586`, on a
clean tree. Re-run anything you want to confirm; none of it is inferred.

### 1. The failure is deterministic under the suite and absent in isolation

```
$ for i in 1 2 3 4 5; do cargo test --lib | tail -1; done
FAILED. 1934 passed; 1 failed; 5 ignored      (x5)

$ cargo test --lib the_flag_clears_once_the_answer_is_submitted
ok. 1 passed; 0 failed; 0 ignored

$ cargo test --lib -- --test-threads=1
ok. 1935 passed; 0 failed; 5 ignored
```

Single-threaded it passes. It is a **timing race that parallel load loses**,
and it became deterministic when phase-04b added eight tests to the suite.
Earlier in the same day it failed roughly one run in four.

### 2. What the assertion sees

`src/shell/backend.rs:650` asserts the credential appears nowhere in the cast
file. The failure output, trimmed to the four lines that matter:

```
[0.033654, "i", "read -rs -p '[de-sudo-prompt-78] Password: ' v; printf '\\n'\n"]
[0.033909, "o", "read -rs -p '[de-sudo-prompt-78] Password: ' v; printf '\\n'\r\n"]
[0.094785, "i", "[redacted]"]
[0.094852, "o", "hunter2-clear-target\r\n"]
```

**The scrub is working.** The credential's `"i"` event is `[redacted]`, which
is exactly what phase-01b built. The credential reaches the cast as an **`"o"`
event** — the shell echoing it back — because terminal echo was still on when
the answer arrived.

### 3. Root cause: the flag arms on the *echo of the command*, not on the prompt

Look at the two timestamps above. `0.033654` is the test writing the fixture
command; `0.033909` is the PTY echoing that command line back. **The command
line contains the literal text `[de-sudo-prompt-78] Password: `**, so the
detector matches its own fixture's echo and arms `awaiting_secret` roughly
60 ms before `read -rs` ever runs and turns echo off.

The test's arm-wait loop (`backend.rs:610-619`) therefore returns almost
immediately, on a false arm. The `sleep(Duration::from_millis(50))` at
`backend.rs:623` is the only thing that was keeping the answer inside the real
echo-off window, and its own comment says so. Under parallel load 50 ms is no
longer enough for bash to get from "echoed the command" to "`read` has cleared
`ECHO`".

**This is the same bug `wrap_command` already solved** for the marker protocol
— see `src/shell/pty.rs`, where the `DE_''BEG` literal is deliberately split
so the echoed command cannot match its own marker.

### 4. Two fixes that do NOT work — measured, so you do not repeat them

Both were prototyped on a scratch copy at drafting and both **failed**:

- **Reordering the fixture so echo is off before the prompt is printed**
  (`stty -echo; printf '…Password: '; read -r v; stty echo`) made it *worse* —
  it then failed in isolation too (`0 passed; 1 failed`), because the echoed
  command line still carries the prompt text and now arms the flag even
  earlier relative to `stty`.
- **Splitting the prompt literal alone**
  (`'…Pass'"'"'word: '`) while deleting the sleep also failed in isolation.
  The split does stop the false arm, but with the sleep gone something else in
  the sequence is still unsynchronised.

Draw the obvious conclusion: **the split literal and the wait condition have to
be fixed together**, and the wait must be on a condition that is actually true
only once echo is off. Do not ship a longer sleep.

### 5. The property that must survive

The test is the only thing proving phase-01b's security guarantee: a
credential typed at a secret prompt never lands in the cast log. A "fix" that
relaxes the assertion, drops the whole-file negative, or marks the test
`#[ignore]` is a regression, not a fix, and Task 3's mutation is what catches
it.

## Spec

### Task 1 — make the wait deterministic

In `src/shell/backend.rs`, fix
`the_flag_clears_once_the_answer_is_submitted` so it passes under the full
parallel suite.

**Constraints, not a prescribed edit** — you hold the compiler and the test
runner, so choose the mechanism:

- The detector must **not** arm on the fixture's own echoed command line.
  Split the prompt literal the way `src/shell/pty.rs` splits `DE_''BEG`, so
  what `printf`/`read -p` *emits* matches the detector and what the shell
  *echoes back* does not.
- The answer must **not** be written until terminal echo is genuinely off.
  Wait on a condition that is only true then — not on a fixed duration. **No
  `sleep` may remain as the synchronisation primitive**; STANDARDS §3.3 says
  tests are deterministic and names `sleep` specifically. A bounded poll loop
  with a deadline is fine, which is the shape the file already uses at
  `backend.rs:610-619` and `backend.rs:627-638`.
- Do not change `src/shell/backend.rs`'s production code unless you find the
  defect is in the detector rather than the fixture. **If it is** — if the
  detector genuinely should not match a prompt pattern appearing in an `"i"`
  echo — say so in the Update Log and fix it there instead; that is a better
  outcome, not a scope violation.

### Task 2 — keep the assertions

Every existing assertion in the test stays, with its meaning intact: the
`[redacted]` `"i"` event, the follow-up logged in clear after the scrub
disarms, and the whole-file negative on the credential. You may add
assertions. You may not weaken or delete one, and you may not add `#[ignore]`.

### Task 3 — mutation pair

Apply each with the `patch` tool (the executor contract forbids `sed -i`,
`perl -i`, `> file` and `rm -rf`), run it, restore, and report what you
observed. A mutation that stays green is a finding to write down.

**Mutation A — defeat the scrub.** Make the input path log the raw bytes
instead of `[redacted]` while the secret flag is armed.
`the_flag_clears_once_the_answer_is_submitted` must go **red**. This proves
your fixed test still tests the thing.

**Mutation B — restore the race.** Replace your new wait condition with the
old `std::thread::sleep(Duration::from_millis(50))`. The test must go **red
under the full suite** (`cargo test --lib`), which is the failure this phase
exists to remove. If it stays green under the full suite, your fix may be
masking rather than resolving the race — say so.

### Task 4 — capture the end-to-end evidence

Run the block in § End-to-end verification verbatim and paste its output into
a new Update Log entry.

## Acceptance criteria

**Run this whole block before reporting done.** Drafting values are shown; the
first line is the one that currently fails.

- [ ] `cargo test --lib` → **1935 passed / 0 failed / 5 ignored**, and it must
      be green on **five consecutive runs** (drafting: FAILED 1934/1/5, five
      for five). The count is unchanged because this phase adds no test.
- [ ] `cargo test --lib -- --test-threads=1` → **1935 passed / 0 failed /
      5 ignored** (drafting: already green — *preservation criterion*).
- [ ] `cargo test --lib the_flag_clears_once_the_answer_is_submitted` →
      **1 passed** (drafting: already green in isolation — *preservation
      criterion*; the point is that it must stay green both ways).
- [ ] `cargo fmt --all` clean; `cargo build` clean;
      `cargo clippy --all-targets --all-features -- -D warnings` exits 0.
- [ ] `cargo test` → every target green: doc_truth 14, integration 32,
      isolation 9, bug_tracker 6 (*preservation criteria* — this phase must
      not touch them).
- [ ] No `#[ignore]` was added: `grep -c '#\[ignore' src/shell/backend.rs`
      → **0** (drafting: 0 — *preservation criterion*, and the one the
      "make it green by hiding it" shortcut would break).
- [ ] Both mutations in Task 3 were applied, run, and their observed results
      recorded in the Update Log.

## Test plan

No new test. This phase repairs an existing one, and the criteria above pin
that the count does not move — a **rising** lib count means work beyond this
phase's scope.

## End-to-end verification

The evidence is the gate itself, run the way CI runs it, five times, plus the
isolation and single-threaded runs that must stay green.

```sh
cd /home/matt/src/daemoneye
{
  for i in 1 2 3 4 5; do
    printf 'full%d=%s\n' "$i" "$(cargo test --lib 2>&1 | grep -E '^test result' | tail -1 | grep -oE 'ok|FAILED')"
  done
  printf 'single_threaded=%s\n' "$(cargo test --lib -- --test-threads=1 2>&1 | grep -E '^test result' | tail -1 | grep -oE 'ok|FAILED')"
  printf 'isolated=%s\n' "$(cargo test --lib the_flag_clears_once_the_answer_is_submitted 2>&1 | grep -E '^test result' | grep -oE 'ok|FAILED')"
  printf 'ignore_count=%s\n' "$(grep -c '#\[ignore' src/shell/backend.rs)"
} > /tmp/e2e-01c.txt
cat /tmp/e2e-01c.txt
```

Every one of the first seven values must read `ok`, and `ignore_count` must be
`0`. Paste the file's contents into an Update Log entry titled
`### Update — <date> (end-to-end verification)` **as that entry's first fenced
block**, then run:

```sh
D=docs/dev/milestones/M21-host-model/phase-01c-prompt-echo-race.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-01c.txt
diff /tmp/pasted-01c.txt /tmp/e2e-01c.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

The literal `PASTE MATCH` line goes **into** the entry. **If a value cannot be
captured, report it unmet** — do not redefine a term beneath its own line.

## Authorizations

You may edit, and nothing else:

- `src/shell/backend.rs` — the test, and the production detector **only if**
  § Spec Task 1's last bullet applies and you say so in the Update Log.
- This phase doc's Update Log.

## Out of scope

- **Everything to do with hosts and SSH.** This phase is a test-gate repair;
  rows 04c onward carry the milestone's feature work.
- **The other four tests in `shell::backend::tests`** — they are green and
  stay untouched.
- **Making the suite faster**, reordering tests, or changing
  `--test-threads`. The suite must be green at its default parallelism.
- **Any new `#[ignore]`**, anywhere.
- `src/shell/pty.rs` — it is quoted here as the worked example for the split
  literal, not as something to change.

## Update Log
