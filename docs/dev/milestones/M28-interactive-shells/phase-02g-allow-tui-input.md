# Phase 02g: allow-tui-input — the runbook opt-in for ghost input to a program

**Milestone:** M28 — Interactive shells
**Status:** todo
**Depends on:** phase-02f
**Estimated diff:** ~150 lines
**Tags:** language=rust, kind=feature, size=s

## Goal

Since phase 02f, a ghost may type with `send_input` only into a shell at
its prompt. A runbook that drives a program on purpose, such as answering
an installer's questions or working through a TUI, has no way to say so.
This phase adds the runbook frontmatter field `allow_tui_input: true`,
which lets a ghost type into a shell whose foreground is a program, line
mode or TUI. A paused, exited or unknown shell stays refused either way.
The agent `ToolPolicy` explicit allow is still required first: the field
widens **where** a permitted ghost may type, never **whether** it may.

## Architecture references

- `docs/design/interactive-shells.md` D9: the guard table and the
  `allow_tui_input` row.

## Spec

**Files:** `src/runbook.rs`, `src/ipc.rs` (`GhostConfig`),
`src/daemon/policy.rs` (`GhostPolicy`), `src/daemon/policy_tests.rs`,
`src/daemon/executor/mod.rs`. Docs: `README.md` (the runbook frontmatter
table), `assets/prompts/sre.toml` and `CLAUDE.md` (one sentence each on
`send_input`).

1. **The field.** `GhostConfig` gains `#[serde(default)] pub allow_tui_input: bool`,
   and `GhostPolicy` gains `pub allow_tui_input: bool`. `GhostPolicy::from_config`
   copies it. The runbook parser reads it with
   `parse_bool_field(frontmatter, "allow_tui_input")`, next to
   `auto_approve_commands`. The compiler names exactly **6** literals to
   update: `src/runbook.rs` (the parser's `GhostConfig`),
   `src/daemon/policy.rs` (`from_config`) and four in
   `src/daemon/policy_tests.rs`. Every other literal uses
   `..Default::default()`. That list comes from a compile and is complete.
2. **The gate.** `ghost_may_type_into(fg, allow_tui_input: bool)` returns
   true for `Prompt`. For `Program { .. }` it returns `allow_tui_input`.
   For `Paused`, `Exited` and `Unknown` it returns false. The `SendInput`
   arm passes the ghost policy's `allow_tui_input` (false when there is no
   policy).
3. **The refusal.** `ghost_input_refusal` keeps its signature. For a
   `Program` foreground it also says that the runbook field
   `allow_tui_input: true` lets a ghost type into a running program. For
   `Paused`, `Exited` and `Unknown` it does **not** mention the field,
   because the field does not lift those.
4. **Docs.** Add an `allow_tui_input` row to the README's runbook
   frontmatter table, next to `auto_approve_commands`: bool, default
   `false`, and what it lifts and what it does not. Add one sentence to the
   `send_input` text in `sre.toml` and to the `send_input` row in
   `CLAUDE.md`: a ghost types only at a prompt unless its runbook sets
   `allow_tui_input: true`. `tests/doc_truth.rs` must pass unmodified.
5. **Capture the end-to-end evidence.** Run § End-to-end verification
   verbatim. Paste `/tmp/e2e-02g.txt` into a new Update Log entry headed
   `### Update — <date> (end-to-end verification)`. The entry holds **only**
   the output fence, followed by the `PASTE MATCH` line.

**Gotchas.**
- These two existing tests change, and that list is complete:
  - `ghost_may_type_only_at_a_prompt` is **renamed**
    `ghost_may_type_into_a_program_only_with_allow_tui_input`. With the
    flag false, only `Prompt` is true. With the flag true, `Prompt` and
    both `Program`s are true. `Paused`, `Exited` and `Unknown` are false
    either way.
  - `ghost_input_refusal_names_the_shell_and_what_holds_it` keeps its name
    and gains two assertions: the `Program` refusal contains
    `allow_tui_input`, and a `Paused { cmd: "vim" }` refusal does not.
- Do not change the `ToolPolicy` gate or its order. A ghost whose policy
  does not explicitly allow `send_input` is refused before the foreground
  probe, whatever its runbook says.

## Tests

- `runbook_parses_allow_tui_input` (`runbook.rs` tests): frontmatter with
  `allow_tui_input: true` ⇒ `ghost_config.allow_tui_input` is true. The same
  runbook without the line ⇒ false.
- `ghost_policy_carries_allow_tui_input` (`policy_tests.rs`):
  `GhostPolicy::from_config` of a config with the flag true ⇒ true, and of
  `GhostConfig::default()` ⇒ false.
- The rename and the extended test in § Gotchas.

## Acceptance criteria

- [ ] The 2 new tests, the renamed one and
      `ghost_input_refusal_names_the_shell_and_what_holds_it` ⇒
      `test result: ok. 1 passed` each.
- [ ] `cargo test` ⇒ `passed=2378 failed=0`: exactly 2376 at `67c39fd2`
      plus the 2 new tests. The rename adds nothing.
- [ ] `cargo test --test doc_truth` ⇒ `failed=0`. Clippy (`-D warnings`)
      and `cargo fmt --all --check` exit 0.
- [ ] `PASTE MATCH` appears in the end-to-end Update Log entry.

## End-to-end verification

```sh
A=/tmp/e2e-02g.txt; : > "$A"
for t in runbook_parses_allow_tui_input ghost_policy_carries_allow_tui_input ghost_may_type_into_a_program_only_with_allow_tui_input ghost_input_refusal_names_the_shell_and_what_holds_it ghost_may_send_input_requires_an_explicit_allow; do cargo test --lib "$t" 2>&1 | grep -E '^test result' | sed "s/^/$t: /; s/[0-9]* filtered out/<n> filtered out/; s/finished in [0-9.]*s/finished in <t>s/" >> "$A"; done
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

Then the self-check:

```sh
D=docs/dev/milestones/M28-interactive-shells/phase-02g-allow-tui-input.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-02g.txt
diff /tmp/pasted-02g.txt /tmp/e2e-02g.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

## Authorizations

- [x] May touch `README.md` (the runbook frontmatter table only) and
      `CLAUDE.md` (the `send_input` row only).

## Out of scope

- An agent-level (`config.toml` / `AgentConfig`) form of the opt-in. It is
  a runbook field only, the same as `auto_approve_commands`.
- Any change to chat input. Chat is never refused by foreground.

## Update Log

(Filled in by the executor. See WORKFLOW.md § "Update Log entries".)

<!-- entries appended below this line -->
