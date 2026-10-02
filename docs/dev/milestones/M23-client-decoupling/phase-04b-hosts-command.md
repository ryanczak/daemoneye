# Phase 04b: hosts-command — `/hosts`, and two shell-tail defects found at the 04a review

**Milestone:** M23 — Client decoupling & shell UX
**Status:** in-progress
**Depends on:** 04a, 03d (done)
**Estimated diff:** ~90 lines of production code across 3 files; 5 new tests, 2 fixed
**Tags:** language=rust, kind=feature, size=m

## Goal

Two parts.

1. **`/hosts`** in `daemoneye chat` lists the implicit `local` host, then each
   configured `[hosts.<name>]` entry. Each row shows its ssh destination,
   whether an ssh ControlMaster is running (`master:up` / `master:down`), and
   how many **live** shells it has. If the daemon cannot be asked, the counts
   read `shells:?`.
2. **Two defects in the 03d shell tail**, found at the 04a review and proved
   at drafting (carried, required):
   - **A relay defect.** When the client hangs up while a chunk is in flight,
     the tail returns `Err(broken pipe)`, so every closed tail would be logged
     as an error. Measured in **3 of 8** full test runs.
   - **A test defect.** `shell_tail_without_follow_sends_only_the_screen`
     asserts on the first screen instead of polling. Its marker
     (`printf 'marker-tail-a\n'`) is also in the typed command's echo, so a
     pass does not prove the output reached the screen. The follow test has
     the same echo proxy (`marker-tail-b`).

Row 04c (the `[HOSTS]` context block) follows.

## Architecture references

- `docs/design/daemoneye-2.0.md` § 2.6 — `/hosts`.
- `docs/dev/milestones/M23-client-decoupling/README.md` — row 04b and its
  carried item.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read this entire phase doc before touching any code.
3. Confirm the repo is on a clean branch with no uncommitted changes.
4. **Never run `kill`, `pkill`, `killall` or `ssh` by hand.** The tests do
   not need ssh. `/hosts` calls an existing function that runs `ssh -O check`,
   and no test calls it.

## Current state

Measured on **2026-10-02** at commit `d764b5f9`. Baseline:
`cargo test --lib` → **2125 passed / 0 failed / 14 ignored**.

**The whole change below was dry-run in this tree, then reverted.** Results
on the dry run:
- the full suite gave lib **2130**/0/14 on **five** consecutive runs;
- clippy `-D warnings` was clean;
- the measured mutations behaved as listed in § Mutations.

### `/hosts` facts (measured)

- `crate::shell::master_running(entry: &HostEntry, control_path: &Path) -> bool`
  (`src/shell/session.rs:159`) already runs
  `ssh -O check -o ControlPath=<dir>/%C <entry.ssh>`. Reuse it; do not write
  another probe.
  - With a master running, `ssh -O check` exits 0 and prints
    `Master running (pid=…)`.
  - With none, it exits 255 and prints
    `Control socket connect(…): No such file or directory`.
  - Both answer in under 10 ms with **no network** (measured live against
    the configured host `pinky`).
- The control directory is `crate::config::ssh_dir()`.
- Configured hosts are `crate::config::Config::load()` → `.hosts`, a
  `HashMap<String, HostEntry>`, where `HostEntry.ssh` is the destination. The
  chat client already loads `Config` (`chat.rs`), and it runs on the daemon's
  machine.
- `ShellInfo.host` is `"local"` or the host's name, and `exited_unix` is
  `Some` for an exited shell.

### The tail's sends today (`src/daemon/server/shells.rs:175-206`)

```rust
    send_response_split(tx, Response::ShellScreen { bytes: screen }).await?;   // :175
                        send_response_split(tx, Response::ShellChunk { bytes }).await?;   // :189
                    send_response_split(tx, Response::ShellExited).await?;   // :195
                    send_response_split(tx, Response::ShellExited).await?;   // :206
```

The `?` turns a client that hung up into `Err`.

**Measured at drafting:** with only the tests' run-time markers in place, the
follow test failed **3 of 8** full runs with
`the tail returns Ok after a hang-up, got Ok(Err(broken pipe))`. With the
send fix below as well: **0 of 8**.

**Gotcha, measured:** you cannot provoke the broken pipe by dropping one half
of a `tokio::io::split` duplex. The halves share the stream, so writes just
buffer. The deterministic test below uses a writer that fails after its first
frame.

### The 03d test defects (`src/daemon/server/shells.rs`)

- `:646` types `b"printf 'marker-tail-a\\n'\n"`, and `:658-664` asserts on
  the first `ShellScreen`:

  ```rust
            if let [Response::ShellScreen { bytes }] = &parse_all(&tx)[..] {
                assert!(
                    rendered(bytes).contains("marker-tail-a"),
                    …
                );
                break;
            }
  ```

  **Measured:** with an output-only marker delayed 0.3 s
  (`sleep 0.3; printf 'marker-%s\\n' tail-a`), this fails **3/3**.
- `:702` types `b"printf 'marker-tail-b\\n'\n"`, the same echo proxy.

## Spec

### Task 1 — `/hosts` (`src/cli/commands/slash.rs`)

1. Add the dispatch arm right after the `/shells` arm (`:62`):

   ```rust
        "/hosts" => cmd_hosts(ctx).await,
   ```

2. Insert this block **above** `async fn cmd_pane` (`:291`). It is the
   dry-run code, formatted and clippy-clean:

```rust
// ── /hosts ───────────────────────────────────────────────────────────────────

/// One configured `[hosts.<name>]` entry as `/hosts` shows it.
struct HostView {
    name: String,
    ssh: String,
    /// Whether an ssh ControlMaster is running for it (`ssh -O check`).
    master: bool,
}

/// Render the `/hosts` panel body: the implicit `local` host first, then each
/// configured host in `hosts` order, with its live-shell count. `shells` is
/// `None` when the daemon could not be asked, and the counts read `?`. Pure
/// so it can be tested without a daemon or ssh.
fn render_host_list(hosts: &[HostView], shells: Option<&[crate::ipc::ShellInfo]>) -> Vec<String> {
    let live_on = |name: &str| match shells {
        Some(list) => list
            .iter()
            .filter(|s| s.host == name && s.exited_unix.is_none())
            .count()
            .to_string(),
        None => "?".to_string(),
    };
    let mut body = vec![format!(
        "local  (this machine)  shells:{}",
        live_on("local")
    )];
    for h in hosts {
        let master = if h.master { "up" } else { "down" };
        body.push(format!(
            "{}  ssh:{}  master:{}  shells:{}",
            h.name,
            h.ssh,
            master,
            live_on(&h.name)
        ));
    }
    if hosts.is_empty() {
        body.push(
            "no remote hosts configured — add [hosts.<name>] with ssh = \"user@host\" to config.toml"
                .to_string(),
        );
    }
    body
}

async fn cmd_hosts(ctx: SlashCtx<'_>) {
    let cfg = crate::config::Config::load().unwrap_or_default();
    let control = crate::config::ssh_dir();
    let mut names: Vec<&String> = cfg.hosts.keys().collect();
    names.sort();
    let hosts: Vec<HostView> = names
        .into_iter()
        .map(|name| {
            let entry = &cfg.hosts[name];
            HostView {
                name: name.clone(),
                ssh: entry.ssh.clone(),
                master: crate::shell::master_running(entry, &control),
            }
        })
        .collect();
    let shells = match request_once(Request::ShellList).await {
        Ok(Response::ShellList { shells }) => Some(shells),
        Ok(other) => {
            render_error(ctx.renderer, other);
            None
        }
        Err(e) => {
            note(ctx.renderer, &format!("✗ {e}"));
            None
        }
    };
    let body = render_host_list(&hosts, shells.as_deref());
    let _ = ctx.renderer.commit_panel("hosts", &body, false);
}
```

### Task 2 — help (`src/cli/commands/chat.rs`)

Replace the line `  /shells    list the agent's shells` (`:23`) with:

```text
  /shells    list the agent's shells  /hosts     configured hosts and ssh state
```

Keep its leading two spaces, so 04a's `^  /shells ` check still matches.

### Task 3 — the tail: a client that hung up ends it cleanly (`src/daemon/server/shells.rs`)

From the `ShellScreen` send onward, a failed write to the client means it
hung up. The tail returns `Ok(())`, not `Err`.

1. Replace the `ShellScreen` send (`:175`) with:

   ```rust
    // From here on a failed write means the client hung up: that ends the
    // tail normally, it is not an error.
    if send_response_split(tx, Response::ShellScreen { bytes: screen })
        .await
        .is_err()
    {
        return Ok(());
    }
   ```

2. Replace the `Ok(Some(line)) => { … }` arm of the host branch with this
   (a let-chain; clippy's `collapsible_if` rejects the nested form):

   ```rust
                Ok(Some(line)) => {
                    if let Ok(crate::shell::ShellResponse::Chunk { bytes }) =
                        serde_json::from_str(&line)
                        && send_response_split(tx, Response::ShellChunk { bytes })
                            .await
                            .is_err()
                    {
                        return Ok(());
                    }
                }
   ```

3. In **both** `ShellExited` sends (`:195`, `:206`), replace
   `send_response_split(tx, Response::ShellExited).await?;` with
   `let _ = send_response_split(tx, Response::ShellExited).await;`. Each is
   already followed by `return Ok(());`.

Leave the `Response::Error(…)` sends before the connection with their `?`.

### Task 4 — the 03d tests (`src/daemon/server/shells.rs`)

1. In `shell_tail_without_follow_sends_only_the_screen`:
   - type `b"printf 'marker-%s\\n' tail-a\n"`;
   - replace the assert-and-break block (`:658-664`) with a poll:

     ```rust
            // Poll: the first screen may predate the command's output.
            if let [Response::ShellScreen { bytes }] = &parse_all(&tx)[..]
                && rendered(bytes).contains("marker-tail-a")
            {
                break;
            }
     ```

   The `tx.clear()` and the deadline assert after it stay as they are.
2. In `shell_tail_follow_streams_live_chunks_until_the_client_disconnects`,
   type `b"printf 'marker-%s\\n' tail-b\n"`. The rest is unchanged.

The marker is built when `printf` runs, so the text `marker-tail-a` exists
only in the command's **output**, never in its echo.

### Task 5 — new tests

**In `src/cli/commands/slash.rs`'s `mod tests`**, right after
`const NOW: u64 = 1_790_000_000;` (`:758`), add this dry-run code (it reuses
04a's `shell(…)` helper):

```rust
fn host(name: &str, ssh: &str, master: bool) -> HostView {
    HostView {
        name: name.to_string(),
        ssh: ssh.to_string(),
        master,
    }
}

#[test]
fn render_host_list_lists_local_first_then_configured_hosts() {
    let mut gone = shell("s3", "pinky", "chat:x", NOW);
    gone.exited_unix = Some(NOW);
    let shells = vec![
        shell("s0", "local", "chat:x", NOW),
        shell("s1", "local", "chat:x", NOW),
        shell("s2", "pinky", "chat:x", NOW),
        gone,
    ];
    let hosts = [host("pinky", "pinky", true), host("db", "admin@db", false)];
    let rows = render_host_list(&hosts, Some(&shells));
    assert_eq!(rows.len(), 3, "{rows:?}");
    assert!(rows[0].starts_with("local"), "{}", rows[0]);
    assert!(rows[0].contains("shells:2"), "{}", rows[0]);
    for want in ["pinky", "ssh:pinky", "master:up", "shells:1"] {
        assert!(rows[1].contains(want), "row 1 has {want:?}: {}", rows[1]);
    }
    for want in ["db", "ssh:admin@db", "master:down", "shells:0"] {
        assert!(rows[2].contains(want), "row 2 has {want:?}: {}", rows[2]);
    }
}

#[test]
fn render_host_list_without_the_daemon_shows_unknown_counts() {
    let rows = render_host_list(&[host("pinky", "pinky", false)], None);
    assert!(rows[0].contains("shells:?"), "{}", rows[0]);
    assert!(rows[1].contains("shells:?"), "{}", rows[1]);
}

#[test]
fn render_host_list_explains_when_no_hosts_are_configured() {
    let rows = render_host_list(&[], Some(&[]));
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert!(rows[0].contains("shells:0"), "{}", rows[0]);
    assert!(rows[1].contains("[hosts."), "{}", rows[1]);
}
```

**In `src/cli/commands/chat.rs`'s `mod tests`**, add
**`help_text_lists_hosts`**, which asserts that `HELP_TEXT` contains
`"/hosts"`.

**In `src/daemon/server/shells.rs`'s `mod tests`**, insert this dry-run code
right **above** `async fn shell_tail_reports_exited_when_the_shell_ends`
(`:739`, together with its `#[tokio::test]` line):

```rust
/// A client connection that takes the first frame and then hangs up:
/// every later write fails with `BrokenPipe`.
struct HangsUpAfterFirstFrame {
    done: bool,
}

impl tokio::io::AsyncWrite for HangsUpAfterFirstFrame {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        if self.done {
            return std::task::Poll::Ready(Err(std::io::ErrorKind::BrokenPipe.into()));
        }
        if buf.contains(&b'\n') {
            self.done = true;
        }
        std::task::Poll::Ready(Ok(buf.len()))
    }
    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::task::Poll::Ready(Ok(()))
    }
    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::task::Poll::Ready(Ok(()))
    }
}

#[tokio::test]
async fn shell_tail_ends_quietly_when_the_client_hangs_up_mid_stream() {
    let (_guard, _dir) = shell_test_home();
    let reg = ShellRegistry::default();
    let owner = Owner::Chat("t".into());
    let id = crate::shell::spawn_for(&reg, owner.clone(), 5, false, |id| {
        Box::pin(spawn_in_process_host(id))
    })
    .await
    .unwrap();
    // The client's reading side never reaches EOF (its other end is kept
    // open), so the tail can only learn of the hang-up from a failed write.
    let (_keep_open, client_side) = tokio::io::duplex(64);
    let task_reg = reg.clone();
    let task_id = id.as_str().to_string();
    let task = tokio::spawn(async move {
        let mut rx = tokio::io::BufReader::new(client_side);
        let mut tx = HangsUpAfterFirstFrame { done: false };
        handle_shell_tail(&mut rx, &mut tx, &task_reg, &task_id, true).await
    });
    // Give the tail time to send its screen, then make the shell print.
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    crate::shell::send_input(
        &crate::shell::socket_path(id.as_str()),
        b"printf 'marker-%s\\n' tail-c\n",
        RELAY_TIMEOUT,
    )
    .await
    .unwrap();
    let outcome = tokio::time::timeout(std::time::Duration::from_secs(5), task)
        .await
        .expect("the tail ends within 5 s of the failed write");
    assert!(
        matches!(&outcome, Ok(Ok(()))),
        "a client that hung up ends the tail without an error, got {outcome:?}"
    );
    close_tail_shell(&reg, id.as_str(), &owner).await;
}
```

**Test count:** five new, so **2130 passed / 14 ignored**:
- `render_host_list_lists_local_first_then_configured_hosts`;
- `render_host_list_without_the_daemon_shows_unknown_counts`;
- `render_host_list_explains_when_no_hosts_are_configured`;
- `help_text_lists_hosts`;
- `shell_tail_ends_quietly_when_the_client_hangs_up_mid_stream`.

### Task 6 — capture the end-to-end evidence

Run the block in § End-to-end verification verbatim. Then create a new Update
Log entry titled exactly `### Update — <date> (end-to-end verification)`.
**Its first fenced block is the contents of `/tmp/e2e-m23-04b.txt`**: the
file's lines, copied by a tool. **Never paste the script itself**, and never
retype the lines (the real lines end `lib1=ok ` **with a trailing space**).
Then run the paste check once, put the line it prints into the entry **on its
own line**, and record the four mutations after it.

## Acceptance criteria

Parenthesised values are what each line produces against the current tree.
Every non-preservation line fails today, and each was run at drafting.

- [ ] `cargo test --lib` → **2130 passed / 0 failed / 14 ignored** on **five**
      consecutive runs (current: 2125/0/14). Five, because the defect this
      fixes showed in 3 of 8.
- [ ] `cargo fmt --all` clean; `cargo build` clean;
      `cargo clippy --all-targets --all-features -- -D warnings` exits 0.
- [ ] `cargo test` → other targets unmoved: bug_tracker 6, doc_truth 14,
      integration 32, isolation 11 (*preservation*).
- [ ] The five new tests pass:
      ``cargo test --lib -- render_host_list help_text_lists_hosts shell_tail_ends_quietly_when_the_client_hangs_up_mid_stream``
      → **5 passed** (current: `0 passed`).
- [ ] Wired: ``grep -c '"/hosts" => cmd_hosts' src/cli/commands/slash.rs`` →
      **1** (current: 0), and
      ``grep -c '/hosts     configured hosts' src/cli/commands/chat.rs`` →
      **1** (current: 0).
- [ ] The echo proxy is gone:
      ``grep -c "printf 'marker-tail-[ab]" src/daemon/server/shells.rs`` →
      **0** (current: 2), and
      ``grep -c "printf 'marker-%s" src/daemon/server/shells.rs`` → **3**:
      tail-a, tail-b and the new test's tail-c (current: 0).
- [ ] Mutations A–D were applied, run, and their observed results recorded
      **in this doc**.
- [ ] The end-to-end entry exists:
      ``grep -c '^### Update — .* (end-to-end verification)$' <this doc>`` →
      **1** (current: 0). Its first fenced block is the capture file's contents,
      and the paste-check line is on its own line:
      ``grep -cx 'PASTE MATCH' <this doc>`` → **1** (current: 0).

## Mutations

Apply each with the `patch` tool (the executor contract forbids `sed -i`,
`perl -i`, `> file` and `rm -rf`). Run as stated, restore, and **record the
observed result**. Each was measured at drafting.

- **Mutation A:** in `render_host_list`, change
  `let master = if h.master { "up" } else { "down" };` to
  `let master = if h.master || true { "up" } else { "down" };`. Run
  `render_host_list_lists_local_first_then_configured_hosts`. Expected **red**.
- **Mutation B:** change `.filter(|s| s.host == name && s.exited_unix.is_none())`
  to `.filter(|s| s.host == name)`. Run the same test. Expected **red**
  (pinky counts its exited shell).
- **Mutation C (must stay GREEN):** in
  `shell_tail_without_follow_sends_only_the_screen`, change the typed bytes to
  `b"sleep 0.3; printf 'marker-%s\\n' tail-a\n"`. Run that test **three
  times**. Expected **green 3/3**: it polls. The old assert-on-first-screen
  loop failed this 3/3.
- **Mutation D:** put the old chunk send back: replace the let-chain arm
  from Task 3.2 with
  `if let Ok(crate::shell::ShellResponse::Chunk { bytes }) = serde_json::from_str(&line) { send_response_split(tx, Response::ShellChunk { bytes }).await?; }`.
  Run `shell_tail_ends_quietly_when_the_client_hangs_up_mid_stream`. Expected
  **red** with `got Ok(Err(broken pipe))` (measured 6/6).

After each restore, confirm `git diff --stat` matches its pre-mutation state.

## Test plan

Five new tests and two fixed:
- `/hosts` lists `local` first and counts only live shells per host;
- unknown counts without the daemon;
- the hint shown when no hosts are configured;
- `/hosts` in the help text;
- the tail ends `Ok` when the client hangs up mid-stream;
- the two 03d tests now use output-only markers, and one now polls.

## End-to-end verification

```sh
cd /home/matt/src/daemoneye
{
  for i in 1 2 3 4 5; do
    out=$(cargo test --lib 2>&1)
    printf 'lib%d=%s %s\n' "$i" "$(printf '%s\n' "$out" | grep -E '^test result' | tail -1 | grep -oE 'ok|FAILED')" "$(printf '%s\n' "$out" | grep -E '^test .* FAILED$' | sed -E 's/^test (.*) \.\.\. FAILED$/\1/' | tr '\n' ' ')"
  done
  printf 'lib_counts=%s\n' "$(cargo test --lib 2>&1 | grep -E '^test result' | tail -1 | sed -E 's/.*ok\. ([0-9]+) passed; ([0-9]+) failed; ([0-9]+) ignored.*/\1p-\2f-\3i/')"
  printf 'new_tests=%s\n' "$(cargo test --lib -- render_host_list help_text_lists_hosts shell_tail_ends_quietly_when_the_client_hangs_up_mid_stream 2>&1 | grep -E '^test result' | tail -1 | sed -E 's/.*ok\. ([0-9]+) passed; ([0-9]+) failed.*/\1p-\2f/')"
  printf 'isolation=%s\n' "$(cargo test --test isolation 2>&1 | grep -E '^test result' | tail -1 | grep -oE '[0-9]+ passed; [0-9]+ failed')"
  printf 'wired=%s/%s\n' "$(grep -c '"/hosts" => cmd_hosts' src/cli/commands/slash.rs)" "$(grep -c '/hosts     configured hosts' src/cli/commands/chat.rs)"
  printf 'markers=%s/%s\n' "$(grep -c "printf 'marker-tail-[ab]" src/daemon/server/shells.rs)" "$(grep -c "printf 'marker-%s" src/daemon/server/shells.rs)"
} > /tmp/e2e-m23-04b.txt
cat /tmp/e2e-m23-04b.txt
```

Expected:
- `lib1..5=ok` with **nothing after `ok`**;
- `lib_counts=2130p-0f-14i`;
- `new_tests=5p-0f`;
- `isolation=11 passed; 0 failed`;
- `wired=1/1`;
- `markers=0/3`.

**Re-run budget: three** (WORKFLOW.md § End-to-end verification). The known
pre-existing flake is `shell::backend::tests::input_after_a_prompt_logs_a_redaction_marker`.
If it fails, name it and re-run the whole block. **If
`shell_tail_follow_streams_live_chunks_until_the_client_disconnects` fails,
that is the defect this phase fixes.** Do not re-run past it: fix Task 3.
After three re-runs, stop, paste the last capture as it is, and name every
failing test.

**Run this block with nothing else running `cargo test`.** Then make the
entry's first fenced block the **contents of** `/tmp/e2e-m23-04b.txt`
(copied, not retyped; **never the script above**), and run:

```sh
D=docs/dev/milestones/M23-client-decoupling/phase-04b-hosts-command.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-m23-04b.txt
diff /tmp/pasted-m23-04b.txt /tmp/e2e-m23-04b.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

If it prints `PASTE MISMATCH`, run
`diff /tmp/pasted-m23-04b.txt /tmp/e2e-m23-04b.txt` once and fix what it
shows. **Run the check at most three times.** Put the line it prints into the
entry on its own line.

## Authorizations

You may edit these files, and nothing else:

- `src/cli/commands/slash.rs`: Task 1 and its tests.
- `src/cli/commands/chat.rs`: the one `HELP_TEXT` line and its test.
- `src/daemon/server/shells.rs`: Tasks 3 and 4 and the new test.
- This phase doc's Update Log.

## Out of scope

- **The `[HOSTS]` context block**: row 04c.
- **Starting or stopping a ControlMaster from `/hosts`.** It reports state
  only.
- **The chat-UI live check**: row 10.

## Update Log

<!-- entries appended below this line -->
### Update — 2026-10-01 20:54 (started)

**Executor:** opus (claude) — phase 04b run.
**Doing:** Task 1 `/hosts` in slash.rs, Task 2 help line, Task 3 tail hang-up → Ok, Task 4 03d test fixes, Task 5 new tests, Task 6 end-to-end capture.
### Update — 2026-10-01 21:06 (end-to-end verification)

Captured by the phase doc's § End-to-end verification block, redirected to
`/tmp/e2e-m23-04b.txt`; its contents follow, copied by a tool:

```
lib1=ok 
lib2=ok 
lib3=ok 
lib4=ok 
lib5=ok 
lib_counts=2130p-0f-14i
new_tests=5p-0f
isolation=11 passed; 0 failed
wired=1/1
markers=0/3
```

PASTE MATCH

**Mutations observed (each applied with `patch`, run, restored; `git diff
--stat` back to its 231-insertion pre-mutation state after each):**

- **A** (`h.master || true`): `render_host_list_lists_local_first_then_configured_hosts`
  → **red** — `row 2 has "master:down": db  ssh:admin@db  master:up  shells:0`.
- **B** (drop the `exited_unix.is_none()` filter): same test → **red** —
  `row 1 has "shells:1": pinky  ssh:pinky  master:up  shells:2`.
- **C** (typed bytes `sleep 0.3; printf 'marker-%s\\n' tail-a`, must stay
  green): `shell_tail_without_follow_sends_only_the_screen` **green 3/3**.
- **D** (old `?` chunk send back):
  `shell_tail_ends_quietly_when_the_client_hangs_up_mid_stream` → **red 3/3** —
  `got Ok(Err(broken pipe))` each run.

Other targets, one full `cargo test`: bug_tracker **6** passed, doc_truth **14**,
integration **32** (2 ignored), isolation **11** (1 ignored) — unmoved.
