# Phase 01: host-truth — the shell host reports what owns its terminal

**Milestone:** M28 — Interactive shells
**Status:** in-progress
**Depends on:** none
**Estimated diff:** ~450 lines
**Tags:** language=rust, kind=feature, size=m

## Goal

The daemon cannot tell what is in the foreground of a PTY shell, and cannot
see its screen; both live in the detached `shell-host` process. This phase
makes the host report them over its socket and adds a pure classifier that
turns the report into a `Foreground`. Nothing in the daemon calls either
yet — phase 01b adds the tool, phase 02 the decisions — so there is **no
behaviour change** for any existing tool.

## Architecture references

- `docs/design/interactive-shells.md` § 1, D1, D2, D3, § 5 — read these
  before starting; the § 5 table is what the classifier tests pin.

## Spec

1. **Selection markers in the screen** — in `src/shell/screen.rs`, add
   `ShellScreen::marked(annotated: bool) -> String`: the visible screen
   where every run of cells with `vt100::Cell::inverse()` set is wrapped
   `[SEL: …]`, and, only when `annotated`, the existing red/yellow/green
   runs are wrapped as `annotated()` does today. Generalise
   `annotated_row`; keep `annotated()` returning exactly what it returns now.
2. **Scrollback rows** — on `ShellScreen`, a way to get the last `n` rows
   that scrolled off the top as plain text (vt100's scrollback; set and
   restore the scrollback offset under the same borrow). `n` larger than
   what is kept returns what is kept.
3. **Proto frames** — in `src/shell/proto.rs`: request
   `Screen { annotated: bool, scrollback: u16 }`; response
   `ScreenText { text: String, rows: u16, cols: u16, cursor: (u16, u16),
   alt_screen: bool }`. `Status` gains `raw_mode: Option<bool>`,
   `alt_screen: Option<bool>`, `foreground: Option<String>`, each
   `#[serde(default)]` so the JSON a pre-phase host sends still parses.
4. **Backend** — `ShellBackend` (in `src/shell/host.rs`) gains
   `screen_text(annotated, scrollback) -> ShellResponse` with a **default**
   that answers `Error { message: "this shell host has no screen" }`;
   `handle` dispatches `Screen` to it. `PtyBackend` implements it under its
   screen mutex (scrollback rows first, then the marked screen) and fills
   the three new `Status` fields: `raw_mode` from the master's termios
   (`ICANON` clear ⇒ raw), `alt_screen` from the screen, `foreground` as the
   basename of argv[0] of the foreground group leader, `None` when that
   group is the shell's own. `SbxBackend` delegates `screen_text` to its
   inner backend.
5. **Client** — in `src/shell/client.rs`, `pub async fn screen(sock,
   annotated, scrollback, timeout) -> anyhow::Result<ShellResponse>`,
   shaped like the existing `status`.
6. **Classifier** — new `src/shell/foreground.rs`, exported from
   `src/shell/mod.rs`. `enum Foreground { Prompt, Program { cmd: String,
   tui: bool }, Paused { cmd: String }, Exited, Unknown }` and two pure
   functions:
   - `classify(status: Option<&ShellResponse>) -> Foreground` — a missing or
     non-`Status` reply ⇒ `Unknown`; `Exited` ⇒ `Exited`; any other state
     with `raw_mode == None` (pre-phase host) ⇒ `Unknown`, **never**
     `Prompt`; `Paused` ⇒ `Paused`; `Running` ⇒ `Program`, `tui = raw_mode
     || alt_screen`, `cmd` = `foreground` or `"?"`; `Idle` ⇒ `Prompt`.
   - `classify_transport(state, in_flight: Option<&str>, last_line:
     Option<&str>) -> Foreground` — D3's transport rule. The prompt test is
     a trimmed last row ending in `$`, `#`, `%` or `>` whose preceding
     character is **not** an ASCII digit.
   - `Display` for `Foreground` (`shell prompt`, `running claude (TUI)`,
     `running sleep`, `paused vim`, `exited`, `unknown`) — the wording
     phase 02's panel shows.
7. **Capture the end-to-end evidence** — run the block in § End-to-end
   verification verbatim and paste `/tmp/e2e-01.txt` into a new Update Log
   entry headed `### Update — <date> (end-to-end verification)`, ending with
   the `PASTE MATCH` line. The server-authored `(complete)` entry does not
   satisfy this.

**Gotchas.**
- Read termios with `MasterPty::as_raw_fd()` and `libc::tcgetattr`. Do
  **not** use `MasterPty::get_termios()`: it returns portable-pty's own
  `nix` version's type, which is not this crate's `nix = 0.31`.
- Every `ShellResponse::Status { .. }` literal in the crate must gain the
  three fields; fake backends in tests answer `None` for all three unless
  the test is about them.
- `src/daemon/executor/foreground.rs` is an unrelated existing module. Do
  not touch it.

## Tests

- `screen.rs`: `marked_wraps_an_inverse_run_as_sel`,
  `marked_leaves_plain_text_unmarked`,
  `marked_with_annotated_also_wraps_colour_runs`,
  `scrollback_rows_return_what_scrolled_off`.
- `proto.rs`: `proto_screen_frames_round_trip` (extend the existing one, or
  a new one covering `Screen` and `ScreenText`),
  `proto_status_from_a_pre_phase_host_parses_with_none_fields`.
- `backend.rs`, real PTY: `backend_status_reports_raw_mode_under_stty_raw`
  (while `sleep 5` runs ⇒ `Some(false)`; while `stty raw -echo; sleep 5`
  runs ⇒ `Some(true)`. **Never assert on an idle shell:** an interactive
  bash's readline clears `ICANON` while it waits at the prompt, so an idle
  shell reads `Some(true)` — correct, and the reason D3 only consults
  `raw_mode` when the state is `Running`),
  `backend_status_names_the_foreground_command` (`sleep` ⇒
  `Some("sleep")`; idle ⇒ `None`), `backend_screen_text_marks_inverse_and_reports_alt_screen`
  (one command that enters the alt screen **and then** prints the inverse
  run, then sleeps; type the escapes as `\\033` text for `printf` to
  expand — raw ESC bytes written to the shell are eaten by readline).
- `host.rs`: `host_screen_on_a_backend_without_a_screen_is_an_error`.
- `client.rs`: `client_screen_round_trip`.
- `foreground.rs`: `classify_matches_the_design_matrix` (every local row
  of design § 5, plus `None` and a non-`Status` reply ⇒ `Unknown`),
  `classify_never_calls_a_pre_phase_host_a_prompt`,
  `classify_transport_matches_the_design_matrix` (the three transport
  rows), `classify_transport_prompt_rule` — must match `matt@zoidberg:~$ `,
  `root@box:/# `, `zoidberg% `, `PS C:\> `; must **not** match `Progress
  50%`, `Password:`, `Continue? (y/n)`, an empty line, a blank line.

## Acceptance criteria

- [ ] Each test named in § Tests passes (`cargo test --lib <name>` ⇒
      `test result: ok. 1 passed`).
- [ ] `cargo test` ⇒ `failed=0`, and `passed` is **greater than 2325** (the
      baseline measured 2026-10-13 on `774929c4`).
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` and
      `cargo fmt --all --check` exit 0.
- [ ] `ShellScreen::annotated()` output is unchanged: the existing
      `screen.rs` annotation tests pass unmodified (preservation).
- [ ] `PASTE MATCH` appears in the end-to-end Update Log entry.

## End-to-end verification

The real artifact is the host's wire behaviour over a real PTY, which the
`backend.rs` tests exercise; the block runs them and the gates.

```sh
A=/tmp/e2e-01.txt; : > "$A"
for t in marked_wraps_an_inverse_run_as_sel marked_leaves_plain_text_unmarked marked_with_annotated_also_wraps_colour_runs scrollback_rows_return_what_scrolled_off proto_status_from_a_pre_phase_host_parses_with_none_fields backend_status_reports_raw_mode_under_stty_raw backend_status_names_the_foreground_command backend_screen_text_marks_inverse_and_reports_alt_screen host_screen_on_a_backend_without_a_screen_is_an_error client_screen_round_trip classify_matches_the_design_matrix classify_never_calls_a_pre_phase_host_a_prompt classify_transport_matches_the_design_matrix classify_transport_prompt_rule; do cargo test --lib "$t" 2>&1 | grep -E '^test result' | sed "s/^/$t: /" >> "$A"; done
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1 >> "$A"
cargo fmt --all --check; echo "fmt_exit=$?" >> "$A"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}' >> "$A"
cat "$A"
```

Run the full `cargo test` line **once**. If it reports failures, re-run at
most twice more, then paste the last capture as it is and name every
failing test; a failing test this phase added is the first suspect.

Then the self-check, whose output line goes into the entry:

```sh
D=docs/dev/milestones/M28-interactive-shells/phase-01-host-truth.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-01.txt
diff /tmp/pasted-01.txt /tmp/e2e-01.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

## Authorizations

None.

## Out of scope

- Any model-facing tool, prompt asset, `CLAUDE.md` or README change — 01b.
- `ShellRecord` fields (`last_foreground`, `in_flight`) — 01b.
- Any change to shell reuse, `run_terminal_command`, `send_input` or the
  `interactive` flag — phase 02.
- Changing `PaneStatus` or `src/shell/status.rs` (the tmux classifier).

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Notes for executor — 2026-10-13

The first run (GLM-5.3-Flash) stalled after 120 read-only calls probing
termios. **The stall was the spec's fault, not yours.** The spec asked
`backend_status_reports_raw_mode_under_stty_raw` to assert that a fresh
idle shell is not raw. That is false for an interactive bash: readline
clears `ICANON` while it waits at the prompt. The § Tests bullet is now
corrected. Your `PtyShell::raw_mode()` is right as written — do not change
it or probe termios again.

**Already done and correct — do not redo:** Tasks 1–4. `marked()`,
`scrollback_rows()`, the `Screen` / `ScreenText` frames, the three
optional `Status` fields, `ShellBackend::screen_text` with its default,
`PtyBackend` and `SbxBackend` implementations, `raw_mode()`,
`foreground_command()`. The build is clean and 380 of 382 `shell::` tests
pass.

**Exactly this remains:**

1. Fix `backend_status_reports_raw_mode_under_stty_raw` per the corrected
   § Tests bullet: drop the idle assertion; assert `Some(false)` while a
   plain `sleep` runs.
2. Fix `backend_screen_text_marks_inverse_and_reports_alt_screen`: it
   writes raw ESC bytes (readline eats them) and prints the inverse run
   before entering the alt screen (which hides it). Send one command such
   as `printf '\\033[?1049h\\033[7mSELECTED\\033[0m plain\\n'; sleep 30`
   followed by a newline.
3. Delete the three debug tests `probe_sb8`, `probe_sb9`, `probe_sb10` in
   `src/shell/screen.rs`.
4. Task 3's missing test: `proto_status_from_a_pre_phase_host_parses_with_none_fields`.
5. Task 4's missing test: `host_screen_on_a_backend_without_a_screen_is_an_error`.
6. Task 5 entirely: `client::screen(...)` and `client_screen_round_trip`.
7. Task 6 entirely: `src/shell/foreground.rs` and its four tests.
8. Task 7: the end-to-end capture and `PASTE MATCH`.

Finish condition: every test in § Tests passes, and `cargo test` reports
`failed=0` with `passed` above 2325.

### Update — 2026-10-13 (escalation)

**Chosen lever:** resume
**Rationale:** Tasks 1–4 are on disk and correct, and the stall came from one impossible assertion the spec dictated; with that assertion corrected in the doc, a resumed run keeps the finished work and has eight enumerated items left, which a fresh re-dispatch would redo.

### Update — 2026-10-13 12:01 (progress)

Started the phase (executor: Claude Opus 4.6). Status flipped to in-progress and
the milestone README row updated. Reading the design (`interactive-shells.md` §
1, D1–D3, § 5), `screen.rs`, `proto.rs`, `host.rs`, `backend.rs`, `client.rs`
and `pty.rs` before editing. No code yet.
