# Phase 04: Path Audit — Fenced Code Blocks

**Milestone:** M7 — Memory Search & Maintenance
**Status:** todo
**Depends on:** phase-03 (test-sleep-removal, done)
**Estimated diff:** ~90 lines in `src/config/path_audit.rs` (one function + tests)

**Tags:** language=rust, kind=feature, size=m

## Goal

`daemoneye audit-prompts` cannot see inside fenced code blocks, so a stale path
written in a shell example or a directory tree passes the gate silently. Three
such literals slipped through during M6. Teach the extractor to read fenced
blocks — using a rule narrow enough not to fire on shebangs or slash commands,
which is why this was deferred rather than done then.

## Architecture references

- `docs/dev/milestones/M6-verification-and-hygiene/README.md` § retrospective,
  open question 5 — the deferred decision this phase resolves. It records the
  constraint: *"the false-positive risk on `/clear`, `/limits reset` and
  shebangs argues for a narrower rule rather than the obvious one."*

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read this entire phase doc before touching any file.
3. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

`extract_path_literals` (`src/config/path_audit.rs:197`) walks the text
character by character looking for **backtick-delimited spans**, and keeps a
span only if it starts with one of the ten `PATH_PREFIXES`
(`src/config/path_audit.rs`, `~/.daemoneye/`, `etc/`, `var/`, `bin/`, `memory/`,
`runbooks/`, `scripts/`, `prompts/`, `agents/`, `sessions/`).

It is the single extraction entry point — `classify_text` (line 286) is its only
caller, and `daemoneye audit-prompts` is the only consumer.

Content inside a ` ``` ` fence is **entirely invisible** to it: fenced paths are
bare text, not backticked, and the scanner discards any span that meets a
newline (the `on_line` flag). The installed assets contain such content today —
`agent-runtime-layout.md` (a directory tree) and `webhook-setup.md`.

**The audit currently exits 0 on a clean tree.** That must remain true; a gate
that fires on a correct tree gets disabled.

### The design constraint, and the rule that satisfies it

The architect prototyped both candidate rules against the real assets. The
results decide the spec:

**The naive rule — keep every prefix-matching token inside a fence — is wrong.**
It extracts 11 tokens and produces **4 false `Unknown` findings**, which would
make `audit-prompts` exit 1 on a clean tree:

```
agent-runtime-layout.md: 'prompts/'  -> prompts    UNKNOWN
agent-runtime-layout.md: 'var/'      -> var        UNKNOWN
agent-runtime-layout.md: 'sessions/' -> sessions   UNKNOWN  (x2)
```

None of those is real drift. `agent-runtime-layout.md`'s tree is
**indentation-relative**, so a bare child name loses its parent:

```
  etc/
    prompts/          <- this means etc/prompts/, not a top-level prompts/
  var/
    sessions/         <- this means var/sessions/
```

**The narrow rule is: inside a fence, keep a token only if its *normalised* form
contains a `/`** — i.e. only multi-segment paths. Against the same assets that
yields **1 extraction and 0 false findings**, and the audit stays at exit 0.

It still catches the drift class this phase exists for. Given a fence containing
`sqlite3 ~/.daemoneye/var/index/memory.db` and `grep x var/lib/old.json`:

```
'~/.daemoneye/var/index/memory.db' -> var/index/memory.db  UNKNOWN
'var/lib/old.json'                 -> var/lib/old.json     UNKNOWN
```

That is exactly the phantom-`memory.db` case that slipped through M6.

**The shebang and slash-command risk is already handled by the prefix anchor**,
and this was verified rather than assumed. A fence containing
`#!/usr/bin/env python3`, `/clear`, `/limits reset` and `ls /bin/sh` yields
**no** extractions, because `starts_with` is anchored: `"#!/usr/bin/env"` does
not start with `bin/`, and `"/bin/sh"` does not either (the prefix is `bin/`,
without a leading slash). Bare top-level names like `etc/` are skipped anyway
under the multi-segment rule.

### Line-by-line processing is behaviour-preserving

The rewrite processes text line by line. That does **not** change the non-fence
path: the current scanner already discards any backtick span containing a
newline (it sets `on_line = false` and drops the span), so a per-line scan is
equivalent. The ~30 literals pinned by `extracts_real_path_spans`
(`src/config/path_audit.rs:375`) must all still be extracted.

**No existing test fixture in this file contains a ` ``` ` fence**, so no
existing test changes behaviour.

## Spec

### 1. Make `extract_path_literals` fence-aware

Rewrite `extract_path_literals` in `src/config/path_audit.rs` to iterate over
`text.lines()` with a `in_fence: bool` state:

- A line whose **trimmed-start** form begins with ` ``` ` toggles `in_fence` and
  contributes nothing itself. (This handles both bare ` ``` ` and tagged
  ` ```bash ` openers, and indented fences.)
- When `in_fence` is **false**: run the existing backtick-span logic on that
  line, unchanged — keep a span iff it starts with a `PATH_PREFIXES` entry after
  trimming.
- When `in_fence` is **true**: split the line on whitespace. For each token,
  trim this exact set of characters from **both ends**:

  ```
  `'",;:()[]{}<>│├└─|*
  ```

  Keep the token only if **both** hold:
  1. it starts with one of `PATH_PREFIXES`, and
  2. `normalise(token)` returns `Some(n)` where `n.contains('/')`.

Condition 2 is the multi-segment rule. `normalise` is already in this module
(line 238) and is pure, so calling it from the extractor is fine.

### 2. Tests — positive cases

Add to the existing `#[cfg(test)]` module in `src/config/path_audit.rs`:

- `fenced_block_yields_multi_segment_paths` — a fence containing
  `sqlite3 ~/.daemoneye/var/index/memory.db` extracts
  `~/.daemoneye/var/index/memory.db`.
- `fenced_block_yields_bare_relative_path` — a fence containing
  `grep x var/lib/old.json` extracts `var/lib/old.json`.
- `fenced_token_strips_surrounding_punctuation` — a fence containing
  `(var/log/daemon.log)` extracts `var/log/daemon.log`.
- `inline_backticks_still_extracted_outside_fences` — text with an inline
  `` `etc/config.toml` `` outside any fence still extracts it, proving the
  non-fence path is unchanged.

### 3. Tests — negative cases (these are the point of the phase)

Each of these must extract **nothing**. Pin them individually so a failure names
the case:

- `fenced_shebang_is_not_a_path` — a fence containing `#!/usr/bin/env python3`
  and `#!/bin/bash`.
- `fenced_slash_command_is_not_a_path` — a fence containing `/clear` and
  `/limits reset`.
- `fenced_absolute_system_path_is_not_a_path` — a fence containing `ls /bin/sh`.
- `fenced_bare_top_level_dir_is_skipped` — a fence containing `etc/`, `var/` and
  `prompts/` on their own extracts nothing, because each normalises to a
  single segment. Add a comment naming why: an indented tree's child names are
  relative to their parent, so a bare `prompts/` would be read as a top-level
  directory that does not exist.
- `fenced_url_is_not_a_path` — a fence containing
  `https://example.com/var/x` extracts nothing.

### 4. Regression test — the real assets stay clean

Add `seeded_assets_have_no_unknown_fenced_paths`: seed a temp `HOME` with
`crate::config::Config::ensure_dirs()`, read each `*.md` under
`memory/knowledge/` plus `etc/prompts/sre.toml`, run `classify_text` over each,
and assert **no** result is `PathClassification::Unknown`.

Use `crate::test_home_guard()` — it restores `HOME` on drop, so no manual
restore block is needed. The RAII pattern is at
`src/cli/commands/audit_prompts.rs:206` (`setup_test_home`); do the same shape.

This test is what fails if someone later widens the rule back to the naive form.

## Acceptance criteria

- [ ] `daemoneye audit-prompts` still exits **0** on a freshly seeded tree.
- [ ] `daemoneye audit-prompts` exits **1** and names the offending path when a
      fenced block containing `~/.daemoneye/var/index/memory.db` is appended to a
      seeded knowledge memory.
- [ ] All tests named in spec tasks 2–4 pass.
- [ ] `extracts_real_path_spans` still passes unchanged — no literal removed
      from its `must_extract` list.
- [ ] `cargo build` zero new warnings; `cargo clippy --all-targets
      --all-features -- -D warnings` exits 0; `cargo fmt --all` leaves the tree
      unchanged.
- [ ] `cargo test` passes. Lib count rises by the number of tests added (9 by
      this spec); integration stays **30** (2 ignored), isolation **8**
      (1 ignored), `bug_tracker` **6**.
- [ ] Only `src/config/path_audit.rs` changes — `git diff --name-only` lists no
      other `.rs` file.

## Test plan

Covered by spec tasks 2–4. The load-bearing ones are the **negative** tests in
task 3 — they encode the constraint that kept this work out of M6 — and
`seeded_assets_have_no_unknown_fenced_paths` in task 4, which is the guard
against a future widening that would fabricate findings.

**What would make this phase a false success:** a rule that extracts nothing at
all from fenced blocks would pass every negative test and the seeded-assets
test. The positive tests in task 2 exist to prevent that, and the second
acceptance criterion proves it end-to-end against the real binary.

## End-to-end verification

The real artifact is the `daemoneye audit-prompts` CLI. Run this block verbatim
and paste the resulting file's contents into your Update Log entry.

**Note two deliberate constraints on this block, both from phase-03's
post-mortem:** it contains **no heredocs**, and every tree-walking command is
wrapped in `timeout`. A phase-03 E2E block nested a `python3` heredoc that hung
and orphaned two processes at 100% CPU for 70 minutes. Do not reintroduce
either pattern here.

```bash
cd /home/matt/src/daemoneye
cargo build 2>&1 | tail -2
H=$(mktemp -d)
{
  echo "=== clean seeded tree: audit must exit 0 ==="
  HOME="$H" timeout 60 ./target/debug/daemoneye setup 2>&1 | tail -3
  HOME="$H" timeout 60 ./target/debug/daemoneye audit-prompts > /dev/null 2>&1
  echo "clean-audit-exit=$?   # 0 == PASS"

  echo "=== inject a fenced phantom path into a seeded knowledge memory ==="
  printf '\n```\nsqlite3 ~/.daemoneye/var/index/memory.db "select 1"\n```\n' \
    >> "$H/.daemoneye/memory/knowledge/agent-runtime-layout.md"
  tail -4 "$H/.daemoneye/memory/knowledge/agent-runtime-layout.md"
  echo "exit=$?"

  echo "=== audit must now exit 1 and name the path ==="
  HOME="$H" timeout 60 ./target/debug/daemoneye audit-prompts 2>&1 | grep -i "memory.db"
  echo "grep-exit=$?   # 0 == the path was reported == PASS"
  HOME="$H" timeout 60 ./target/debug/daemoneye audit-prompts > /dev/null 2>&1
  echo "dirty-audit-exit=$?   # 1 == PASS"

  echo "=== the new tests ==="
  timeout 300 cargo test --lib path_audit 2>&1 | grep -E "^test |^test result"
  echo "exit=$?"

  echo "=== full gate ==="
  timeout 600 cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -3
  echo "clippy-exit=$?"
  timeout 600 cargo test 2>&1 | grep -E "^test result"
  echo "exit=$?"
} > /tmp/phase04-e2e.txt 2>&1
rm -rf "$H"
cat /tmp/phase04-e2e.txt
```

`clean-audit-exit=0` and `dirty-audit-exit=1` together are the proof: the gate
is quiet on a correct tree and loud on a stale one.

Paste the captured file into an Update Log entry titled
`### Update — <date> (end-to-end verification)`. **The server-authored
`(complete)` entry does not satisfy this** — its "Command output tails" block is
the automatic gate capture every phase receives, and it shows that
build/lint/test ran, not that this phase's acceptance criteria were exercised.

**If any part of the capture block fails or hangs, stop and report it as a
blocker.** Do not re-run the surviving sections separately and paste the
result — a transcript assembled from more than one run fails `STANDARDS.md` §1
even when every claim in it is true.

## Authorizations

- [ ] May add dependencies: **none**. All parsing is `std` string work.
- [ ] May touch `docs/architecture.md`: no.
- [ ] May create new files: no — everything lands in `src/config/path_audit.rs`.

## Out of scope

- **Adding `var` to `INVENTORY`.** It is a genuine gap — bare `var` is not an
  inventory entry — but under the multi-segment rule nothing ever normalises to
  it, so the gate is unaffected. Fixing the inventory is a separate concern.
- **Adding `prompts` or `sessions` to `INVENTORY`.** These would be **wrong**:
  no such top-level directories exist. They appear only as indentation-relative
  children (`etc/prompts`, `var/sessions`), both already inventoried.
- **Reconstructing full paths from tree indentation.** It would let the gate
  read `agent-runtime-layout.md`'s tree properly, but it is a much larger piece
  of work and phase 05 removes the hand-maintained tree entirely.
- **Widening `PATH_PREFIXES`.** The prefix anchor is what makes shebangs and
  slash commands safe. Do not touch that list.
- **Changing `classify_text`, `normalise`, or the `INVENTORY` table.**
- **Any file other than `src/config/path_audit.rs`.**

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->
