# Phase 03b: geometry — shells spawn at `[shells] rows` × `cols`, default 40×120

**Milestone:** M28 — Interactive shells
**Status:** todo
**Depends on:** phase-03a
**Estimated diff:** ~150 lines
**Tags:** language=rust, kind=feature, size=s

## Goal

Every shell the daemon spawns is 24×80, which cramps every modern TUI, and
Claude Code most of all. This phase adds `rows` and `cols` to `[shells]`
(default **40×120**). Every production spawn path reads them through one
helper. The model's `open_shell(rows, cols)` is phase 03c.

## Architecture references

- `docs/design/interactive-shells.md` D10: the defaults, and that every
  spawn path reads them. The `Resize` frame and attached mode's resize are
  unchanged. Tests that pin 24×80 keep passing it explicitly.

## Spec

1. **Config.** `ShellsConfig` (`src/config/types.rs`) gains `rows: u16`
   (default 40) and `cols: u16` (default 120), each with a serde default
   function like the existing ones. Add a method
   `ShellsConfig::size(&self) -> (u16, u16)` that clamps rows to `10..=200`
   and cols to `40..=500`, so a config of `rows = 0` cannot spawn an
   unusable terminal.
2. **One helper.** Add `pub fn spawn_size() -> (u16, u16)` in
   `src/shell/session.rs`, re-exported from `src/shell/mod.rs`. It returns
   `Config::load()`'s `shells.size()`, or `ShellsConfig::default().size()`
   when the load fails. `ShellsConfig` already has a hand-written
   `impl Default`: add the two fields to it, so that `default().size()`
   is `(40, 120)`. Also correct the struct's doc comment, which still says
   nothing in the daemon reads these values.
3. **The spawn sites.** Each of these 13 production calls passes
   `24, 80` or `TEST_ROWS, TEST_COLS`. Replace the size with
   `spawn_size()`. The list comes from the grep in § Acceptance criteria
   and is complete:
   - `src/daemon/executor/mod.rs`: 2;
   - `src/daemon/scheduled.rs`: 2;
   - `src/daemon/executor/bg_shell.rs`: 2;
   - `src/daemon/executor/foreground.rs`: 3;
   - `src/daemon/executor/file_ops/read.rs`: 1;
   - `src/daemon/executor/file_ops/ops.rs`: 1;
   - `src/daemon/executor/knowledge/shell.rs`: 2. These are the
     `#[cfg(not(test))]` `spawn_in_process_host` and `spawn_on_named_host`.
     `TEST_ROWS` / `TEST_COLS` stay for the test-only in-process host.

   Also change the fallback in `src/daemon/server/shells.rs`, where a host
   that cannot answer `Status` "reads as the spawn default": it uses
   `spawn_size()` now.
4. **Docs.** Add `rows` and `cols` to the `[shells]` block of
   `assets/etc/config.toml` and to the README's `[shells]` table and
   example, next to `scrollback_lines`. The
   `seeded_config_template_documents_every_config_field` test in
   `tests/doc_truth.rs` enforces the template and must pass unmodified.
   While you are in that block, delete its stale line "Nothing here takes
   effect yet; later M20 phases consume these values."
5. **Capture the end-to-end evidence.** Run § End-to-end verification
   verbatim. Paste `/tmp/e2e-03b.txt` into a new Update Log entry headed
   `### Update — <date> (end-to-end verification)`. The entry holds the
   output fence **and**, after it, the `PASTE MATCH` line the self-check
   printed.

**Do not touch:**
- `PtyShell::spawn(..., (24, 80))` in `src/daemon/executor/shell_exec.rs`.
  It is not a shell host.
- The `shell-host` CLI's `--rows` / `--cols` defaults in `src/main.rs`.
  Every spawn passes explicit sizes.
- Any test that passes 24×80 explicitly.

## Tests

- `shells_config_defaults_to_40_by_120` (`src/config/mod.rs` tests): an
  empty `[shells]` table gives rows 40 and cols 120; `rows = 50` and
  `cols = 200` parse; `ShellsConfig::default().size() == (40, 120)`.
- `shells_size_clamps_nonsense`: rows 0 and cols 0 ⇒ `(10, 40)`; rows
  9999 and cols 9999 ⇒ `(200, 500)`; `(50, 200)` is unchanged.

## Acceptance criteria

- [ ] The 2 new tests ⇒ `test result: ok. 1 passed` each.
- [ ] `grep -rnE 'spawn_(detached|ssh|sbx|ghost_ssh)_host\([^)]*(24, 80|TEST_ROWS, TEST_COLS)\)' src | wc -l`
      ⇒ `0` (13 today).
- [ ] `cargo test` ⇒ `passed=2387 failed=0`: exactly 2385 at `02bf9a12`
      plus the 2 new tests.
- [ ] `cargo test --test doc_truth` ⇒ `failed=0`, with the test file
      unmodified. Clippy (`-D warnings`) and `cargo fmt --all --check` exit 0.
- [ ] The end-to-end Update Log entry contains the output fence **and** the
      `PASTE MATCH` line.

## End-to-end verification

```sh
A=/tmp/e2e-03b.txt; : > "$A"
for t in shells_config_defaults_to_40_by_120 shells_size_clamps_nonsense; do cargo test --lib "$t" 2>&1 | grep -E '^test result' | sed "s/^/$t: /; s/[0-9]* filtered out/<n> filtered out/; s/finished in [0-9.]*s/finished in <t>s/" >> "$A"; done
echo "fixed_size_spawns=$(grep -rnE 'spawn_(detached|ssh|sbx|ghost_ssh)_host\([^)]*(24, 80|TEST_ROWS, TEST_COLS)\)' src | wc -l)" >> "$A"
cargo test --test doc_truth 2>&1 | grep -E '^test result' | sed 's/^/doc_truth: /; s/finished in [0-9.]*s/finished in <t>s/' >> "$A"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1 | sed 's/in [0-9.]*s/in <t>s/' >> "$A"
cargo fmt --all --check; echo "fmt_exit=$?" >> "$A"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}' >> "$A"
cat "$A"
```

Run each line **once**. Do not run module test suites separately, and do
not write throwaway scripts under `/tmp`. The last line runs the full
suite. If it reports failures, re-run it at most twice more, then paste
the last capture as it is and name every failing test.

Then run the self-check, and put the line it prints into the entry after
the fence:

```sh
D=docs/dev/milestones/M28-interactive-shells/phase-03b-geometry.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-03b.txt
diff /tmp/pasted-03b.txt /tmp/e2e-03b.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

## Authorizations

- [x] May touch `README.md` (the `[shells]` table and example only).

## Out of scope

- `open_shell(rows, cols)` and the result naming the size: phase 03c.
- `run_terminal_command(interactive: true)` and the ssh auto-upgrade:
  phase 03d.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->
