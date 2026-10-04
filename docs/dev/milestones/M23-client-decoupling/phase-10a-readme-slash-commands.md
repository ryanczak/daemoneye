# Phase 10a: readme-slash-commands — the README documents every chat slash command, and a doc_truth test keeps it that way

**Milestone:** M23 — Client decoupling & shell UX
**Status:** review
**Depends on:** 09b (done)
**Estimated diff:** ~42 lines of README.md; 1 new test plus a helper (~65 lines) in `tests/doc_truth.rs`; no production code
**Tags:** language=rust, kind=docs, size=s

## Goal

M23 added eleven chat slash commands. `/help` lists them, but README.md's
"In-chat slash commands" table does not. After this phase:

- **README.md documents the M23 chat surface:**
  - eleven rows in "### In-chat slash commands": `/shells`, `/hosts`,
    `/tail <id>` (alias `/watch`), `/logs`, `/log <id>`, `/attach <id>`,
    `/agents`, `/shell [host]`, `/runs`, `/run <id>`, `/stop <id>`;
  - two new key tables after "Transcript viewer keys": **"### Shell viewer
    keys"** (`/tail`, `/log`) and **"### Attached mode keys"** (`Ctrl+P`,
    `Ctrl+C`, `Ctrl+D`, `Ctrl+D Ctrl+D`), plus what is recorded when you type
    while attached;
  - the tmux requirement row says tmux is optional with
    `tmux_session = ""`.
- **A doc_truth test fails the build when a command is missing:**
  `readme_documents_every_chat_slash_command` (`tests/doc_truth.rs`) collects
  every command chat knows, from two places:
  - the `/word` tokens on `HELP_TEXT`'s command and `Aliases:` lines
    (`src/cli/commands/chat.rs`);
  - the `"/word"` arms of `handle_slash` (`src/cli/commands/slash.rs`).

  It then requires each one to appear backticked in that README section.

This phase is row 10's documentation half. Row 10b (the live close) remains.

## Architecture references

- `docs/dev/milestones/M23-client-decoupling/README.md`: row 10.
- PE rule (2026-09-03): README.md is updated at every 2.0 milestone close and
  on any user-visible change. Until now, `doc_truth` guarded only the tool
  tables.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read this entire phase doc before touching any code.
3. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

Measured on **2026-10-04** at commit `23eb21bf`. Baseline:
`cargo test --lib` gives **2219 passed / 0 failed / 14 ignored**, and
`cargo test --test doc_truth` gives **14 passed**.

**The whole change below was dry-run in this tree, then reverted.** Results
on the dry run:
- the full suite gave lib **2219**/0/14 (unchanged) and doc_truth **15**;
  every other target was unmoved (bug_tracker 6, integration 32,
  isolation 11);
- fmt and clippy `-D warnings` were clean;
- **the § End-to-end verification block itself was run on the dry-run tree**
  and printed exactly the Expected values below. No capture line ends in
  whitespace. On the current tree it prints `doc_truth=14p-0f`,
  `phase_test=0p-0f` and `wired=0/0/0`;
- Mutations A–C each went red on 3 of 3 runs, **and each mutant passes
  clippy `-D warnings` with zero diagnostics**.

### Facts measured for this phase

- **Where the commands are.**
  - `HELP_TEXT` is a `const &str` in `src/cli/commands/chat.rs`. It begins
    `const HELP_TEXT: &str = "` and ends at a line holding only `";`. Command
    lines begin (after indentation) with `/`. Aliases sit on a line
    beginning `Aliases:`.
  - `handle_slash` in `src/cli/commands/slash.rs` dispatches with
    `match cmd {` and falls through at
    `_ => return SlashOutcome::NotACommand`. Every arm between those two
    names its commands as quoted `"/word"` literals.
- **A command starts a word.** Help lines also carry argument lists such as
  `list/on/off` and keys such as `n/p`. A plain `/[a-z]+` scan captures
  `/on`, `/off` and `/p`. Measured: the first dry-run regex did exactly
  that. The pinned regex `(?:^|[\s(])(/[a-z]+)` only takes a `/` at the
  start of a line or after whitespace or `(`.
- **The scan finds 28 commands today** (aliases included). The test asserts
  at least 20. That
  bound catches a scan that silently reads nothing, and Mutation C is red
  through it.
- **The README section** runs from `### In-chat slash commands` to the next
  `\n### ` heading ("### Transcript viewer keys"). A command counts as
  documented if the section contains `` `/cmd` `` or `` `/cmd `` (a
  backtick, the command, a space), so `` `/tail <id>` `` documents `/tail`.
  It does **not** document `/ta`, because the next character must be a
  backtick or a space.
- `regex` is already a dev-dependency, and `tests/doc_truth.rs` already
  imports `std::path::Path`. No `Cargo.toml` change is needed.
- **Known flakes:**
  `shell::pty::pause_resume_tests::pty_stop_with_no_foreground_job_remembers_nothing`
  (frequent) and
  `shell::backend::tests::input_after_a_prompt_logs_a_redaction_marker`.
  Both are pre-existing and in files this phase does not touch.
- **Gotcha: no process killing.** Do not run `kill`, `pkill`,
  `tmux kill-*`, `daemoneye chat` or `daemoneye daemon`.

## Spec

Make **exactly** the changes the diffs below show. They are the dry run,
formatted and clippy-clean, tests included. Apply them file by file with
your edit tools. Copy; do not retype from memory. README table cells contain
`\|` (an escaped pipe inside a cell). Keep the backslash.

### Task 1 — README.md

```diff
--- a/README.md
+++ b/README.md
@@ -62,7 +62,7 @@ Then `tmux source-file ~/.tmux.conf`. The bind-key uses the full path so it work
 |---|---|
 | Linux | `fork(2)`, Unix domain sockets, Linux-specific tmux hooks |
 | Rust 1.79+ | Required by Rust edition 2024 |
-| tmux 2.6+ | Required for hook support (`pane-focus-in`, `client-attached`, `after-new-session`) |
+| tmux 2.6+ | Required for hook support (`pane-focus-in`, `client-attached`, `after-new-session`). Optional with `tmux_session = ""`: the daemon then starts no tmux, and `daemoneye chat` runs in any terminal |
 
 ```sh
 sudo apt install tmux      # Debian/Ubuntu
@@ -453,6 +453,17 @@ Both `reindex` and `audit-prompts` run without a daemon. `reindex` is safe to ru
 | `/approvals` | Inspect approval state; `on`/`off` toggle commands, `revoke` gates everything (alias: `/approval`) |
 | `/limits` | Show active limits and live session counters; `/limits reset` |
 | `/session` | `save`/`load`/`list`/`delete`/`rename`/`diff`/`tag` (alias: `/sessions`) |
+| `/shells` | The PTY shells the daemon runs: id, host, owner (`this session`, `user`, `ghost:…`), live or exited, age and last command |
+| `/hosts` | `local`, then each configured `[hosts.<name>]` with its ssh destination, ControlMaster state (`master:up`/`down`) and live-shell count |
+| `/tail <id>` | A live, read-only view of one shell in the alternate screen, with follow mode, scrollback and search (alias: `/watch`) |
+| `/logs` | Past shell recordings, newest first: `/logs [host] [--owner chat\|ghost\|adopted] [--since 2h]` |
+| `/log <id>` | Open one recording by the log id `/logs` prints: `n`/`p` step through its commands, `space` replays it at its recorded speed |
+| `/attach <id>` | Type into a live shell, full screen: every key goes to the shell except the chords in [Attached mode keys](#attached-mode-keys). A bare `/attach` opens the picker |
+| `/agents` | Pick a live shell to attach to from a list in the chat viewport, ghost-owned shells first: `↑`/`↓` select, `Enter` attaches, `Esc` closes |
+| `/shell [host]` | Open a shell you own, on the daemon host or a configured host, and attach to it, for example to type a password once before an agent needs the host. Agents see it in their shell list but can drive it only through an approval that is always asked |
+| `/runs` | Recent runs (chat turns and ghost runs) with their state |
+| `/run <id>` | The last records of one run's turn log |
+| `/stop <id>` | Ask a running run to stop (it ends at its next turn boundary) |
 
 At a tool-approval prompt, typing a message instead of `Y`/`A`/`N` redirects the agent. Up/Down navigate the input; at the top or bottom edge they recall history.
 
@@ -490,6 +501,36 @@ Active only while the alternate-screen viewer is open (`ctrl+o`).
 
 While the search prompt is open, typing edits the query, `Enter` commits it and `Esc` cancels it.
 
+### Shell viewer keys
+
+Active in `/tail` and `/log`, which share the alternate-screen shell viewer.
+
+| Key | Effect |
+|---|---|
+| `↑` / `↓`, `PgUp` / `PgDn`, `Home` / `End` | Scroll; scrolling up in `/tail` pauses follow |
+| `f` | `/tail`: follow the newest output again |
+| `/`, then `n` / `N` | `/tail`: search, then the older / newer match |
+| `n` / `p` | `/log`: put the next / previous recorded command at the top, with its exit code in the status line |
+| `space` | `/log`: replay the recording at its recorded speed (pauses longer than 2 s replay as 2 s); `space` again pauses and resumes, any other key skips to the end |
+| `Esc` / `q` / `Ctrl+C` / `ctrl+o` | Close the viewer |
+
+### Attached mode keys
+
+Active while attached (`/attach`, `/agents`, `/shell`). Every other key goes
+to the shell.
+
+| Key | Effect |
+|---|---|
+| `Ctrl+P` | Pause the shell's foreground job; press again to resume. A ghost run that owns the shell waits at its next turn and is told how long it was paused |
+| `Ctrl+C` | Interrupt the foreground job, and cancel the run that owns the shell (it ends at its next turn boundary) |
+| `Ctrl+D` | Detach back to chat. Detaching while paused resumes the job first |
+| `Ctrl+D` `Ctrl+D` | Send one `Ctrl+D` (EOF) to the shell |
+
+What you type while attached is recorded in the shell's own log (scrubbed
+at password prompts), and as a byte count, never the bytes, in
+`events.jsonl`. The agent that owns the shell is told at its next turn that
+you typed into it.
+
 ---
 
 ## AI tools
```

### Task 2 — the guard (`tests/doc_truth.rs`)

```diff
--- a/tests/doc_truth.rs
+++ b/tests/doc_truth.rs
@@ -939,3 +939,68 @@ fn e2e_prompt_asset_tool_hygiene() {
         "capture drifted from the pinned line"
     );
 }
+
+/// The slash commands chat knows: every `/word` in `HELP_TEXT`
+/// (`src/cli/commands/chat.rs`) and every `"/word"` arm of `handle_slash`
+/// (`src/cli/commands/slash.rs`), aliases included.
+fn chat_slash_commands(root: &Path) -> std::collections::BTreeSet<String> {
+    // A command starts a word: `list/on/off` and `n/p` in the help text are
+    // argument lists and keys, not commands.
+    let word = regex::Regex::new(r"(?:^|[\s(])(/[a-z]+)").expect("valid regex");
+    let mut commands = std::collections::BTreeSet::new();
+
+    let chat = std::fs::read_to_string(root.join("src/cli/commands/chat.rs"))
+        .expect("reading src/cli/commands/chat.rs");
+    let start = chat
+        .find("const HELP_TEXT: &str = \"")
+        .expect("HELP_TEXT is defined in chat.rs");
+    let help = &chat[start..];
+    let help = &help[..help.find("\n\";").expect("HELP_TEXT ends")];
+    for line in help.lines() {
+        let line = line.trim_start();
+        if line.starts_with('/') || line.starts_with("Aliases:") {
+            commands.extend(word.captures_iter(line).map(|c| c[1].to_string()));
+        }
+    }
+
+    let slash = std::fs::read_to_string(root.join("src/cli/commands/slash.rs"))
+        .expect("reading src/cli/commands/slash.rs");
+    let start = slash
+        .find("match cmd {")
+        .expect("handle_slash matches on cmd");
+    let arms = &slash[start..];
+    let arms = &arms[..arms
+        .find("_ => return SlashOutcome::NotACommand")
+        .expect("handle_slash's fallback arm")];
+    let quoted = regex::Regex::new(r#""(/[a-z]+)""#).expect("valid regex");
+    commands.extend(quoted.captures_iter(arms).map(|c| c[1].to_string()));
+    commands
+}
+
+/// README.md's "In-chat slash commands" section: every command chat knows is
+/// documented there, backticked (`` `/log` `` or `` `/log <id>` ``). The M23
+/// shell commands sat undocumented until the milestone close — this keeps the
+/// table from falling behind `/help` again.
+#[test]
+fn readme_documents_every_chat_slash_command() {
+    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
+    let readme = std::fs::read_to_string(root.join("README.md")).expect("reading README.md");
+    let start = readme
+        .find("### In-chat slash commands")
+        .expect("README has an In-chat slash commands section");
+    let section = &readme[start..];
+    let section = &section[..section[4..].find("\n### ").map_or(section.len(), |i| i + 4)];
+    let commands = chat_slash_commands(root);
+    assert!(
+        commands.len() >= 20,
+        "the command scan found too few commands to be reading the sources: {commands:?}"
+    );
+    let missing: Vec<&String> = commands
+        .iter()
+        .filter(|c| !section.contains(&format!("`{c}`")) && !section.contains(&format!("`{c} ")))
+        .collect();
+    assert!(
+        missing.is_empty(),
+        "README.md's In-chat slash commands section does not document: {missing:?}"
+    );
+}
```

`cargo test --test doc_truth` must give **15 passed** after Tasks 1–2.

### Task 3 — apply and record Mutations A–C (before the capture)

Do this **after** Tasks 1–2 pass and **before** Task 4. A mutation does not
affect the capture: each one is restored, and `git diff --stat` is checked,
before the next step. Follow § Mutations: for each one, apply it with the
`patch` tool, run the test once, restore it, and confirm `git diff --stat`
matches. Then create an Update Log entry titled exactly
`### Update — <date> (mutations)` with one line per mutation: its letter,
the test, and the observed result (`red` or `green`).

### Task 4 — capture the end-to-end evidence

Run the block in § End-to-end verification verbatim. Then create a new Update
Log entry titled exactly `### Update — <date> (end-to-end verification)`.
**Its first fenced block is the contents of `/tmp/e2e-m23-10a.txt`**: the
file's lines, copied by a tool, **unchanged**. No line ends in whitespace.
**Never paste the script itself**, never retype the lines, and do not also
paste them unfenced. Then run the paste check once and put the line **it
prints** into the entry on its own line. If it prints `PASTE MISMATCH`,
write that line, not another.

## Acceptance criteria

Parenthesised values are what each line produces against the current tree.
Every non-preservation line fails today. Each line was run on the current
tree **and** on the dry-run tree.

- [ ] `cargo test --lib` → **2219 passed / 0 failed / 14 ignored** on three
      consecutive runs (*preservation*: no production code changes). A
      failure of a known flake named below is re-run, not counted.
- [ ] `cargo fmt --all` clean; `cargo build` clean;
      `cargo clippy --all-targets --all-features -- -D warnings` exits 0.
- [ ] `cargo test --test doc_truth` → **15 passed / 0 failed** (current: 14).
- [ ] `cargo test` → other targets unmoved: bug_tracker 6, integration 32,
      isolation 11 (*preservation*).
- [ ] The phase's test passes:
      ``cargo test --test doc_truth -- readme_documents_every_chat_slash_command``
      → **1 passed** (current: 0, the test does not exist).
- [ ] Wired, as the end-to-end block's `wired=` line prints it → **`1/1/1`**
      (current: `0/0/0`):
      ``grep -c '^| `/shells` |' README.md`` → 1;
      ``grep -c '^### Attached mode keys$' README.md`` → 1;
      ``grep -c '^### Shell viewer keys$' README.md`` → 1.
- [ ] The mutations entry exists and records all three:
      ``grep -c '^### Update — .* (mutations)$' <this doc>`` → **1**
      (current: 0).
- [ ] The end-to-end entry exists:
      ``grep -c '^### Update — .* (end-to-end verification)$' <this doc>`` →
      **1** (current: 0). Its first fenced block is the capture file's
      contents, unchanged, and the paste check, run as written below,
      prints `PASTE MATCH`, which is on its own line:
      ``grep -cx 'PASTE MATCH' <this doc>`` → **1** (current: 0).

## Mutations

Apply each with the `patch` tool (the executor contract forbids `sed -i`,
`perl -i`, `> file` and `rm -rf`). Run
`cargo test --test doc_truth -- readme_documents_every_chat_slash_command`
once, restore, and record the observed result in the Task 3 entry. Each was
measured red at drafting on 3 of 3 runs, and each mutant builds
clippy-clean.

- **Mutation A** (README, a missing row is caught): delete the README line
  that begins ``| `/runs` |``. Expected **red**, naming `/runs`.
- **Mutation B** (README, a missing aliased row is caught): delete the README
  line that begins ``| `/tail <id>` |``. Expected **red**, naming `/tail`
  and `/watch`.
- **Mutation C** (`tests/doc_truth.rs`, a scan that reads nothing is caught):
  change `r"(?:^|[\s(])(/[a-z]+)"` to `r"()(/[a-z]+)"`. The help scan now
  captures only empty strings. Expected **red** on the `commands.len() >= 20`
  assertion or on the missing list.

After each restore, confirm `git diff --stat` matches its pre-mutation state.

## Test plan

One new integration test, `readme_documents_every_chat_slash_command`, with
its helper `chat_slash_commands`. It:
- reads the commands from the help text and the dispatcher, so a command
  added to either without a README row fails the build;
- fails when the scan finds fewer than 20 commands, so a broken scan cannot
  pass vacuously;
- accepts `` `/cmd` `` or `` `/cmd ...` `` and nothing looser.

## End-to-end verification

```sh
cd /home/matt/src/daemoneye
{
  for i in 1 2 3; do
    out=$(cargo test --lib 2>&1)
    verdict=$(printf '%s\n' "$out" | grep -E '^test result' | tail -1 | grep -oE 'ok|FAILED')
    failed=$(printf '%s\n' "$out" | grep -E '^test .* FAILED$' | sed -E 's/^test (.*) \.\.\. FAILED$/\1/' | paste -sd, -)
    printf 'lib%d=%s failed:%s\n' "$i" "$verdict" "${failed:-none}"
  done
  printf 'lib_counts=%s\n' "$(cargo test --lib 2>&1 | grep -E '^test result' | tail -1 | sed -E 's/.*ok\. ([0-9]+) passed; ([0-9]+) failed; ([0-9]+) ignored.*/\1p-\2f-\3i/')"
  printf 'doc_truth=%s\n' "$(cargo test --test doc_truth 2>&1 | grep -E '^test result' | tail -1 | sed -E 's/.*ok\. ([0-9]+) passed; ([0-9]+) failed.*/\1p-\2f/')"
  printf 'phase_test=%s\n' "$(cargo test --test doc_truth -- readme_documents_every_chat_slash_command 2>&1 | grep -E '^test result' | tail -1 | sed -E 's/.*ok\. ([0-9]+) passed; ([0-9]+) failed.*/\1p-\2f/')"
  printf 'wired=%s/%s/%s\n' "$(grep -c '^| `/shells` |' README.md)" "$(grep -c '^### Attached mode keys$' README.md)" "$(grep -c '^### Shell viewer keys$' README.md)"
} > /tmp/e2e-m23-10a.txt
cat /tmp/e2e-m23-10a.txt
```

Expected:
- `lib1=ok failed:none`, `lib2=ok failed:none`, `lib3=ok failed:none`;
- `lib_counts=2219p-0f-14i`;
- `doc_truth=15p-0f`;
- `phase_test=1p-0f`;
- `wired=1/1/1`.

**Re-run budget: three** (WORKFLOW.md § End-to-end verification). **Two
pre-existing lib flakes are known:**
`shell::backend::tests::input_after_a_prompt_logs_a_redaction_marker` and
`shell::pty::pause_resume_tests::pty_stop_with_no_foreground_job_remembers_nothing`.
The second is frequent. Neither is in a file this phase touches. If either
fails, name it and re-run the whole block. After three re-runs, stop, paste
the last capture as it is, and name every failing test. Any other failing
test is a finding: report it by name.

**Run this block with nothing else running `cargo test`.** Then make the
entry's first fenced block the **contents of** `/tmp/e2e-m23-10a.txt`
(copied, not retyped; **never the script above**), and run:

```sh
D=docs/dev/milestones/M23-client-decoupling/phase-10a-readme-slash-commands.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-m23-10a.txt
diff /tmp/pasted-m23-10a.txt /tmp/e2e-m23-10a.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

If it prints `PASTE MISMATCH`, run
`diff /tmp/pasted-m23-10a.txt /tmp/e2e-m23-10a.txt` once and fix what it
shows. **Run the check at most three times.** Put the line it prints into the
entry on its own line, exactly as printed. Never write a verdict this check
did not print.

## Authorizations

You may edit these files, and nothing else:

- `README.md`, `tests/doc_truth.rs`: the diffs above.
- This phase doc's Update Log.

**Do not run `kill`, `pkill`, `tmux kill-*`, `daemoneye chat` or
`daemoneye daemon`.** Every test here reads files.

## Out of scope

- **`/help` text and slash-command behaviour.** No production code changes.
- **CLAUDE.md.** It documents tools, not slash commands.
- **The live close (row 10b).** It is architect-run with the PE at the
  keyboard.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-10-04 22:07 (started, rexyMCP-executor)

Executor dispatched headless for phase 10a. Flipped status to
`in-progress` (both the phase doc and the milestone README's row). Now
applying Task 1 (README.md) and Task 2 (`tests/doc_truth.rs`) exactly as the
diffs show, then the mutations entry and the end-to-end capture.

### Update — 2026-10-04 22:09 (mutations)

- **A** — deleted the `` | `/runs` | `` README row;
  `readme_documents_every_chat_slash_command` → **red** (naming `/runs`).
- **B** — deleted the `` | `/tail <id>` | `` README row;
  `readme_documents_every_chat_slash_command` → **red** (naming `/tail` and
  `/watch`).
- **C** — `r"(?:^|[\s(])(/[a-z]+)"` → `r"()(/[a-z]+)"` in `chat_slash_commands`;
  `readme_documents_every_chat_slash_command` → **red** on the
  `commands.len() >= 20` assertion.
- Each mutation was restored with the `patch` tool and `git diff --stat`
  confirmed the pre-mutation state (README.md +43, tests/doc_truth.rs +65)
  before the next step.

### Update — 2026-10-04 22:18 (end-to-end verification)

Ran the § End-to-end verification block verbatim into
`/tmp/e2e-m23-10a.txt`. The block ran once clean; the first run's `lib2`
failed the pre-existing flake
`daemon::utils::event_log::tests::for_each_streams_across_segments_in_order`
(not one of the two known flakes; it passed 5/5 in isolation, see Notes), so
the block was re-run in full per the re-run budget and the capture below is
that second, clean run.

```
lib1=ok failed:none
lib2=ok failed:none
lib3=ok failed:none
lib_counts=2219p-0f-14i
doc_truth=15p-0f
phase_test=1p-0f
wired=1/1/1
```

PASTE MATCH

Notes:

- **Unexpected flake (first block run).** `lib2` of run 1 failed
  `daemon::utils::event_log::tests::for_each_streams_across_segments_in_order`
  — *not* one of the two known flakes the phase names. Re-run in isolation
  5/5 green, and run 2's `lib2` was green; the capture above is run 2. Any
  other target was unmoved (bug_tracker 6, integration 32, isolation 11 on
  the last full `cargo test`).
- **Scan sanity.** The guard's `commands.len() >= 20` floor passes with 28
  commands found (aliases included), matching the phase doc's measurement;
  Mutation C showed the floor is what catches a dead scan.
- The phase doc's pinned literals are present: `grep -c '^| `/shells` |'
  README.md` → 1, `'^### Attached mode keys$'` → 1,
  `'^### Shell viewer keys$'` → 1 (the block's `wired=1/1/1` line).
- Re-run budget used: 1 of 3. Paste check run once, printed `PASTE MATCH`.

### Update — 2026-10-04 22:22 (complete, server-authored)

**Summary:** </think>The working tree is clean, all committed. Status is left `in-progress`, no `(complete)` entry written. All acceptance criteria:

- cargo test --lib → 2219/0/14 ✓ (three consecutive runs in the capture)
- fmt clean, build clean, clippy -D warnings exit 0 ✓
- doc_truth → 15 passed ✓
- other targets unmoved: bug_tracker 6, integration 32, isolation 11 ✓ (verified in full cargo test)
- phase test → 1 passed ✓
- wired=1/1/1 ✓
- mutations entry exists ✓ (1 match)
- e2e entry exists ✓ (1 match), first fenced block = capture contents, PASTE MATCH ✓ (1 exact-line match)

Now the final Summary + Notes for review, no tool call.</think>
**Summary + Notes for review**

Phase 10a is implemented and committed (`017283f8`): README.md now documents all eleven M23 chat slash commands (`/shells` … `/stop`) in the "In-chat slash commands" table, gains the "Shell viewer keys" and "Attached mode keys" tables with the typed-while-attached recording note, and the tmux requirement row now says tmux is optional with `tmux_session = ""` — all exactly the diff the phase doc pins, copied verbatim. Task 2 added `readme_documents_every_chat_slash_command` + `chat_slash_commands` to `tests/doc_truth.rs` verbatim from the spec; it finds 28 commands (aliases included) and `cargo test --test doc_truth` went 14 → **15 passed**.

End-to-end verification ran the pinned block verbatim into `/tmp/e2e-m23-10a.txt`: `lib1/lib2/lib3=ok failed:none`, `lib_counts=2219p-0f-14i`, `doc_truth=15p-0f`, `phase_test=1p-0f`, `wired=1/1/1`; the paste check printed **PASTE MATCH**. Mutations A–C each went red as expected (A named `/runs`, B named `/tail` + `/watch`, C tripped the `>= 20` floor) and each restore was confirmed with `git diff --stat`; `cargo fmt --all`, `cargo build`, and `clippy --all-targets --all-features -- -D warnings` are clean, and the full `cargo test` left every other target unmoved (lib 2219, bug_tracker 6, integration 32, isolation 11). Status stays `in-progress` with no `(complete)` entry, per the contract; the tree is clean.

**Notes for review:** (1) Run 1 of the e2e block hit an unexpected lib failure in `daemon::utils::event_log::tests::for_each_streams_across_segments_in_order` — not one of the two flakes the phase names; it passed 5/5 in isolation and was green in run 2, so I re-ran the block in full (1 of 3 re-runs used) and the capture above is run 2. Worth adding to the known-flake list if it recurs. (2) No deviations from the spec's diffs; no production code was touched. (3) The phase doc's Update Log has the started, mutations, and end-to-end entries; the mutations/e2e entry counts and the `PASTE MATCH` literal all grep to their pinned values.

**Executor:** local-inference-lab/GLM-5.3-Flash-NVFP4-Spark

**Gates:** format=run, build=run, lint=run, test=run

**Command output tails:**

```
FORMAT


BUILD
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.06s


LINT
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.08s


TEST
bug_tracker_is_consistent ... ok

test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 15 tests
test approval_gated_tools_all_exist ... ok
test claude_md_tools_table_counts_are_accurate ... ok
test claude_md_tools_table_matches_the_code ... ok
test readme_tools_counts_are_accurate ... ok
test e2e_prompt_asset_tool_hygiene ... ok
test readme_tools_tables_match_the_code ... ok
test readme_approval_markers_match_the_gated_tools ... ok
test docs_document_the_reindex_command ... ok
test prompt_assets_name_no_retired_tools ... ok
test e2e_seeded_asset_tool_hygiene ... ok
test docs_do_not_carry_retired_index_claims ... ok
test seeded_assets_name_no_retired_tools ... ok
test seeded_config_template_has_no_phantom_keys ... ok
test seeded_config_template_documents_every_config_field ... ok
test readme_documents_every_chat_slash_command ... ok

test result: ok. 15 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 34 tests
test daemon_ping_status_loop ... ignored
test cancel_request_roundtrip ... ok
test g3_tool_policy_runbook_precedence_over_agent ... ok
test g1_spawn_ghost_shell_with_agent_merge ... ok
test g3_tool_policy_allow_merged_and_enforced ... ok
test g3_tool_policy_deny_merged_and_enforced ... ok
test g4_briefing_injection_block_format ... ok
test g5_depth_limit_enforced ... ok
test g5_child_inherits_depth_and_parent ... ok
test g6_tool_policy_enforced_in_ghost ... ok
test ipc_tool_call_response_round_trip ... ok
test ipc_session_info_round_trip ... ok
test ipc_ask_round_trip ... ok
test ghost_config_parsing ... ok
test minimal_config_parsing ... ok
test resume_run_request_roundtrip ... ok
test window_switch_does_not_corrupt_chat ... ignored
test schedule_store_persistence ... ok
test config_pricing_round_trip ... ok
test g4_briefing_masking_applied ... ok
test event_log_append_read ... ok
test event_log_entry_format ... ok
test cost_record_serializes_to_events_jsonl_round_trip ... ok
test g4_briefing_read_and_clear ... ok
test g4_briefing_injects_on_next_run ... ok
test g6_agent_config_roundtrip ... ok
test g6_agent_namespace_field_persisted ... ok
test session_index_persistence ... ok
test session_jsonl_round_trip ... ok
test webhook_alert_no_severity_passes_gate ... ok
test webhook_alert_to_event_log ... ok
test g5_mailbox_write_and_read ... ok
test webhook_alert_below_threshold_discarded ... ok
test webhook_alert_unrankable_severity_passes_gate ... ok

test result: ok. 32 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.04s


running 12 tests
test webhook_ghost_e2e_http ... ignored
test held_port_cannot_be_rebound ... ok
test webhook_ports_differ_between_environments ... ok
test stub_returns_canned_response_via_make_client ... ok
test webhook_ghost_records_its_ssh_target_resolution ... ok
test an_empty_session_starts_no_tmux_server ... ok
test config_contains_webhook_and_stub_url ... ok
test hooks_land_on_private_server ... ok
test daemon_boots_in_throwaway_root ... ok
test default_server_unchanged ... ok
test webhook_ghost_e2e_deterministic ... ok
test daemon_webhook_returns_200 ... ok

test result: ok. 11 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.17s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.07s
     Running unittests src/lib.rs (target/debug/deps/daemoneye-9810e08c3de1e001)
kill: sending signal to 849653 failed: No such process
     Running unittests src/main.rs (target/debug/deps/daemoneye-54ad57cf3704bc40)
     Running tests/bug_tracker.rs (target/debug/deps/bug_tracker-d709c9b862b5691d)
     Running tests/doc_truth.rs (target/debug/deps/doc_truth-34cb94d3012a3d69)
     Running tests/integration.rs (target/debug/deps/integration-deb70358b520d0d5)
     Running tests/isolation.rs (target/debug/deps/isolation-45facff22bac60f5)
   Doc-tests daemoneye

```

**Files changed:**

- `README.md` — +42 -1
- `docs/dev/milestones/M23-client-decoupling/README.md` — +1 -1
- `docs/dev/milestones/M23-client-decoupling/phase-10a-readme-slash-commands.md` — +60 -1
- `tests/doc_truth.rs` — +65 -0

**Commit:** 017283f8fb198f0d73c70f695eef6c73d8374545

**Notes:** server-authored completion entry (executor no longer owns the bookkeeping tail; see M27 phase-03).
