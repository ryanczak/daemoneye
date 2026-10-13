# Interactive Shells

**Status:** design, 2026-10-13 — promoted from `docs/proposal/read-screen.md`
(scoped the same day) with the PE's three decisions applied: this is **M28**
and the roadmap's M28–M30 shift down by one; the static `interactive` flag is
**replaced** by derived live state; `annotated` defaults to **false**.
**Amended:** 2026-10-13 — transport shells (D3, D4, § 5): a shell whose
child is `ssh` or `sbx exec` is never classified from the local terminal.
Same day: the five coverage caveats became decisions — selection markers
(D1, D7), settle that cannot settle (D7), modifier keys and paste (D8),
geometry (D10).
**Drives:** M28 (release `2.1.0`), three phases (§ 6).
**Sibling:** [`daemoneye-2.0.md`](daemoneye-2.0.md) § 6 — the roadmap row.

The goal: an interactive TUI — Claude Code is the reference case — can run
inside a DaemonEye PTY shell, launched by the user from attached mode or by
the model, and the model can **see** it, **type into** it safely, **wait**
on it, and never **poison** it with a command meant for a prompt.

---

## 1. The incident

Session `a9aac6a561c720a0`, 2026-10-13. The user attached to the daemon's
shell `s0` and launched Claude Code in it. The model's next bare
`run_terminal_command` would have reused `s0` — its lowest-numbered shell —
and typed the marker wrapper into Claude Code's prompt. The model avoided
that only because it read the asciicast by hand and recognised Claude Code's
glyphs. Nothing in the tool layer could have told it. The three facts that
decide the design were established in that session and the one after it:

1. **The daemon cannot see the screen.** `ShellScreen` (`src/shell/screen.rs`,
   a `vt100::Parser`) lives in the detached `daemoneye shell-host` process.
   The daemon reaches a shell only over `var/run/shells/<id>.sock`
   (`src/shell/proto.rs`). The CLI's `/tail` rebuilds the screen from
   `SubscribeScreen` bytes; no model-facing tool reads it at all.
2. **Alt-screen is the wrong signal for Claude Code.** Mode toggles counted
   in `s0-1791861681.cast`: alt screen (`?1049h`) **1**, bracketed paste
   (`?2004h`) **47**, cursor hide (`?25l`) **47**, mouse (`?1000h`) **7**.
   Claude Code is an inline Ink app; `is_alt_screen()` is false for its
   whole life. The robust signal is **raw-mode termios**: a probe confirmed
   that on Linux `tcgetattr` on the PTY *master* reports the slave's
   `ICANON`/`ECHO`, so the host reads it from the fd it already owns.
3. **The static `interactive` flag is a command-name guess.** It is set only
   by `is_interactive_command` (ssh, mosh, telnet, screen) on the
   `run_terminal_command` path, never by anything the user types while
   attached, and never cleared — an ssh that exited leaves the shell
   unusable forever.

## 2. What breaks today

| Path | What happens | Why |
|---|---|---|
| bare `run_terminal_command` while a TUI holds the owner's lowest shell | wrapper typed into the TUI | reuse predicate reads the static flag, not the terminal |
| `shell: "s0"` naming a busy shell | same, then a 45 s timeout and **SIGTERM to the foreground group** | no busy check; the timeout path assumes a hung command |
| `run_terminal_command("claude")` | marker-wrapped, waits for an END that never comes, **kills Claude Code** on timeout | only ssh/mosh take the no-wait path |
| `send_input(text, enter: true)` to a raw-mode app | app sees `\n`; Ink's return key is `\r` | `\n` is only right for a cooked line discipline |
| `send_input` needing Escape, arrows, Shift-Tab | model must emit raw escape bytes in JSON | no named keys; the encoder exists only in `cli/attach.rs` |
| `watch_shell(pattern)` on TUI output | SGR codes split words; the pattern misses | matches raw bytes, not the rendered screen |
| "what is this shell showing?" | unanswerable | no screen read |

## 3. Non-goals

- No daemon-side VT parser: the host owns the one screen model.
- No cross-shell screen aggregation (`find_in_shells` covers the logs).
- No writes to the screen model; it stays a projection of the PTY bytes.
- No change to the cast log, the marker protocol or `wrap_command`.
- No change to the attached-mode client beyond sharing its key encoder.
- Sandboxed (`sbx`) and `[hosts.*]` shells get the same host frames because
  the same host serves them; nothing here is sandbox-specific.

## 4. Decisions

### D1 — The screen stays in the host; a `Screen` frame carries text out

`ShellRequest::Screen { annotated: bool, scrollback: u16 }` →
`ShellResponse::ScreenText { text, rows, cols, cursor: (u16, u16),
alt_screen: bool }`. The host renders under its existing
`Mutex<ShellScreen>`: `contents()` or `annotated()`, with the last
`scrollback` rows that scrolled off the top prepended (vt100 already keeps
`[shells] scrollback_lines`; Claude Code's transcript lives there). No
daemon-side parser, so the screen a tool reports and the screen `/tail`
draws can never disagree in substance.

**Selection markers are always on.** Independently of `annotated`, a run of
cells in **inverse video** is wrapped as `[SEL: …]`. Inverse is how
ncurses menus, `dialog`, `whiptail`, `less`'s status line and most pickers
mark the selected row, and plain `contents()` drops it; colour markers
(`annotated`) are semantics the model opts into, inverse is structure it
always needs. `ShellScreen::marked()` generalises `annotated_row` over
`vt100::Cell::inverse()`; the frame's `annotated` flag only adds the
colour runs on top.

### D2 — `Status` grows the three foreground facts

`ShellResponse::Status` gains `raw_mode: bool` (termios on the master fd,
`ICANON` off), `alt_screen: bool` (`ShellScreen::is_alt_screen`) and
`foreground: Option<String>` (basename of the foreground group leader's
`/proc/<pgid>/cmdline`, `None` when the group is the shell's own). All three
are `#[serde(default)]` so a host from before this milestone still answers
and classifies as `Unknown`, never as a prompt.

### D3 — One pure classifier: `Foreground`

```
enum Foreground {
    Prompt,                                   // Idle: the shell owns the terminal
    Program { cmd: String, tui: bool },       // Running: tui = raw_mode || alt_screen
    Paused  { cmd: String },                  // a stopped job
    Exited,
    Unknown,                                  // status unreadable or pre-D2 host
}
```

`Foreground::classify(state, raw_mode, alt_screen, foreground)` in
`src/shell/foreground.rs`, table-tested over every row of § 5. `PaneStatus`
(`src/shell/status.rs`) is the **tmux** classifier and is not touched.

**Transport shells are classified by the daemon, not the terminal.** A
record with a named `host` has `ssh` as its child; a record with a
`sandbox` has `sbx exec -it`. Both put the local pty slave in raw mode for
the life of the session and hold the foreground group throughout, so the
local terminal reports `Program { cmd: "ssh", tui: true }` from the first
byte to the last, whatever the remote side is doing. For those records
`raw_mode`, `alt_screen` and `foreground` are **ignored** and
`Foreground::classify_transport(state, in_flight, last_line)` decides:

- `Exited` → `Exited`;
- `in_flight` set (D4: the daemon has a marker-wrapped command outstanding
  on this shell) → `Program { cmd: <that command>, tui: false }`;
- otherwise the last non-blank rendered row from a `Screen` read ends in a
  prompt character (`$`, `#`, `%`, `>` after optional whitespace) →
  `Prompt`;
- otherwise → `Program { cmd: "<remote>", tui: true }`: something the
  daemon did not start holds the remote terminal, which is exactly the
  user-launched-Claude-Code-over-ssh case.

The transport rule is the conservative one: it can only produce a false
"busy" when a remote prompt does not end in one of the four characters,
and the cost of that is a fresh shell on the same host.

### D4 — The static `interactive` flag is removed

`ShellRecord.interactive`, `mark_interactive`, the `[interactive]` flag in
`list_shells` / `[SHELLS]` / `ShellInfo` and the `/shells` column all go.
In their place the record carries an **observation** and a **fact**:

- `last_foreground: Option<(Foreground, u64)>` — refreshed by every
  `Status` probe the daemon makes (reuse decision, explicit-shell check,
  `send_input` guard, `read_shell_screen`, `list_shells`, run completion).
  Never asserted, only observed, and it carries its timestamp.
- `in_flight: Option<InFlight { cmd: String, nonce: Nonce, started_unix }>`
  — set by `shell_exec` **before** the wrapped command is written and
  cleared on every exit of `run_command` (outcome, timeout, prompt given
  up, shell exited). This is the daemon's own knowledge that it is using
  the shell; it is what the transport rule in D3 reads, and it also lets a
  local shell refuse a second command while the first is still running
  without a `Status` round-trip. Interactive launches (D6) set it for the
  duration of the settle and clear it: after that the shell is whatever the
  program made it, and the terminal — or the transport rule — says so.

- **Decisions probe live.** `shell_for_owner` and the explicit `shell:` path
  call `Status` before handing a shell over; a `Program`/`Paused` shell is
  skipped (reuse) or refused (explicit, § D5). The flock liveness probe
  already lives there, so this is the same I/O class.
- **Prompt assembly reads the cache.** `[SHELLS]` / `[DEFAULT SHELL]` keep
  their no-I/O contract and render `last_foreground` as `[running claude]`
  / `[paused vim]` / nothing, with the 08a caveat extended: the block can be
  one observation stale, the decision never is.
- `is_interactive_command` survives only as the auto-upgrade in D6.
- **Transport records never consult the terminal.** The probe for a named
  `host` or a `sandbox` record is `in_flight` plus one `Screen` read for
  the last row (D3's transport rule); `raw_mode` and `foreground` are
  read and discarded. This is the rule that keeps `[hosts.*]` and sbx
  shells reusable at all.

### D5 — A busy shell refuses a command

`run_terminal_command` naming (or defaulting to) a shell whose live
`Foreground` is `Program` or `Paused` is refused before any byte is
written: *"shell s0 is running `claude` (raw-mode TUI); a command written
there would be typed into it. Use `send_input` to type into that program,
`read_shell_screen` to see it, or `shell: "new"` for a fresh shell."* The
45 s timeout and its SIGTERM are therefore reached only by a command the
shell actually started.

### D6 — Launching a TUI is a first-class, no-wait run

`run_terminal_command` gains `interactive: bool` (default `false`). When
true: the bare command plus `\r` is written (no markers), the call sleeps
`settle_ms` (default 500, cap 5000), probes `Status`, and returns the
`Foreground` plus the masked screen (`read_shell_screen` semantics, D7).
It never waits for an exit and never signals the foreground group. The
approval panel says `interactive: the command keeps the shell; nothing
waits for it`. A command `is_interactive_command` recognises is upgraded to
this path automatically with a note in the result, replacing today's
`[Interactive session started]` text. `wait: false` with `interactive`
stays refused, as the dedicated-shell path refuses ssh today.

### D7 — `read_shell_screen`: read-only, ungated, masked, can wait

```
read_shell_screen(
  shell        (required)                   id from list_shells
  lines        (optional, default all)      tail N visible rows
  scrollback   (optional, default 0)        prepend N rows that scrolled off
  pattern      (optional)                   regex filter on the rendered rows
  wait_for     (optional)                   regex; block until the screen shows it
  settle_ms    (optional)                   block until no output for this long
  timeout_secs (optional, default 30, cap 300)  bound on wait_for / settle_ms
  annotated    (optional, default false)    [OK/WARN/ERROR] runs
)
```

Same trust class as `read_shell`: not approval-gated,
`should_emit_tool_feedback() == true`, output through `mask_sensitive`,
exited-and-reaped shell refused with `read_shell`'s message shape. The
header names the `Foreground`, the size and whether the result is a wait
that timed out. `wait_for` matches the **rendered** screen, which is what
makes it work on TUI output where `watch_shell`'s byte match does not;
`settle_ms` is "the app finished drawing", the right primitive for "Claude
Code finished its turn". `annotated` stays false by decision: a TUI's
colours are not OK/WARN/ERROR semantics.

**A settle that cannot settle is a result, not an error.** `top`, clocks
and spinners redraw forever. Quiescence is measured on the **rendered
text** (two consecutive `Screen` reads `settle_ms` apart that are equal),
not on the byte stream, so a cursor blink or an SGR-only redraw settles and
only a visible change does not. When `timeout_secs` elapses first the tool
returns the current screen with the header `did not settle within N s —
the screen is still changing; use wait_for for a pattern that marks the
state you need`. The tool description names `top` as the example so the
model reaches for `wait_for` on the first call, not the second.

### D8 — `send_input` sends `\r` and takes named keys

- `enter: true` appends `\r`, not `\n`. A cooked line discipline turns it
  into `\n` (`ICRNL`), a raw-mode app gets the byte a real Enter sends. The
  `wrap_command` terminator is unchanged (it never targets a raw-mode app).
- New `keys: [String]`, tmux's key vocabulary: `Enter`, `Escape`, `Tab`,
  `BTab`, `Up`/`Down`/`Left`/`Right`, `Home`, `End`, `PageUp`, `PageDown`,
  `Delete`, `BSpace`, `F1`–`F12`, `C-<x>`, `M-<x>`. Encoded by
  `src/shell/keys.rs`, where `attach_keys`'s application-cursor rewrite
  moves from `cli/attach.rs` so attached mode and the tool agree; the modes
  come from the `Status`/`ScreenText` reply. `text` is sent first, then
  `keys`, then Enter if `enter`. An unknown key name is an error naming the
  vocabulary; nothing is sent.
- **Modifier grammar.** `C-`, `M-` and `S-` compose: `S-Enter`, `C-S-p`,
  `M-Enter`. Combinations a legacy terminal has no byte for — `S-Enter`,
  `C-Enter`, `S-Tab` already being `BTab`, `C-S-<x>` — encode as **CSI u**
  (`ESC [ <codepoint> ; <modifier> u`, `S-Enter` = `ESC [ 13 ; 2 u`), the
  kitty keyboard protocol's legacy-compatible form. Whether the program
  honours CSI u is the program's business; the tool's result says which
  keys went out as CSI u so the model can read the screen and tell. The
  phase 03 live close checks `S-Enter` against Claude Code's own
  `/terminal-setup` expectation.
- **Paste.** `paste: bool`, default **true when `text` contains a newline
  and the shell has bracketed paste on**, else false. When true the text is
  wrapped in `ESC [ 200 ~` / `ESC [ 201 ~` so a multi-line prompt reaches
  Claude Code as one paste instead of a submit per line; when the shell has
  not asked for bracketed paste the brackets are never sent, the same rule
  `attach_keys` applies. `paste: true` with the mode off is an error naming
  the mode, not a silent downgrade.

### D9 — The guard, the panel and the ghost refusal

`send_input` probes `Status` and classifies **before** the approval prompt:

| `Foreground`          | chat                                    | ghost |
|-----------------------|-----------------------------------------|-------|
| `Prompt`              | allow — panel: *will run as a command*  | allow if `ToolPolicy` explicitly allows `send_input` |
| `Program { tui: false }` | allow — panel: *stdin of `cmd`*      | **refuse** unless runbook `allow_tui_input: true` |
| `Program { tui: true }`  | allow — panel: *raw-mode TUI `cmd`* + 5-row masked screen tail | **refuse** unless `allow_tui_input: true` |
| `Paused`              | allow — panel: *job is stopped*         | refuse |
| `Exited` / `Unknown`  | allow — panel: *may be gone*            | refuse |

In chat the guard informs and the human decides; the panel's `foreground:`
line and screen tail are the blast radius made visible. For a ghost the
default is refuse; the refusal names the shell, the classification and
`allow_tui_input` as the runbook field that lifts it. The existing
`ToolPolicy` explicit-allow stays a prerequisite — `allow_tui_input` widens
*where* a permitted ghost may type, never *whether* it may.

### D10 — Geometry is configurable and a shell can be opened to size

Shells spawn at 24×80 today, which cramps every modern TUI. `[shells]`
gains `rows` (default **40**) and `cols` (default **120**); every spawn
path (`shell_for_owner`, `open_shell`, the dedicated background shell, the
sandbox and host shells) reads them. `open_shell` gains optional `rows` /
`cols` so the model can open a wide shell for a wide app; the result names
the size. The existing `Resize` frame is unchanged and attached mode still
resizes to the user's terminal, so a shell the user attaches to follows the
human, and one the model drives keeps its configured size. Tests that pin
24×80 pass the size explicitly rather than inheriting the default.

## 5. The matrix the tests pin

| state   | raw | alt | fg        | `Foreground`                     | reuse | explicit `shell:` | ghost `send_input` |
|---------|-----|-----|-----------|----------------------------------|-------|-------------------|--------------------|
| Idle    | f   | f   | —         | `Prompt`                         | yes   | run               | allow (policy)     |
| Running | f   | f   | `sleep`   | `Program{tui:false}`             | skip  | refuse            | refuse             |
| Running | t   | f   | `claude`  | `Program{tui:true}`              | skip  | refuse            | refuse             |
| Running | t   | t   | `vim`     | `Program{tui:true}`              | skip  | refuse            | refuse             |
| Running | f   | t   | `less`    | `Program{tui:true}`              | skip  | refuse            | refuse             |
| Paused  | —   | —   | `sleep`   | `Paused`                         | skip  | refuse            | refuse             |
| Exited  | —   | —   | —         | `Exited`                         | skip  | refuse            | refuse             |
| (no answer / old host) | | |      | `Unknown`                        | skip  | refuse            | refuse             |
| **transport**, Running | t | f | `ssh` | — ignored; `in_flight` set → `Program{tui:false}` | skip | refuse | refuse |
| **transport**, Running | t | f | `ssh` | — ignored; no `in_flight`, last row `…$ ` → `Prompt` | yes | run | allow (policy) |
| **transport**, Running | t | f | `ssh` | — ignored; no `in_flight`, last row not a prompt → `Program{tui:true}` | skip | refuse | refuse |

`Unknown` is treated as busy everywhere a decision is made: the cost of a
false "busy" is a fresh shell, the cost of a false "prompt" is a poisoned
program. The three **transport** rows are the D3 transport rule and are
pinned by the same table test; a local record never takes them and a
transport record never takes the rows above them.

## 6. Phases

| Phase | Delivers | Exit |
|---|---|---|
| **01 — Host truth** | D1, D2, D3: the `Screen` frame, the three `Status` fields (raw mode read from the master fd), `src/shell/foreground.rs` with both `classify` and `classify_transport` + the § 5 table test, `read_shell_screen` without `wait_for`/`settle_ms` but with the `[SEL: …]` inverse markers, `last_foreground` and `in_flight` on the record (D4, flag still present). Tool count 42→43 in `CLAUDE.md`/`README.md`; `sre.toml` documents the tool. | A shell running `less` and one at a prompt: `read_shell_screen` shows the frame and the prompt, and `less`'s inverse status line comes back as `[SEL: …]`; `Status` reports `raw_mode`/`alt_screen`/`foreground` for each; a pre-D2 `Status` JSON parses as `Unknown`; a shell on `zoidberg` at its remote prompt classifies `Prompt` while its local terminal reports raw mode. |
| **02 — Safety** | D4 (flag removed), D5, D8, D9: live probes in `shell_for_owner` and the explicit path, the busy refusal, `\r` + `keys` via `src/shell/keys.rs` shared with `cli/attach.rs`, the guard + panel + ghost refusal + `allow_tui_input`. | Live: Claude Code launched from attached mode in the owner's lowest shell; a bare `run_terminal_command` lands in a fresh shell; `shell: "s0"` is refused naming `claude`; `send_input(keys: ["BTab"])` cycles its mode and the panel showed the screen tail; a ghost with no `allow_tui_input` is refused; `run_terminal_command(host: "zoidberg")` twice in a row reuses the same host shell. |
| **03 — Launch & wait** | D6, D7's `wait_for`/`settle_ms`/`scrollback` with the rendered-text quiescence rule and the did-not-settle result, D8's modifier grammar (CSI u) and `paste`, D10's `[shells] rows`/`cols` and `open_shell(rows, cols)`: `interactive: true`, the ssh auto-upgrade replacing the `[Interactive session started]` path, screen waits. | Live close (M14 convention): the model launches Claude Code with `interactive: true`, types a prompt with `send_input`, `read_shell_screen(settle_ms: 1500)` returns the finished answer, `scrollback` returns the part that scrolled off; a two-line prompt goes in as one bracketed paste; `S-Enter` goes out as CSI u and the screen shows what Claude Code made of it; `read_shell_screen(settle_ms: 500)` on `top` returns the did-not-settle result inside `timeout_secs`; a shell opened with `cols: 160` runs `htop` at that width; `ssh zoidberg` goes through the auto-upgrade and the shell is reusable again after `exit`. |

Phase 01 is independently shippable and has no behaviour change for a model
that does not call the new tool. Phase 02 is the hazard fix. Phase 03 is
the TUI-as-a-first-class-citizen step and the milestone's live close.

## 7. Safety

- **The read path is read-only.** `Screen` renders under the host's lock and
  never feeds, resizes or subscribes.
- **Masking is mandatory** on every screen byte that reaches the model,
  including the panel tail and the D6 launch result.
- **The screen is not the record.** The tool description says so: cite the
  cast log (`read_shell` / `read_shell_log`) for anything audited.
- **Nothing widens what a ghost may type.** D9 only narrows; `allow_tui_input`
  is a named opt-in in the runbook, mirroring `auto_approve_scripts`.
- **Fail busy.** `Unknown` refuses commands and ghost input; it never
  defaults to "prompt".
- **Keys are bytes the user could type.** `keys` encodes exactly what a
  terminal sends; there is no key that reaches the host's control plane
  (pause, cancel, close stay on `ShellSignal` / `Close`).

## 8. Open items (settled at phase scoping, not here)

- **Panel tail length.** D9 says 5 rows. Tune once it renders.
- **`watch_shell` and `wait_for` overlap.** `watch_shell` stays the
  byte-stream primitive; if phase 03 shows `wait_for` covering every real
  use, retiring `watch_shell`'s `pattern` is a post-M28 follow-on.

## 9. What stays out of reach

Known after the decisions above, so nobody rediscovers them live:

- **Mouse.** Claude Code and others enable tracking; no tool sends mouse
  events and nothing here plans to. Keyboard covers every flow that matters.
- **Colour-only selection.** `[SEL: …]` marks inverse video. `fzf` and
  some pickers mark the current row with colour alone; `annotated: true`
  then shows the run as `[OK/WARN/ERROR: …]` only when the colour happens
  to be one of the three. The cursor position in the header is the
  fallback.
- **CSI u depends on the program.** `S-Enter` goes out correctly; a program
  that does not speak CSI u sees an escape sequence it ignores or
  misreads. The result says the key went out as CSI u, the screen says
  whether it landed.
- **Exotic remote prompts.** The transport rule reads a last row ending
  in `$ # % >` as a prompt; a remote prompt ending in anything else reads
  as busy and gets a fresh shell on that host. Correct, but one shell per
  command until the prompt is changed.
- **Transport-shell foregrounds are nameless.** Over ssh or sbx the
  daemon cannot name the remote program; the classification is
  `Program { cmd: "<remote>" }`. The screen read is how the model finds
  out what it is.
- **The ambient block is one observation stale** by design (D4); a shell
  that became busy since the last probe still shows as the default until
  the next decision probes it. Decisions never use the cache.
- **Box-drawing TUIs are token-heavy.** A 40×120 frame of `htop` is the
  full grid every read; `lines` and `pattern` are the cost controls, there
  is no semantic compaction.
- **Ghost shells drive TUIs only by opt-in** (`allow_tui_input`), and a
  ghost never sees the approval panel's screen tail — it reads the screen
  itself or not at all.
