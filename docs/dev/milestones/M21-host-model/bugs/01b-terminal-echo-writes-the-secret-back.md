# Bug: the terminal echoes the credential back into the output side of the cast log

**Milestone:** M21 — Host model & SSH
**Phase discovered during:** 01b (input-scrub-on-prompt)
**Severity:** high (the same credential, opposite direction of the wire)
**Status:** open

## Symptom

Phase 01b's Task 6 capture, against a real `daemoneye shell-host`:

```
[1.009027, "i", "[redacted]"]
[1.009318, "o", "hunter2-secret-e2e\r\n\u001b]0;matt@scrappy:~\u0007"]
```

The input side is redacted (01b's own property), but the **output** carries the
secret in clear. In that session's `cast` file

```
"secret_in_cast=true marker_in_cast=true"
```

— the grep for the throwaway password hit the `"o"` event, not the `"i"` one.

## Why it happens

`bash` with echo enabled (`echo -e on`) reflects every byte the master writes
back through the slave, and yes — the password answer is visible on the
screen, i.e. in output. `docs/security.md` § 2's own premise reflects this
direction is out of scope for 01b:

> - **Masking on the read path.** `read_shell` / `read_shell_log` already run
>   output through `mask_sensitive`. This phase is about what reaches disk.

but that masking is the daemon's *read* path; the cast file is the
**at-rest** artifact, the same artifact 01b's exit criterion is written
against. `secret_in_cast=false` is not achievable by redacting `input` alone
while the terminal echo is on.

## Why the probe nonetheless says it holds

The capture script greps for the secret string *anywhere* in the cast; at the
time the check was written the intent was "not present as an input event". The
echo makes the boolean honest only if the secret never passes through the
terminal at all, which no password prompt does when echo is on.

## Suggested direction (for the principal engineer to decide)

Either
- flip the terminal's echo off for the duration of the answer (`stty -echo`),
  which is what real sudo/sshd do, so the secret is not on the output bus
  either — this belongs in 01a's answer-writer, or
- scrub the **output** side too, keyed on the same `awaiting_secret` flag, at
  the cost of one line of screen fidelity per prompt.

## What is not wrong

- `PtyBackend::input`'s redaction branch — it fired; the output event proves
  the shell got the real bytes.
- `CastWriter`'s API — untouched, as pinned by 01b's must-survive criteria.
- `docs/shell.md` / `docs/security.md` — both are consistent with what the
  code now does on the input side.

## Repro

`.scratch-01b/probe.sh` (the Task 6 probe, which must be deleted with the
scratch directory before phase close): any command of the shape

```
printf '[de-sudo-prompt-0] Password:'; read -r pw
```

followed by an `Input` frame carrying the answer. The cast then holds the
answer as an output event from the terminal echo.
