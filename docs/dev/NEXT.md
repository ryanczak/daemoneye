# NEXT

**Active phase:** **M21 phase-08f — prompt-on-the-real-prompt**, drafted
2026-09-27, status `todo`. Dispatch with `/rexymcp:dispatch phase-08f`.

**What it does.** A PTY password prompt is recognised only when the program
is really prompting, answered exactly once, and — when nobody can answer — cancelled
with `^C` so the shell stays usable. Six files: a split, exported sentinel
(one wrap, not two); the callback sees only output after the last answer; a
`^C` give-up with a 300 ms settle wait and a typed `PromptGivenUp`; honest
error text; the host scrub ends on `^C`.

**Why it jumped the queue.** Drafting 08c (ghost commands on a remote PTY
shell) probed a ghost's elevated command through the real run path on
`pinky`. It found the sentinel matched on the shell's echo (double
`SUDO_PROMPT` wrap), a correct password **re-sent into the shell as a
command** after it was accepted (13 plaintext copies in one cast — a
shipped chat defect, not ghost-only), and a `\n` give-up that re-prompts on
both sudo versions and eats the next command. PE decision 2026-09-27: new
row ahead of 08c. The fix was prototyped and measured on pinky and locally
before drafting; the prototype diff is quoted in the spec.

**Pre-dispatch check run.** The § End-to-end block was run verbatim against
`873187e0`: every non-preservation line reads its stated current value
(`new_tests=0p-0f`, `pty_sentinel=0`, `old_wrap_sites=2`, `gaveup_type=0`,
`false_comments=2`, `scrub_helper=0`, `ignore_total=9`); both preservation
lines hold (`tmux_untouched=0`, `client_tests=25p-0f`). Mutations A–C were
measured against the prototype with the spec's own fake program; D (settle
wait) is live-only, measured on pinky. **The backend redaction-marker flake
hit 2 of 3 lib runs on the untouched tree** during this check — the doc
tells the executor to report its name, not edit it.

**Next after 08f:** 08c (ghost-remote-pty — PE: sync, 300 s cap; close a
ghost's shells at exit), 08d (spawn-ghost-host), 08e
(remote-connect-handshake, probe first), 09 (live close).

Twenty-three rows in M21; live host `pinky` (sudo-rs 0.2.13, password
required; ssh is key-auth); `max_turns` 1000.
