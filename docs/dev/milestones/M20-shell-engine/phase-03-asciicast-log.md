# Phase 03: asciicast v2 shell log and its command index

**Milestone:** M20 — Shell Engine
**Status:** in-progress
**Depends on:** none in code (phase-02's `src/shell/` exists; this phase adds a
sibling module and does not call it)
**Estimated diff:** ~420 lines
**Tags:** language=rust, kind=feature, size=m

## Goal

Add `src/shell/log.rs`: an **asciicast v2** writer for a shell session, a
`.meta.json` sidecar indexing each command by byte range and exit code, and a
reader that returns the output bytes of command N as an O(1) file slice.

Everything is pure over byte streams and an injected timestamp — no PTY, no
clock call, no `Instant::now()` inside the module. Nothing calls it yet;
phase-05 (the shell-host) is its first consumer.

## Architecture references

Read before starting:

- `docs/design/daemoneye-2.0.md` § 2.1, the "Recording format" paragraph — why
  asciicast v2 and what the `"m"` events are for.
- `docs/dev/milestones/M20-shell-engine/README.md` § "Design decisions on
  record" — the `.meta.json` sidecar is derived from the cast and rebuildable
  from it, so it is an index and never a second source of truth.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any code.
4. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

- `src/shell/` holds `mod.rs` (11 lines) and `pty.rs` (576 lines) from
  phase-02. **`src/shell/log.rs` does not exist.**
- `src/shell/mod.rs` currently reads, in full:

  ```rust
  //! Shell engine building blocks shared by the M20 phases.
  //!
  //! `pty.rs` holds the two primitives every later phase builds on: a PTY-backed
  //! shell spawned through `portable-pty`, and the marker protocol that returns a
  //! command's real exit code and its exact output bytes.

  mod pty;

  pub use pty::{
      CommandOutcome, Nonce, PtyShell, exit_var, parse_outcome, strip_markers, wrap_command,
  };
  ```

- `config::shell_logs_dir()` (`src/config/load.rs:45`) resolves
  `~/.daemoneye/var/log/shells/`; phase-01 created it and put it under the
  lifecycle policy. **This phase does not write there** — every test uses a
  `tempfile::tempdir()`. The directory becomes live in phase-05.
- `serde` and `serde_json` are already dependencies. `tempfile` is already a
  dev-dependency. **No new dependency is needed or authorized.**

## Reference — the asciicast v2 format (fetched 2026-09-03; the executor cannot reach the web)

This is the whole contract. Build to it exactly.

A `.cast` file is **newline-delimited JSON**, not a JSON document. Line 1 is
the header object; every later line is a 3-element array.

**Header** — required `version` (integer `2`), `width` (columns), `height`
(rows). Optional and used by this phase: `timestamp` (integer unix seconds at
session start). Minimal valid header:

```json
{"version": 2, "width": 80, "height": 24}
```

**Event lines** — `[time, code, data]`:

- `time` — **float, seconds since session start** (not absolute, not
  milliseconds).
- `code` — a one-character string.
- `data` — a JSON string.

**Event codes**, with the spec's own examples:

| code | meaning | example |
|---|---|---|
| `"o"` | output printed to the terminal | `[5.0, "o", "hello"]` |
| `"i"` | input, i.e. keystrokes sent to the terminal | `[5.0, "i", "h"]` |
| `"m"` | marker; `data` is a label string, possibly empty | `[10.0, "m", "Configuration"]` |
| `"r"` | resize; `data` is `"{COLS}x{ROWS}"` | `[5.0, "r", "100x50"]` |

`"r"` is **out of scope for this phase** — phase-05 owns resize.

## Measured facts — executed 2026-09-03, not reasoned about

### F1. `serde_json` already escapes every control byte we emit

Serialising the Rust string `"a\u{1f}b\r\n\u{1b}[31mred\u{1b}[0m"` produced, exactly:

```
"a\u001fb\r\n\u001b[31mred\u001b[0m"
```

So the marker protocol's unit-separator (`0x1f`) and ANSI escapes (`0x1b`)
survive a JSON round trip as `\u001f` / `\u001b`, and CR/LF as `\r` / `\n`.
**Do not hand-escape anything** — `serde_json::to_string` on the payload string
is correct and complete.

### F2. Float times serialise with a decimal point

`serde_json` rendered `0.0`, `0.123457`, `1.5`, `12.0` — never a bare integer.
That satisfies the spec's "float" requirement with no special handling.
Rounding to 6 decimal places (`(secs * 1e6).round() / 1e6`) is what produced
`0.123457` from `0.123456789`; do the same so lines stay short and stable.

### F3. **The load-bearing one: a PTY read splits multi-byte characters, and `from_utf8` tells you which kind of failure it is.**

Phase-02 measured that chunking a 4942-byte UTF-8 stream at 4096 bytes leaves
**both** chunks invalid on their own while the whole buffer is valid. Since
asciicast `data` must be a JSON *string*, the writer cannot simply decode each
chunk.

`std::str::from_utf8` distinguishes the two cases. Measured on the bytes of
`"é世界😀"`:

| input | result |
|---|---|
| first 1 byte | `Err` `valid_up_to=0` **`error_len=None`** |
| first 2 bytes | `Ok("é")` |
| first 3 bytes | `Err` `valid_up_to=2` **`error_len=None`** |
| first 5 bytes | `Ok("é世")` |
| first 8 bytes | `Ok("é世界")` |
| first 11 bytes | `Err` `valid_up_to=8` **`error_len=None`** |
| the bytes `0xff 0x41` | `Err` `valid_up_to=0` **`error_len=Some(1)`** |

**`error_len() == None` means the trailing bytes are an *incomplete* sequence —
carry them to the next write. `error_len() == Some(n)` means they are
*genuinely invalid* — they will never become valid, so they must be consumed,
not carried.** Carrying an invalid byte forever is the failure mode this table
exists to prevent.

## Spec

### Task 1 — Create `src/shell/log.rs` and declare it

Create the file and add `mod log;` to `src/shell/mod.rs` beside the existing
`mod pty;`, re-exporting the public items named below in the same
`pub use` style the file already uses for `pty`.

### Task 2 — `CastWriter`

```rust
/// Writes an asciicast v2 recording, one JSON line per record, flushed per
/// record so a live tail sees motion immediately.
pub struct CastWriter { /* file, utf8 carry buffer */ }
```

- `CastWriter::create(path: &Path, cols: u16, rows: u16, started_unix: u64) -> anyhow::Result<Self>`
  creates the file and writes the header line plus a newline. The header must
  serialise exactly these four fields: `version` (always `2`), `width`,
  `height`, `timestamp`. Use a `#[derive(Serialize)]` struct, not hand-built
  JSON.
- `write_output(&mut self, at: Duration, bytes: &[u8]) -> anyhow::Result<()>`
  emits `[t, "o", data]`.
- `write_input(&mut self, at: Duration, bytes: &[u8]) -> anyhow::Result<()>`
  emits `[t, "i", data]`.
- `mark(&mut self, at: Duration, label: &str) -> anyhow::Result<()>`
  emits `[t, "m", label]`.
- `byte_len(&self) -> u64` — bytes written so far, i.e. the offset the **next**
  line will start at. The command index (Task 3) is built from this.
- Every method flushes before returning.

**Time.** `at` is supplied by the caller — the module never reads a clock.
`t` is `at.as_secs_f64()` rounded to 6 decimal places (F2).

**The UTF-8 carry (F3).** `write_output` / `write_input` must not lose or
corrupt a split character. Required behaviour:

1. Prepend any carried bytes from the previous call to `bytes`.
2. `std::str::from_utf8` on the result.
   - `Ok(s)` → emit `s`, carry nothing.
   - `Err(e)` with `e.error_len() == None` → emit the valid prefix
     (`&buf[..e.valid_up_to()]`), **carry the remainder** for the next call.
   - `Err(e)` with `e.error_len() == Some(n)` → emit a lossy conversion of the
     whole buffer (which substitutes `U+FFFD` for the invalid run) and carry
     nothing. Never carry a byte the error called invalid.
3. If nothing is left to emit after this (the whole input was carried), write
   **no line at all** rather than an empty `"o"` event.
4. A carry never needs to exceed 3 bytes — a 4th byte makes the sequence either
   decodable or invalid. Do not assert this; just let the logic make it true.

Worked example of the required shape:

```rust
let mut buf = std::mem::take(&mut self.carry);
buf.extend_from_slice(bytes);
let (text, carry): (String, Vec<u8>) = match std::str::from_utf8(&buf) {
    Ok(s) => (s.to_string(), Vec::new()),
    Err(e) if e.error_len().is_none() => {
        let valid = e.valid_up_to();
        (
            String::from_utf8_lossy(&buf[..valid]).into_owned(),
            buf[valid..].to_vec(),
        )
    }
    Err(_) => (String::from_utf8_lossy(&buf).into_owned(), Vec::new()),
};
self.carry = carry;
if text.is_empty() {
    return Ok(());
}
```

### Task 3 — The `.meta.json` index

```rust
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct CommandRecord {
    pub index: u32,
    pub command: String,
    pub started: f64,          // seconds since session start
    pub ended: f64,
    pub exit_code: i32,
    pub first_byte: u64,       // offset of this command's first event line
    pub end_byte: u64,         // offset just past its last event line
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct MetaIndex {
    pub shell_id: String,
    pub cast: String,          // the .cast file's basename
    pub started_unix: u64,
    pub commands: Vec<CommandRecord>,
}
```

- `MetaIndex::save(&self, path: &Path) -> anyhow::Result<()>` — pretty JSON.
- `MetaIndex::load(path: &Path) -> anyhow::Result<Self>`.
- `meta_path_for(cast: &Path) -> PathBuf` — the sidecar beside the cast, with
  a `.cast` extension replaced by `.meta.json`. For
  `…/s7-1788470000-build.cast` this is `…/s7-1788470000-build.meta.json`.
  **Pin the negative case:** a path whose extension is not `.cast` gets
  `.meta.json` **appended**, so `…/x.log` becomes `…/x.log.meta.json` rather
  than losing its `.log`.

### Task 4 — The reader

```rust
/// Return the concatenated `"o"` payload bytes of the command at `index`,
/// reading only that command's byte range from the cast file.
pub fn read_command_output(cast: &Path, meta: &MetaIndex, index: u32)
    -> anyhow::Result<Vec<u8>>
```

- Look the record up by its `index` **field**, not by position in the vector —
  a later phase may prune. A missing index is an `Err`, not an empty `Vec`.
- Seek to `first_byte`, read `end_byte - first_byte` bytes, parse those lines.
- Keep only `"o"` events; concatenate their payloads as UTF-8 bytes. Skip
  `"i"` and `"m"` lines.
- A malformed line inside the range is skipped, not fatal — the log is
  append-only and a torn final write must not make the whole slice unreadable.

### Task 5 — Write the tests named in § Test plan

Hermetic: `tempfile::tempdir()` for every file. No PTY, no daemon, no network.
No test may write under `~/.daemoneye/`.

### Task 6 — Capture the end-to-end evidence

Run the block in § End-to-end verification **verbatim and unmodified**, then
paste the resulting `/tmp/e2e-03.txt` into a new Update Log entry headed
`### Update — <date> (end-to-end verification)`. The server-authored
`(complete)` entry does not satisfy this. Then run the PASTE MATCH self-check
in that same section and paste **the literal verdict line it prints** into the
same entry.

## Acceptance criteria

Every command below was run against the current tree while drafting and returns
the "before" value shown; every "after" value was computed from the code this
phase specifies rather than from intent.

- [ ] `test -f src/shell/log.rs && echo yes` → **yes** (file absent now).
- [ ] `grep -c '^mod log;' src/shell/mod.rs` → **1** (now `0`).
- [ ] `grep -cE '^pub struct CastWriter' src/shell/log.rs` → **1**.
- [ ] `grep -cE '^pub struct MetaIndex' src/shell/log.rs` → **1**.
- [ ] `grep -cE '^pub fn read_command_output' src/shell/log.rs` → **1**.
- [ ] `grep -c 'error_len' src/shell/log.rs` → **at least 1** — the F3
      discriminator is present rather than a blanket `from_utf8_lossy`.
- [ ] No `unwrap`/`expect`/`panic!` outside test code:
      `awk '/^#\[cfg\(test\)\]/{exit} {print}' src/shell/log.rs | grep -cE '\.(unwrap|expect)\(|panic!\('`
      → **0**. The `^` anchor is required; without it a doc comment mentioning
      the test attribute stops awk early and the guard becomes vacuous.
- [ ] No `unsafe` outside comments:
      `grep -vE '^\s*(//|///|//!|\*)' src/shell/log.rs | grep -c 'unsafe'` → **0**.
- [ ] `cargo test --lib shell::log::` reports **9 or more** passing and
      `0 failed` (now: `0 passed; 0 failed; … 1553 filtered out`).
      Use `shell::log::`, **not** `log::` — the latter also matches unrelated
      tests elsewhere in the tree.
- [ ] `cargo test --lib shell::pty::` still reports **13 passed, 0 failed** —
      phase-02 is untouched.
- [ ] All four gates pass: `cargo fmt --all`, `cargo build`,
      `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`.

## Test plan

Names pinned; placement is not — a `#[cfg(test)] mod tests` at the **bottom**
of `src/shell/log.rs`, the repo convention. Every name begins `cast_`, `meta_`
or `read_command_` so the E2E block can select them.

**The headline test is the round trip.** A log is only correct if a written
session reads back byte-exact, so that is the test that matters most; the rest
pin the pieces it depends on.

- `cast_and_meta_round_trip_a_three_command_session` — **the primary-use
  test.** Write a session: the header, then for each of three commands a `"m"`
  start marker, one or more `"o"` events, and a `"m"` end marker, capturing
  `byte_len()` before and after each command to fill `first_byte` / `end_byte`.
  Save the `MetaIndex`. Then assert `read_command_output` for **each** of
  index 0, 1 and 2 returns exactly that command's own output bytes and nothing
  from its neighbours. Make one of the three produce **empty** output.
- `cast_header_is_valid_asciicast_v2` — line 1 parses as JSON with
  `version == 2` and the given `width` / `height` / `timestamp`, and the file's
  first byte is `{`.
- `cast_event_line_shape` — an `"o"` event line parses as a 3-element JSON
  array whose element `[1]` is `"o"`, whose `[0]` is a JSON number, and whose
  payload round-trips.
- `cast_marker_and_input_events_use_their_codes` — `mark` writes `"m"` with the
  given label (including an **empty** label), `write_input` writes `"i"`.
- `cast_preserves_ansi_and_unit_separator_bytes` — a payload containing an ESC
  sequence, a `0x1f` byte and a CRLF reads back byte-identical through the
  reader (F1).
- `cast_carries_a_split_multibyte_character` — feed the bytes of `"é世界😀"`
  **split at a boundary that cuts a character** (F3 gives cut points 1, 3 and
  11), one piece per `write_output` call, and assert the reader returns the
  original bytes exactly. Also assert the writer emitted **no** line for a call
  whose input was entirely carried.
- `cast_does_not_carry_genuinely_invalid_bytes` — the negative half of F3.
  Write the bytes `0xff 0x41`; the writer must emit a line, and a
  **subsequent** valid write must come back correct — proving the invalid byte
  was consumed rather than carried forever and poisoning every later write.
- `meta_round_trips_through_save_and_load` — a `MetaIndex` holding two
  `CommandRecord`s, saved then loaded, compares equal.
- `meta_path_for_replaces_and_appends` — `…/x.cast` → `…/x.meta.json`; and the
  **negative case** `…/x.log` → `…/x.log.meta.json`.
- `read_command_output_rejects_an_unknown_index` — an index absent from
  `commands` returns `Err`, not `Ok(vec![])`.
- `read_command_output_skips_a_malformed_line` — a hand-built cast with one
  garbage line inside a command's byte range still returns the surrounding
  `"o"` payloads.

## End-to-end verification

Run this block verbatim from the repo root. It writes `/tmp/e2e-03.txt`.

```sh
{
echo "== A. build =="
cargo build 2>&1 | tail -2; echo "cargo_exit=${PIPESTATUS[0]}"
echo "== B. named tests (each line is one pinned test) =="
cargo test --lib 2>&1 | grep -E "^test shell::log::.* ok$" | sed 's/^test //' | sort
echo "cargo_exit=${PIPESTATUS[0]}"
echo "== C. shell::log:: totals =="
cargo test --lib shell::log:: 2>&1 | grep -E "^test result:"; echo "cargo_exit=${PIPESTATUS[0]}"
echo "== D. phase-02 untouched (must still be 13 passed) =="
cargo test --lib shell::pty:: 2>&1 | grep -E "^test result:"; echo "cargo_exit=${PIPESTATUS[0]}"
echo "== E. lib suite totals =="
cargo test --lib 2>&1 | grep -E "^test result:"; echo "cargo_exit=${PIPESTATUS[0]}"
echo "== F. structural greps (each must print the stated number) =="
echo -n "log.rs exists           (1): "; test -f src/shell/log.rs && echo 1 || echo 0
echo -n "mod log declaration     (1): "; grep -c '^mod log;' src/shell/mod.rs
echo -n "pub struct CastWriter   (1): "; grep -cE '^pub struct CastWriter' src/shell/log.rs
echo -n "pub struct MetaIndex    (1): "; grep -cE '^pub struct MetaIndex' src/shell/log.rs
echo -n "pub fn read_command_out (1): "; grep -cE '^pub fn read_command_output' src/shell/log.rs
echo -n "error_len discriminator(>=1): "; grep -c 'error_len' src/shell/log.rs
echo -n "unsafe in log.rs        (0): "; grep -vE '^\s*(//|///|//!|\*)' src/shell/log.rs | grep -c 'unsafe'
echo -n "unwrap/expect/panic pre-test (0): "
awk '/^#\[cfg\(test\)\]/{exit} {print}' src/shell/log.rs | grep -cE '\.(unwrap|expect)\(|panic!\('
} > /tmp/e2e-03.txt 2>&1
cat /tmp/e2e-03.txt
```

Paste the contents of `/tmp/e2e-03.txt` into your Update Log entry as a fenced
block, then run the self-check and paste its verdict line into the same entry:

```sh
D=docs/dev/milestones/M20-shell-engine/phase-03-asciicast-log.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-03.txt
diff /tmp/pasted-03.txt /tmp/e2e-03.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

**Sections B, C and D can each report success with nothing having run.**
Measured on the current tree: `cargo test --lib shell::log::` prints
`test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 1553 filtered out`
and exits `0`. A zero exit proves nothing — the pass conditions are the named
test lines in B, a count of nine or more in C, and exactly `13 passed` in D.

**Section F on an absent file errors rather than printing `0`.** Measured: a
`grep -c` against a missing path emits a `No such file or directory` warning on
stderr and exits `2`, printing no count. The block redirects `2>&1`, so such a
warning appearing in section F is itself proof the file is missing.

The PASTE MATCH self-check was validated both ways while drafting a sibling
phase: a byte-exact paste printed `PASTE MATCH`, and the same paste with one
line retyped printed `PASTE MISMATCH` naming the divergent line.

## Authorizations

- Create `src/shell/log.rs`; edit `src/shell/mod.rs` (the `mod log;` line and
  the `pub use`).
- Run `cargo fmt --all`, `cargo build`,
  `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`.
- **No new dependencies.** `serde`, `serde_json` and `tempfile` (dev) are
  already present and are all this phase needs.
- May **not** touch `src/shell/pty.rs`. Phase-02 is `done` and its 13 tests
  must stay at 13.
- May **not** touch `docs/architecture.md`, `CLAUDE.md` or `README.md` —
  M20's documentation updates land in phase-09.

## Out of scope

- **Wiring the log to anything.** No `PtyShell` integration, no writes under
  `~/.daemoneye/var/log/shells/`. Phase-05 (the shell-host) is the first
  consumer. A module with tests and no production caller is the intended end
  state — do not add `#[allow(dead_code)]`, and do not invent a caller.
- **`"r"` resize events.** The format reference above documents them so the
  writer's shape is informed by the whole spec, but phase-05 owns resize and
  adds that method together with its first caller.
- **Rotation, retention, sweeping.** `shells.log_retention_days` exists from
  phase-01 and is implemented in a later phase; this module deletes nothing.
- **Masking.** Payloads are recorded raw, which is what the security model
  requires for logs at rest. Masking happens where bytes reach a model, and
  that is phase-07's concern.
- **A vt100 screen, ANSI interpretation, or scrollback.** Phase-04 owns that;
  this module treats bytes as bytes.
- **Reading `"i"` input back out.** `read_command_output` returns `"o"`
  payloads only, as specified.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->

### Update — 2026-09-03 23:01 (progress)

Started phase-03: flipped status to `in-progress`, updated the milestone README
row, and wrote `src/shell/log.rs` + the `mod log;` / `pub use` changes in
`src/shell/mod.rs`. All 11 `shell::log::` tests pass, `shell::pty::` stays at
13/13, and build + clippy are clean. Working through the end-to-end capture now.

### Update — 2026-09-03 23:05 (end-to-end verification)

The block from § End-to-end verification, run verbatim from the repo root:

```
== A. build ==
   Compiling daemoneye v1.0.0 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 2.62s
cargo_exit=0
== B. named tests (each line is one pinned test) ==
shell::log::tests::cast_and_meta_round_trip_a_three_command_session ... ok
shell::log::tests::cast_carries_a_split_multibyte_character ... ok
shell::log::tests::cast_does_not_carry_genuinely_invalid_bytes ... ok
shell::log::tests::cast_event_line_shape ... ok
shell::log::tests::cast_header_is_valid_asciicast_v2 ... ok
shell::log::tests::cast_marker_and_input_events_use_their_codes ... ok
shell::log::tests::cast_preserves_ansi_and_unit_separator_bytes ... ok
shell::log::tests::meta_path_for_replaces_and_appends ... ok
shell::log::tests::meta_round_trips_through_save_and_load ... ok
shell::log::tests::read_command_output_rejects_an_unknown_index ... ok
shell::log::tests::read_command_output_skips_a_malformed_line ... ok
cargo_exit=0
== C. shell::log:: totals ==
test result: ok. 11 passed; 0 failed; 0 ignored; 0 measured; 1553 filtered out; finished in 0.00s
cargo_exit=0
== D. phase-02 untouched (must still be 13 passed) ==
test result: ok. 13 passed; 0 failed; 0 ignored; 0 measured; 1551 filtered out; finished in 2.00s
cargo_exit=0
== E. lib suite totals ==
test result: ok. 1560 passed; 0 failed; 4 ignored; 0 measured; 0 filtered out; finished in 4.79s
cargo_exit=0
== F. structural greps (each must print the stated number) ==
log.rs exists           (1): 1
mod log declaration     (1): 1
pub struct CastWriter   (1): 1
pub struct MetaIndex    (1): 1
pub fn read_command_out (1): 1
error_len discriminator(>=1): 1
unsafe in log.rs        (0): 0
unwrap/expect/panic pre-test (0): 0
```

PASTE MATCH self-check: `PASTE MATCH`