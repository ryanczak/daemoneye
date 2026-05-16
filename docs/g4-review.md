# G4 Persistent Briefing State — Code Review

*Reviewed 2026-05-15 against `multi-agent` branch (commit 81ce173). 627 unit + 17 integration tests pass. Clippy/fmt clean.*

---

## Summary

Core briefing machinery is correctly implemented — generation, masking, path routing, CLI access, and first-turn injection all work as designed. Three defects: the `read_agent` tool omits briefing content (G4.4 sub-requirement), the four plan-specified behavioral tests are absent, and CLAUDE.md was not updated for the new source file (Engineering Standards gate). One ambiguity in the turn-limit exit path is worth noting but not blocking.

---

## Exit Criteria Assessment

| Criterion | Code | Test |
|---|---|---|
| Briefing written after clean ghost exit | ✓ | ✗ |
| Injected on next invocation | ✓ | ✗ |
| Masked before write | ✓ | ✗ (indirect) |
| `daemoneye agent briefing` shows content | ✓ | — |

---

## What's Correct

### Call site guarding — CORRECT ✓

`ghost.rs:808`. `generate_and_save_briefing` is called only inside the normal completion block at the bottom of `trigger_ghost_turn()`, after the `ghost_complete` event and `inc_ghosts_completed()`. All error paths (`anyhow::bail!` for timeout at line 407, AI error at line 651, session not found at line 183) return `Err` before reaching the completion block, so briefing generation is correctly skipped on failure.

### Path routing — CORRECT ✓

`agents/mod.rs:81`. `briefing_path(name)` returns `agent_dir(name).join("briefing.md")` → `~/.daemoneye/agents/<name>/briefing.md`. Consistent with the design document.

### Masking — CORRECT ✓

`briefing.rs:51`. `mask_sensitive(&result)` is called before the write, satisfying the Engineering Standards security invariant. The masked bytes are written to disk, not the raw model output.

### First-turn injection — CORRECT ✓

`prompt.rs:102–106`. The briefing block is assembled as `## Previous Session Summary\n{content}\n\n` and placed before the memory block in the first-turn prompt. The injection is gated on `ctx.agent_name.is_some()` via the `and_then` chain, so non-agent interactive sessions are unaffected.

### Best-effort error handling — CORRECT ✓

`briefing.rs:38–48` and `briefing.rs:54–62`. Failures in AI generation and file write both log a warning and return, not propagate. The ghost shell's exit code and status reporting are unaffected.

### Module declaration — CORRECT ✓

`daemon/mod.rs` contains `pub mod briefing;`.

### CLI access — CORRECT ✓

`main.rs:598`. `run_agent_briefing(name, clear)` reads and prints the briefing, or clears it when `--clear` is passed.

---

## Defects

### Defect 1 — `read_agent` tool omits briefing content — MEDIUM

**G4.4** (plan §4, line ~312): "extend the `read_agent` tool to include briefing content." This is listed as part of the G4.4 work, but `knowledge.rs:read_agent()` (lines 1142–1172) does not call `read_briefing()` or include any briefing field in its output string.

The IPC pair (`Request::GetAgentBriefing` / `Response::AgentBriefing`) is labelled "optional" in the plan, but the tool extension is not labelled optional. The consequence is that the interactive AI can `read_agent` to inspect its configuration but cannot discover whether a prior briefing exists — it has to `run_terminal_command` to check the file directly, which is worse for introspection.

**Required fix:** In `read_agent()`, after building the config output, call `crate::daemon::briefing::read_briefing(&cfg.name)` and append:

```rust
if let Some(b) = crate::daemon::briefing::read_briefing(&cfg.name) {
    out.push_str(&format!("\n  last_briefing ({}  chars):\n{}\n", b.len(), b));
}
```

---

### Defect 2 — Plan-specified behavioral tests are absent — MEDIUM

The plan (line ~315) names four tests:

- `ghost::briefing::writes_on_clean_exit`
- `ghost::briefing::skips_on_error_exit`
- `ghost::briefing::injects_on_next_run`
- `ghost::briefing::masking_applied`

None of these exist. What is present:

- Four unit tests in `briefing.rs` — test file I/O (`read_briefing`, `clear_briefing`) but not the behavioral contracts.
- `g4_briefing_read_and_clear` (integration) — exercises file I/O through public API. Correct, but redundant with the unit tests and not a substitute for behavioral coverage.
- `g4_briefing_injection_block_format` (integration) — tests `format_tool_restriction_block` from G3, not briefing injection. The name is misleading and the content is mis-categorized.

The behavioral contracts "briefing is written on clean exit" and "briefing is not written on error exit" are entirely untested. These do not require a live daemon. A unit test can call `trigger_ghost_turn` in a test configuration or simply assert the call site conditions directly. At minimum, `injects_on_next_run` should be a unit test in `prompt.rs` that writes a briefing file and asserts the formatted prompt contains `## Previous Session Summary`.

**Required fix:** Add at minimum:

1. In `daemon/prompt.rs` tests or `tests/integration.rs`:
   - `g4_briefing_injects_on_next_run` — write a briefing file, call `build_first_turn_prompt`, assert prompt contains `## Previous Session Summary`.
   - `g4_briefing_absent_does_not_inject` — no briefing file present; assert prompt does not contain `## Previous Session Summary`.

2. In `daemon/briefing.rs` tests:
   - `masking_applied` — call `do_generate_briefing` (or mock the channel) with content containing a fake secret; assert the written file does not contain it. (The mask call is present; this validates it's in the right place.)

3. Rename or fix `g4_briefing_injection_block_format` — it belongs in G3 coverage, not G4.

---

### Defect 3 — CLAUDE.md not updated — MINOR

`src/daemon/briefing.rs` is a new source file. Engineering Standards §1 (gate 7): "CLAUDE.md is updated if the phase adds a new source file to the key files table." The file is absent from the Key Files table in CLAUDE.md.

**Required fix:** Add a row to the Key Files table:

```
| `src/daemon/briefing.rs` | Briefing generation, injection, and CLI helpers for named agents (G4) |
```

---

## Observation — Turn-limit exit logs `ghost_error` then generates briefing

`ghost.rs:313–320, 808`. When max turns are exhausted, the code logs a `ghost_error` event (`"error": "max turns (N) reached"`) and then `break`s out of the loop. Control falls through to the completion block, which logs `ghost_complete`, calls `inc_ghosts_completed()`, and generates a briefing.

This means a turn-limit run emits both a `ghost_error` event and a `ghost_complete` event for the same session. A monitoring tool scanning `events.jsonl` would see an "error" followed by a "complete," which is semantically inconsistent.

The behavior is not necessarily wrong — the wrap-up turn explicitly asks the agent to summarize findings, so generating a briefing at turn-limit is useful. But the dual event emission is confusing.

**Not blocking**, but the `ghost_error` event at turn-limit should probably be renamed to `ghost_warn` or `ghost_turn_limit`, reserving `ghost_error` for paths that return `Err`. Or the completion block should skip briefing generation when `wrap_up_turn` fired (tracking with a flag). Document the intent one way or the other.

---

## What Needs to Happen for G4 to Be Closed

| # | Item | Severity |
|---|---|---|
| 1 | `read_agent` tool includes briefing content | Medium |
| 2 | `g4_briefing_injects_on_next_run` test | Medium |
| 3 | `g4_briefing_absent_does_not_inject` test | Medium |
| 4 | `masking_applied` test in `briefing.rs` | Medium |
| 5 | CLAUDE.md key files table updated | Minor |
| 6 | `ghost_error` / `ghost_complete` dual-event (turn-limit) | Minor / doc |

Items 1–4 are required to satisfy the plan's exit criteria and Engineering Standards test gate. Items 5–6 are minor cleanup.
