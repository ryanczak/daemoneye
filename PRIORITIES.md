# T1000 Development Priorities

## 1. Foreground command completion detection robustness ✅ (implemented)

Added a content-stability check alongside the process-name check. Two
consecutive identical pane snapshots (100 ms apart) are now required before
declaring a command done. This prevents false-positive completion when
`idle_cmd` matches a subshell name (e.g. `bash script.sh` with idle shell
`bash`). The process-name check alone was insufficient for this case.

## 2. Prompt library UI (FR-1.3)

Config and file loading for `~/.t1000/prompts/` is implemented but there is no
way to discover or invoke prompts from the chat interface. A `t1000 prompts`
subcommand and a `/prompt <name>` in-chat command would complete the feature.
It is already half-built.

## 3. `/clear` in-session command ✅ (implemented)

Typing `/clear` generates a new session ID and prints a dim separator line
showing the new session. The daemon stores history by session ID so the next
message starts a clean context. The header hint was updated to advertise the
command. Also fixed the byte-vs-column bug in the turn indicator separator
(`label.len()` → `visual_len(&label)`).

## 4. Background command output trimming ✅ (implemented)

The same trailing-blank-line problem fixed for foreground commands could affect
background commands. Background output comes from subprocess stdout/stderr so
it is generally clean, but edge cases around whitespace-only or empty output
benefit from the same normalisation.

## 5. Plugin architecture (FR-1.5)

Largest remaining unimplemented requirement. Minimal first pass: executables in
`~/.t1000/plugins/` that receive prompt lifecycle events over stdin/stdout.
Unlocks community extensions without a full API.
