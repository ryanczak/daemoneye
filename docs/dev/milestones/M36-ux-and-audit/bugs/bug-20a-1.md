# Bug 1 on phase-20a: every local edit's approval diff is masked, not just the config's

**Severity:** major
**Status:** open
**Filed:** 2026-11-09

## What's wrong

`run_edit_with_spawn` (`src/daemon/executor/file_ops/ops.rs`, the local
edit's `EditFilePrompt`) now sends

```rust
existing_content: Some(crate::ai::mask_sensitive(&original)),
new_content: Some(crate::ai::mask_sensitive(&updated)),
```

with no condition. `is_config` is computed a few lines above and used only
for `always_asked`. So every local `edit_file operation="edit"` masks its
approval diff, whatever file it targets. Suppose a user asks the AI to change
`password = old` to `password = new` in an ordinary app config. The approval
panel shows `<REDACTED>` on both sides, and the user approves a change they
cannot see.

It is also inconsistent: the remote edit's prompt, `create` and `copy` still
send raw content. The change is not named in Findings or the docs.

## What should happen

Behaviour 3 masks the prompt **for the config edit**: "The `EditFilePrompt`'s
existing and new contents pass through `mask_sensitive`, so the client
never receives a secret value." Every other edit's prompt carries the file's
real contents, exactly as before the phase.

## Root cause

The masking was applied at the shared prompt site instead of under the
`is_config` condition that already exists in the same function.

## Definition of done

- [ ] `edit_file_ordinary_edit_prompt_is_not_masked` exists and passes. A
      local edit of a non-config file containing a `password = …` line
      sends an `EditFilePrompt` whose existing and new contents both hold
      the real value. **Today the test does not exist, and the behaviour
      it pins fails.**
- [ ] `edit_file_config_prompt_is_masked_and_always_asked` still passes.
