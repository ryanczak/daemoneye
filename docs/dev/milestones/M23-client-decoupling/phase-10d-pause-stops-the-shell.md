# Phase 10d: pause-stops-the-shell — `Ctrl-p` pauses the command, not just its current child process

**Milestone:** M23 — Client decoupling & shell UX
**Status:** todo
**Depends on:** 08a (done: the attach chords), 10c (done: a local ghost runs in a PTY shell)
**Estimated diff:** ~40 lines of production code in 1 file; 2 new tests and two adjusted tests (~110 lines)
**Tags:** language=rust, kind=bugfix, size=s

## Goal

Pausing a shell from attached mode (`Ctrl-p`, the `STOP` signal) stops only
the terminal's **foreground process group**: the command's current child
process. The interactive bash that runs the command keeps running. Bash
reaps the stopped job and **goes on with the rest of the command line**:

- **A loop keeps going.** In the live rehearsal for row 10e, a ghost's
  `for i in $(seq 1 120); do echo tick $i; sleep 1; done` kept ticking
  through a 12 s pause. The status line said `PAUSED`, the cast log's
  largest gap between ticks was 1.0 s, and the ghost was then told "The
  user paused shell s0 for 12 s", which was false.
- **A single command reads as finished.** With the daemon's marker
  wrapping, bash prints the end marker at once with exit **147**
  (`128 + SIGSTOP`, signal 19), so the caller gets a finished command while its job
  sits stopped.

After this phase, `PtyShell::signal_foreground` (`src/shell/pty.rs`):

- **`STOP` stops the shell first, then the job.** A stopped shell cannot
  reap the stop, so the job keeps the terminal and nothing runs on.
- **`CONT` resumes the job first, then the shell.** Bash wakes to a running
  job and never sees the stop. The command finishes with its real exit code.
- **`INT`/`TERM` on a paused shell also resume the job and the shell**,
  after queueing the signal. Otherwise an interrupt or a timeout would wait
  on two stopped processes forever.
- **`hangup()` on a paused shell resumes it after queueing `HUP`**, so
  closing a paused shell ends it.
- A new private helper, `signal_shell(sig)`, sends a signal to the shell's
  own pid. `hangup()` now uses it.

This is row 10d. Row 10e (the live close) follows.

## Architecture references

- `docs/design/daemoneye-2.0.md` § 2.6: attached mode, `Ctrl-p` pause and
  resume.
- `docs/dev/milestones/M23-client-decoupling/README.md` § Exit criteria:
  "pause it (`Ctrl-p`), resume it".

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read this entire phase doc before touching any code.
3. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

Measured on **2026-10-05** at commit `97c5639f`.
Baseline: `cargo test --lib` gives **2223 passed / 0 failed / 14 ignored**.

**The whole change below was dry-run in this tree, then reverted.** Results
on the dry run:
- the full suite gave lib **2225**/0/14, and every other target was unmoved
  (bug_tracker 6, doc_truth 15, integration 32, isolation 11);
- fmt and clippy `-D warnings` were clean;
- the six `shell::pty::pause_resume_tests` passed on 6 of 6 runs;
- **the § End-to-end verification block itself was run on the dry-run tree**
  and printed exactly the Expected values below. No capture line ends in
  whitespace. On the current tree it prints `lib_counts=2223p-0f-14i`,
  `phase_tests=4p-0f` and `wired=0/0/1`;
- Mutations A–D each went red on 3 of 3 runs, **and each mutant passes
  clippy `-D warnings` with zero diagnostics**. No stopped process was left
  behind by any run (`ps -eo stat` shows no `T`).

### Facts measured for this phase

- **The mechanism, measured under a plain PTY** (Python `pty.fork` running
  `bash --norc -i`; **not** under tmux, whose server sends `SIGCONT` to any
  pane process that stops, which hides the effect):

  | Pause method | single `sleep 6` during / after resume | loop `t1..t6` during / after resume |
  |---|---|---|
  | foreground group only (today) | `Stopped` + `END 147` / nothing | `Stopped`, `t4 t5 t6 END 0` / nothing |
  | shell first, then group; resume group, then shell | nothing / `END 0` | nothing / `t4 t5 t6 END 0` |

- **The real door, measured.** The live rehearsal (throwaway daemon with
  `tmux_session = ""`, a webhook ghost running a 60-tick loop in its PTY
  shell, `/agents` attach, `Ctrl-p` for 12 s, `Ctrl-p`, `Ctrl-d`):
  - **current tree:** largest tick gap 1.0 s, so the loop never paused;
  - **dry-run tree:** largest gap 12.9 s, right after `tick 11`; all 60
    ticks arrived, no `Stopped` line in the cast, and the ghost's notice
    ("paused … for 12 s") is true.

  `tmux ls` under the isolated `TMUX_TMPDIR` reported no server
  throughout.
- **Two existing tests asserted the defect as correct.**
  - `pty_pause_then_resume_actually_restarts_the_job` waited for "the
    stopped job to no longer hold the terminal (bash retook it)".
  - `pty_pause_resume_pause_toggles_more_than_once` waited for "the
    resumed job to no longer hold the terminal (bash kept it)".

  With the shell stopped, bash never takes the terminal back, so the diff
  replaces those legs:
  - the first test now asserts the shell itself is in `/proc` state `T`
    and the job still holds the terminal;
  - after resume it asserts the shell is back in `S`/`R` and ends the job
    with `INT`. The job still holds the terminal, and the shell cannot run
    `printf 'RESUMED'` until the job is gone.
- **The new test's needle must not match the echo.** The PTY echoes the
  typed line, so `printf 'E''ND %s\n'` splits the word, the same trick as
  the marker protocol's `DE_''BEG`. A first draft matched `END ` in the echo
  and failed.
- **The `/proc` state idiom** is `job_state_char` in the test module
  (`src/shell/pty.rs`, `pause_resume_tests`): the state char is the first
  token after the last `)`.
- **Clippy:** `b"sleep\02\0"` trips `clippy::octal_escapes`. The diff writes
  `b"sleep\x002\x00"`.
- **`stopped_procs=0`** in the capture is a preservation line. A test that
  left a stopped process behind would show here.
- **Known flakes:**
  `shell::backend::tests::input_after_a_prompt_logs_a_redaction_marker` and
  `daemon::utils::event_log::tests::for_each_streams_across_segments_in_order`.
  The frequent `pty_stop_with_no_foreground_job_remembers_nothing` is **in
  the file this phase changes**, but this phase does not change it. It
  passed 6 of 6 on the dry run. If it fails, re-run it as a known flake and
  name it.
- **Gotcha: no process killing.** Do not run `kill`, `pkill`,
  `tmux kill-*`, `daemoneye chat` or `daemoneye daemon` yourself. The tests
  send their own signals to processes they started.

## Spec

Make **exactly** the changes the diff below shows. It is the dry run,
formatted and clippy-clean, tests included. Apply it hunk by hunk with your
edit tools. `cargo build --tests` is the check that you missed nothing.
Copy; do not retype from memory.

### Task 1 — the fix and its tests (`src/shell/pty.rs`)

```diff
--- a/src/shell/pty.rs
+++ b/src/shell/pty.rs
@@ -364,13 +364,19 @@ impl PtyShell {
     /// `CONT` — to a process group.
     ///
     /// `STOP` targets the group this shell is tracking — the paused or
-    /// resumed memory first, because the moment the job stops bash takes the
-    /// terminal back (F2) and a later target cannot re-derive it from a tty
-    /// query; a shell tracking nothing falls back to the live foreground
-    /// group, unless that group is the shell's own (F1: stopping it would
-    /// stop the shell). `CONT` targets only the remembered paused group and
-    /// clears it. `INT`/`TERM` keep the live foreground target and clear the
-    /// memory, because a cancelled job leaves nothing paused (F4).
+    /// resumed memory first, so a later target never has to re-derive it
+    /// from a tty query; a shell tracking nothing falls back to the live
+    /// foreground group, unless that group is the shell's own (F1). It stops
+    /// **the shell first, then the job**: a running bash reaps a stopped job
+    /// and runs on — the rest of a `for` loop keeps going, and a wrapped
+    /// command prints its end marker with exit 147, so the command reads as
+    /// finished while its job sits stopped (measured). A stopped shell
+    /// cannot. `CONT` resumes the remembered group first, then the shell, so
+    /// bash wakes to a running job and never sees the stop; it clears the
+    /// memory. `INT`/`TERM` keep the live foreground target and clear the
+    /// memory, because a cancelled job leaves nothing paused (F4); on a
+    /// paused shell they also resume the job and the shell, or the signal
+    /// would wait on two stopped processes forever.
     ///
     /// portable-pty exposes no signal-sending API, so the platform `kill(1)`
     /// is invoked with the negative PGID. The shell itself sits in its own
@@ -409,17 +415,18 @@ impl PtyShell {
                     }
                 }
                 if let Some(pgid) = target {
+                    self.signal_shell("STOP")?;
                     self.signal_group(pgid, sig)?;
                     self.paused_pgid = Some(pgid);
                     self.resumed_pgid = None;
                 }
             }
             "CONT" => {
-                // F2: the stopped job is not the foreground group any more;
-                // a CONT that re-derives its target signals the shell's own
-                // group and the job stays stopped (red-verified).
+                // The remembered group, never a re-derived one: a tty query
+                // can answer the shell's own group and leave the job stopped.
                 if let Some(pgid) = self.paused_pgid.take() {
                     self.signal_group(pgid, sig)?;
+                    self.signal_shell("CONT")?;
                     self.resumed_pgid = Some(pgid);
                 }
             }
@@ -427,7 +434,10 @@ impl PtyShell {
                 if let Some(pgid) = current {
                     self.signal_group(pgid, sig)?;
                 }
-                self.paused_pgid = None;
+                if let Some(pgid) = self.paused_pgid.take() {
+                    self.signal_group(pgid, "CONT")?;
+                    self.signal_shell("CONT")?;
+                }
                 self.resumed_pgid = None;
             }
         }
@@ -439,20 +449,32 @@ impl PtyShell {
     }
 
     /// Hang up the shell itself: SIGHUP to its pid, not to the foreground
-    /// group. An interactive bash exits on it and hangs up its own jobs.
+    /// group. An interactive bash exits on it and hangs up its own jobs. A
+    /// paused shell is stopped, so the hangup would wait forever: it and its
+    /// job are resumed after the signal is queued.
     pub fn hangup(&self) -> anyhow::Result<()> {
+        self.signal_shell("HUP")?;
+        if let Some(pgid) = self.paused_pgid {
+            self.signal_group(pgid, "CONT")?;
+            self.signal_shell("CONT")?;
+        }
+        Ok(())
+    }
+
+    /// Send `sig` to the shell's own pid.
+    fn signal_shell(&self, sig: &str) -> anyhow::Result<()> {
         let pid = self
             .pid()
-            .ok_or_else(|| anyhow::anyhow!("the shell has no pid to hang up"))?;
+            .ok_or_else(|| anyhow::anyhow!("the shell has no pid to signal"))?;
         let status = std::process::Command::new("kill")
             .arg("-s")
-            .arg("HUP")
+            .arg(sig)
             .arg("--")
             .arg(pid.to_string())
             .status()
-            .context("running kill -s HUP")?;
+            .with_context(|| format!("running kill -s {sig}"))?;
         if !status.success() {
-            anyhow::bail!("kill -s HUP -- {pid} failed: {status}");
+            anyhow::bail!("kill -s {sig} -- {pid} failed: {status}");
         }
         Ok(())
     }
@@ -737,19 +759,17 @@ mod pause_resume_tests {
             "the remembered job to be in /proc state T after STOP",
         );
 
-        // The regression leg: a paused job is NOT the foreground group — bash
-        // took the terminal back — so a CONT that re-derives its target from
-        // a tty query (the pre-08a behaviour, F2) would signal the
-        // shell's own group and leave the job stopped forever. This
-        // foreground condition is itself part of the guard: after the STOP
-        // the terminal no longer belongs to the job. Bash reaps the stop and
-        // takes the terminal back asynchronously (after the job is already
-        // in state T, in order but not atomically), so this waits for the
-        // handoff rather than asserting it instantly (bug-08a-1).
+        // The shell is stopped too, so it cannot reap the stopped job and
+        // run on: the job keeps the terminal for the whole pause.
+        let shell_pid = shell.pid().map(i64::from).expect("the shell has a pid");
         poll_until(
             Instant::now() + Duration::from_secs(10),
-            || !shell.has_foreground_job(),
-            "the stopped job to no longer hold the terminal (bash retook it)",
+            || job_state_char(shell_pid).as_deref() == Some("T"),
+            "the shell itself to be in /proc state T after STOP",
+        );
+        assert!(
+            shell.has_foreground_job(),
+            "the paused job keeps the terminal: bash never took it back"
         );
 
         shell
@@ -760,10 +780,18 @@ mod pause_resume_tests {
             || matches!(job_state_char(paused).as_deref(), Some("S") | Some("R")),
             "the remembered job to resume (state S or R) after CONT",
         );
+        poll_until(
+            Instant::now() + Duration::from_secs(10),
+            || matches!(job_state_char(shell_pid).as_deref(), Some("S") | Some("R")),
+            "the shell to resume (state S or R) after CONT",
+        );
         assert!(
             shell.paused_pgid().is_none(),
             "CONT consumed the memory: nothing is paused afterwards"
         );
+        // The job still holds the terminal, so the shell cannot run the next
+        // command until it is gone.
+        shell.signal_foreground("INT").expect("end the resumed job");
         let outcome = shell
             .run("printf 'RESUMED\\n'", Duration::from_secs(10))
             .expect("the shell still answers after a pause/resume cycle");
@@ -872,18 +900,9 @@ mod pause_resume_tests {
             Some(pgid),
             "the resumed group must still be alive before the second STOP"
         );
-        // Same F2 guard as the primary test: after the resumed job runs, the
-        // terminal belongs to bash, not to the job — poll, don't assert (bug-08a-1).
-        poll_until(
-            Instant::now() + Duration::from_secs(10),
-            || !shell.has_foreground_job(),
-            "the resumed job to no longer hold the terminal (bash kept it)",
-        );
-
-        // Second cycle: the terminal is bash's now — inside its own group
-        // bash can also transiently hand the terminal to an rc-spawned
-        // child — so the STOP must reach the job through the remembered
-        // group, not through a tty query.
+        // Second cycle: the STOP reaches the job through the remembered
+        // group, not through a tty query — inside its own group bash can
+        // transiently hand the terminal to an rc-spawned child.
         shell.signal_foreground("STOP").expect("the second STOP");
         assert_eq!(
             shell.paused_pgid(),
@@ -906,6 +925,90 @@ mod pause_resume_tests {
             "the second CONT must consume the paused memory too"
         );
     }
+
+    /// The property a pause exists for: a command paused mid-run does not
+    /// finish during the pause, and after the resume it finishes with its
+    /// own exit code. With only the job stopped, bash reaped the stop and
+    /// printed `END 147` at once (measured).
+    #[test]
+    fn pty_a_paused_command_finishes_with_its_real_exit_code() {
+        let _home = crate::test_home_guard();
+        let mut shell = PtyShell::spawn("bash", &[], (24, 80))
+            .map(JobGuard)
+            .expect("bash available");
+        let mut rx = shell.subscribe();
+        shell
+            // `E''ND` in the typed line: the echo must not match the needle.
+            .write_input(b"sleep 2; printf 'E''ND %s\\n' $?\n")
+            .expect("start the command");
+        let pid = shell.pid().map(i64::from);
+        poll_until(
+            Instant::now() + Duration::from_secs(10),
+            || {
+                shell.current_foreground_pgid().is_some_and(|fg| {
+                    Some(fg) != pid
+                        && std::fs::read(format!("/proc/{fg}/cmdline"))
+                            .is_ok_and(|c| c == b"sleep\x002\x00")
+                })
+            },
+            "the command's sleep to hold the terminal",
+        );
+        shell.signal_foreground("STOP").expect("pause the command");
+        let paused = shell.paused_pgid().expect("STOP remembers the job");
+        poll_until(
+            Instant::now() + Duration::from_secs(10),
+            || job_state_char(paused).as_deref() == Some("T"),
+            "the job to be in /proc state T after STOP",
+        );
+        shell.signal_foreground("CONT").expect("resume the command");
+
+        let mut seen = Vec::new();
+        let deadline = Instant::now() + Duration::from_secs(10);
+        while !String::from_utf8_lossy(&seen).contains("END ") {
+            assert!(
+                Instant::now() < deadline,
+                "the command never finished: {:?}",
+                String::from_utf8_lossy(&seen)
+            );
+            match rx.try_recv() {
+                Ok(chunk) => seen.extend_from_slice(&chunk),
+                Err(_) => std::thread::sleep(Duration::from_millis(20)),
+            }
+        }
+        let text = String::from_utf8_lossy(&seen);
+        assert!(
+            text.contains("END 0"),
+            "the paused command finished with its own exit code: {text:?}"
+        );
+        assert!(
+            !text.contains("Stopped"),
+            "bash never saw the job stop: {text:?}"
+        );
+    }
+
+    /// Closing a paused shell ends it: the hangup reaches a shell that is
+    /// itself stopped, so the shell and its job are resumed after it.
+    #[test]
+    fn pty_hangup_while_paused_ends_the_shell() {
+        let _home = crate::test_home_guard();
+        let mut shell = PtyShell::spawn("bash", &[], (24, 80))
+            .map(JobGuard)
+            .expect("bash available");
+        start_job(&mut shell, b"sleep 30\n");
+        shell.signal_foreground("STOP").expect("pause the job");
+        let paused = shell.paused_pgid().expect("STOP remembers the job");
+        poll_until(
+            Instant::now() + Duration::from_secs(10),
+            || job_state_char(paused).as_deref() == Some("T"),
+            "the job to be in /proc state T after STOP",
+        );
+        shell.hangup().expect("hang up the paused shell");
+        poll_until(
+            Instant::now() + Duration::from_secs(10),
+            || !shell.is_alive().unwrap_or(true),
+            "the paused shell to exit after the hangup",
+        );
+    }
 }
 
 #[cfg(test)]
```

### Task 2 — tests

All are in the diff above, in `mod pause_resume_tests`:
- new: `pty_a_paused_command_finishes_with_its_real_exit_code`,
  `pty_hangup_while_paused_ends_the_shell`;
- adjusted (names unchanged): `pty_pause_then_resume_actually_restarts_the_job`,
  `pty_pause_resume_pause_toggles_more_than_once`.

**Test count:** two new, so **2225 passed / 14 ignored**; the module's filter
prints **6**.

### Task 3 — apply and record Mutations A–D (before the capture)

Do this **after** Tasks 1–2 pass and **before** Task 4. A mutation does not
affect the capture: each one is restored, and `git diff --stat` is checked,
before the next step. Follow § Mutations: for each one, apply it with the
`patch` tool, run its named test once, restore it, and confirm
`git diff --stat` matches. Then create an Update Log entry titled exactly
`### Update — <date> (mutations)` with one line per mutation: its letter,
the test, and the observed result (`red` or `green`).

### Task 4 — capture the end-to-end evidence

Run the block in § End-to-end verification verbatim. Then create a new Update
Log entry titled exactly `### Update — <date> (end-to-end verification)`.
**Its first fenced block is the contents of `/tmp/e2e-m23-10d.txt`**: the
file's lines, copied by a tool, **unchanged**. No line ends in whitespace.
**Never paste the script itself**, never retype the lines, and do not also
paste them unfenced. Then run the paste check once and put the line **it
prints** into the entry on its own line. If it prints `PASTE MISMATCH`,
write that line, not another.

## Acceptance criteria

Parenthesised values are what each line produces against the current tree.
Every non-preservation line fails today. Each line was run on the current
tree **and** on the dry-run tree.

- [ ] `cargo test --lib` → **2225 passed / 0 failed / 14 ignored** on three
      consecutive runs (current: 2223/0/14). Both counts are exact. A failure
      of a known flake named below is re-run, not counted.
- [ ] `cargo fmt --all` clean; `cargo build` clean;
      `cargo clippy --all-targets --all-features -- -D warnings` exits 0.
- [ ] `cargo test` → other targets unmoved: bug_tracker 6, doc_truth 15,
      integration 32, isolation 11 (*preservation*).
- [ ] The module's tests pass:
      ``cargo test --lib -- shell::pty::pause_resume_tests`` → **6 passed**
      (current: 4). That exact filter printed 6 on the dry run.
- [ ] No stopped process is left behind: the capture's `stopped_procs=`
      line reads **0** (*preservation*; current: 0).
- [ ] Wired, as the end-to-end block's `wired=` line prints it →
      **`1/3/0`** (current: `0/0/1`):
      ``grep -c 'self.signal_shell("STOP")?;' src/shell/pty.rs`` → 1;
      ``grep -c 'self.signal_shell("CONT")?;' src/shell/pty.rs`` → 3
      (`CONT`, the `INT`/`TERM` arm, `hangup`);
      ``grep -c 'bash retook it' src/shell/pty.rs`` → 0 (the test leg that
      asserted the defect is gone).
- [ ] The mutations entry exists and records all four:
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
`perl -i`, `> file` and `rm -rf`). Run the named test once with
`cargo test --lib -- <name>`, restore, and record the observed result in the
Task 3 entry. Each was measured red at drafting on 3 of 3 runs, and each
mutant builds clippy-clean.

- **Mutation A** (`STOP` stops the shell): in the `"STOP"` arm, delete the
  line `                    self.signal_shell("STOP")?;`. Run
  `pty_a_paused_command_finishes_with_its_real_exit_code`. Expected **red**.
- **Mutation B** (`CONT` resumes the shell): in the `"CONT"` arm, delete the
  line `                    self.signal_shell("CONT")?;` (the one followed
  by `self.resumed_pgid = Some(pgid);`). Run
  `pty_a_paused_command_finishes_with_its_real_exit_code`. Expected **red**.
- **Mutation C** (an interrupt resumes a paused shell): in the `_` arm,
  replace the four lines
  `                if let Some(pgid) = self.paused_pgid.take() {` /
  `                    self.signal_group(pgid, "CONT")?;` /
  `                    self.signal_shell("CONT")?;` / `                }`
  with the one line `                self.paused_pgid = None;`. Run
  `pty_cancel_clears_the_paused_group`. Expected **red**.
- **Mutation D** (a hangup resumes a paused shell): in `hangup`, delete the
  four lines `        if let Some(pgid) = self.paused_pgid {` /
  `            self.signal_group(pgid, "CONT")?;` /
  `            self.signal_shell("CONT")?;` / `        }`. Run
  `pty_hangup_while_paused_ends_the_shell`. Expected **red**.

After each restore, confirm `git diff --stat` matches its pre-mutation state.

## Test plan

- `pty_a_paused_command_finishes_with_its_real_exit_code`: a `sleep 2;
  printf 'E''ND %s\n' $?` line, paused while `sleep` holds the terminal,
  then resumed. The output carries `END 0`, never `Stopped`.
- `pty_hangup_while_paused_ends_the_shell`: a paused `sleep 30`, then
  `hangup()`; the shell exits within the wait.
- `pty_pause_then_resume_actually_restarts_the_job` (adjusted): after
  `STOP`, the shell is in state `T` and the job holds the terminal; after
  `CONT`, the job and the shell are both `S`/`R`.
- `pty_pause_resume_pause_toggles_more_than_once` (adjusted): two full
  cycles through the remembered group.
- `pty_cancel_clears_the_paused_group` (unchanged): `INT` on a paused shell
  leaves it usable. It is Mutation C's witness.

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
  printf 'phase_tests=%s\n' "$(cargo test --lib -- shell::pty::pause_resume_tests 2>&1 | grep -E '^test result' | tail -1 | sed -E 's/.*ok\. ([0-9]+) passed; ([0-9]+) failed.*/\1p-\2f/')"
  printf 'stopped_procs=%s\n' "$(ps -eo stat= | grep -c '^T')"
  printf 'wired=%s/%s/%s\n' "$(grep -c 'self.signal_shell("STOP")?;' src/shell/pty.rs)" "$(grep -c 'self.signal_shell("CONT")?;' src/shell/pty.rs)" "$(grep -c 'bash retook it' src/shell/pty.rs)"
} > /tmp/e2e-m23-10d.txt
cat /tmp/e2e-m23-10d.txt
```

Expected:
- `lib1=ok failed:none`, `lib2=ok failed:none`, `lib3=ok failed:none`;
- `lib_counts=2225p-0f-14i`;
- `phase_tests=6p-0f`;
- `stopped_procs=0`;
- `wired=1/3/0`.

**Re-run budget: three** (WORKFLOW.md § End-to-end verification). **Three
pre-existing lib flakes are known:**
`shell::backend::tests::input_after_a_prompt_logs_a_redaction_marker`,
`shell::pty::pause_resume_tests::pty_stop_with_no_foreground_job_remembers_nothing`
(frequent; in `src/shell/pty.rs`, but a test this phase does not change)
and `daemon::utils::event_log::tests::for_each_streams_across_segments_in_order`.
If one fails, name it and **re-run the whole block, using the budget**.
After three re-runs, stop, paste the last capture as it is, and name every
failing test. Any other failing test is a finding: report it by name. A
failure of any of the five other `pause_resume_tests` is **not** a flake.

**Run this block with nothing else running `cargo test`.** Then make the
entry's first fenced block the **contents of** `/tmp/e2e-m23-10d.txt`
(copied, not retyped; **never the script above**), and run:

```sh
D=docs/dev/milestones/M23-client-decoupling/phase-10d-pause-stops-the-shell.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-m23-10d.txt
diff /tmp/pasted-m23-10d.txt /tmp/e2e-m23-10d.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

If it prints `PASTE MISMATCH`, run
`diff /tmp/pasted-m23-10d.txt /tmp/e2e-m23-10d.txt` once and fix what it
shows. **Run the check at most three times.** Put the line it prints into the
entry on its own line, exactly as printed. Never write a verdict this check
did not print.

## Authorizations

You may edit these files, and nothing else:

- `src/shell/pty.rs`: the diff above.
- This phase doc's Update Log.

**Do not run `kill`, `pkill`, `tmux kill-*`, `daemoneye chat` or
`daemoneye daemon` yourself.** The tests signal only processes they start.

## Out of scope

- **A long pause and a command's deadline.** A paused command's wait keeps
  counting against its timeout (300 s for a ghost's command). Pause time is
  not added back, as prompt time is (`read_to_marker`).
- **Pausing a sandboxed ghost's container** (`docker pause`). Carried to M26
  with the sandboxed-ghost row (PE decision 2026-10-05).
- **The daemon and client sides of pause** (08a, 08c). They are unchanged;
  only the shell host's signal step changes.

## Update Log

<!-- entries appended below this line -->
