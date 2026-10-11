# Phase 06e: no-fg-target — the prompt stops naming a foreground pane

**Milestone:** M27 — tmux becomes an optional observer; 2.0 close
**Status:** todo
**Depends on:** phase-06c
**Tags:** language=rust, kind=bugfix, size=s

## Goal

`run_terminal_command` has had no `target_pane` and no `background`
parameter since the tmux execution path was deleted. Commands run in PTY
shells. But the daemon still tells the model, on every turn, to "pass
`target_pane="%N"` for `run_terminal_command(background=false)`". It also
sends a `[Pane target changed]` system message that says the same thing. The
model is told to call parameters that do not exist. This phase removes every
place the daemon tells the model about a foreground pane.

Dispatch this **before** phase 06d. 06d's docs describe the code after this
phase.

## Spec

1. **Remove the model-facing foreground-target text**, at every site below:
   - `prepend_foreground_target` in `src/daemon/prompt.rs` and both of its
     call sites, in the first-turn and subsequent-turn builders. Delete the
     function.
   - The `fg_target_line` in the subsequent-turn prompt
     (`src/daemon/prompt.rs`).
   - The `fg_line` that `get_shell_context`'s arm in
     `src/daemon/executor/mod.rs` puts in front of the context.
   - The `[Pane target changed]` drift message in
     `src/daemon/server/ask.rs`. Stop building it and sending it. **Keep**
     updating `entry.default_target_pane`.
   - The `## Execution Context` block in the first-turn prompt
     (`src/daemon/prompt.rs`) ends with two lines: `background=true → runs
     on DAEMON HOST` and `background=false → runs in USER'S PANE`. Replace
     both with one line: commands run in daemon-owned PTY shells on the
     daemon host. `wait: false` returns at once. `host` runs the command
     on a configured `[hosts.*]` host. Keep the Environment, Daemon host
     and terminal-pane lines.
   - **Every other string that says `background=true` or
     `background=false`**, in tool results, refusals and code comments
     alike. The acceptance grep lists them. Today they are in `bg_shell.rs`
     (both interactive refusals, plus the module doc), `executor/mod.rs`
     (the ghost no-sandbox refusal), `knowledge/shell.rs` (`watch_shell`'s
     nothing-running hint) and `ask.rs` (a comment). `background=true` →
     `wait: false`. "Run it in the foreground (background=false)" → "run it
     without `wait: false`".
2. **The seeded prompt.** In `assets/prompts/sre.toml`, delete the
   `[FOREGROUND TARGET]` paragraph and the `[Pane target changed]`
   paragraph. Keep the `[PANE MAP]` paragraph, the "refer to panes by window
   and command" paragraph, and the section heading.
3. **Keep the state.** `default_target_pane`, `/pane`, `pane_prefs.json`
   and the activity-based snapshot refresh in `ask.rs` stay exactly as they
   are. They still choose which pane's activity triggers a fresh context
   snapshot. Only the text that reaches the model goes away.
4. **Tests.** Some existing tests assert that the line is present. Change
   each one to assert that it is absent, and never delete one. Add
   `no_prompt_names_a_foreground_target`. It builds a first-turn and a
   subsequent-turn prompt with `default_target_pane` and `chat_pane` both
   set. It asserts that neither prompt contains `FOREGROUND TARGET` or
   `target_pane=`.

### Gotchas

- `PromptCtx.default_target_pane` may become unused once the text is gone.
  If clippy says so, remove the field and its initialisers. Do not add
  `#[allow(dead_code)]`.
- `target_pane_hint` in `executor/mod.rs` is a different symbol, the PTY
  route hint. Leave it alone.

## Acceptance criteria

- [ ] `grep -rn 'background=\(true\|false\)\|Pane target changed' src assets | grep -v src/ai/backends/gemini.rs`
      prints nothing. Today it prints 15 lines. (`gemini.rs` holds
      malformed-call parser fixtures. Leave them.)
- [ ] `grep -rn 'FOREGROUND TARGET' assets` prints nothing. Every
      remaining `FOREGROUND TARGET` hit under `src/` is inside a
      `#[cfg(test)]` module that asserts absence.
- [ ] `cargo test no_prompt_names_a_foreground_target` reports `1 passed`.
- [ ] `cargo test` reports `failed=0`. `passed` is today's 2308 plus the
      tests you add; name them in the Update Log.
- [ ] clippy (`-D warnings`) and `cargo fmt --all --check` exit 0.
- [ ] *Preservation:* `cargo test --test doc_truth` stays all-green (the
      prompt-asset hygiene tests read `sre.toml`).

## End-to-end verification

```sh
grep -rn 'background=\(true\|false\)\|Pane target changed' src assets | grep -v src/ai/backends/gemini.rs | wc -l
grep -rn 'FOREGROUND TARGET' assets | wc -l
cargo test no_prompt_names_a_foreground_target 2>&1 | grep -E '^test result: ok. 1 passed'
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1
cargo fmt --all --check; echo "fmt_exit=$?"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
```

Run each line once. Paste the output into the Update Log yourself,
followed by `PASTE MATCH: yes`. The server does not write it for you.

## Authorizations

- [x] May edit `src/daemon/prompt.rs`, `src/daemon/server/ask.rs`,
      `src/daemon/executor/mod.rs`, `src/daemon/executor/bg_shell.rs`,
      `src/daemon/executor/knowledge/shell.rs`, their tests, and
      `assets/prompts/sre.toml`. String and comment text only, outside the
      removals in Spec 1.

## Out of scope

- Removing `/pane`, `pane_prefs.json` or `default_target_pane` itself.
- README, CLAUDE.md and `docs/`. Phase 06d covers them.

## Update Log

<!-- entries appended below this line -->
