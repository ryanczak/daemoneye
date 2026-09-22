# NEXT

**Active phase:** **M21 phase-05 — ssh-prompt-detection**, drafted 2026-09-22,
status `todo`. Dispatch with `/rexymcp:dispatch phase-05`.

**Every prompt string in the spec was captured live off `pinky` at drafting**,
through a PTY with the same `-tt` argv the daemon spawns. Each of the three
prompts **hangs to a 124 timeout today** — the same failure 01a found for
local sudo:

```
Are you sure you want to continue connecting (yes/no/[fingerprint])?
matt@pinky.home.planetfoo.org's password:
Password:                                  (su, over the ssh PTY)
```

and `Permission denied (publickey,password).` ends the connection with exit
255 — terminal, not answerable, so the row fails fast rather than waiting out
`timeout_secs`.

**Measured hazard the spec pins with a test and a mutation.** The host-key
prompt contains the word *fingerprint* twice (`ED25519 key fingerprint is:`
and `(yes/no/[fingerprint])`). `is_fingerprint_prompt` matches only the three
biometric sentences, so it does not fire today — but `classify_prompt` checks
it **first**, so broadening it to a bare `"fingerprint"` would classify every
host-key prompt as an unanswerable biometric prompt. Mutation A makes exactly
that edit and must turn the test red.

**The scrub needs no change, and that is measured too.**
`looks_like_secret_prompt` already matches the password and passphrase prompts
and correctly does *not* match the host-key question — whose answer is `yes`,
not a secret, and belongs in the log in clear. Two tests pin all three facts
and the needle list is a preservation criterion.

## RESOLVED 2026-09-22 (mid-dispatch of 05) — PE made `pinky` require a sudo password

The blocker below was real when 05 was drafted and is **no longer true**. The
PE changed `pinky`'s sudoers while phase-05 was running; re-measured:

```
$ ssh pinky 'sudo -k; sudo -n true; echo rc=$?'
sudo: interactive authentication is required
sudo_n_rc=1
```

**And the sentinel survives the ssh hop** — the exact measurement row 09
needs, taken over `ssh -tt` with the daemon's own `SUDO_PROMPT` form:

```
[sudo: [de-sudo-prompt-77]] Password: *
```

`pinky`'s sudo wraps the sentinel as `[sudo: …]` and appends `Password: *`
(a different build/config from the local host), but the sentinel is present
as a substring, so `classify_prompt`'s `output.contains(sentinel)` matches
and 01a's round-trip applies unchanged. Note the trailing `Password:` — the
sentinel check runs **first** (phase-05 test 6 pins that), so it classifies
as `SudoPassword`, never as an ssh prompt.

**Phase 05 is unaffected and was left running.** Its live block uses key
login (still works) and `su`, neither of which the sudoers change touches.
Its § Current state 4 is now stale as a *current* fact but was correct as
measured; that gets recorded at 05's review, not by editing the doc under a
running executor. **Row 09's exit criterion is verifiable as written.**

## (superseded) Measured blocker for row 09 — needs a PE decision before 09 is drafted

```
$ ssh pinky 'sudo -k; sudo -n true; echo rc=$?; sudo -l'
sudo_n_rc=0
User matt may run the following commands on pinky:
    (ALL) NOPASSWD: ALL
```

**`pinky` never prompts for sudo.** Two consequences:

1. The milestone README's row-05 line said "Remote sudo is 01a's detector over
   the ssh PTY — **verified live**." On this host that verification is not
   available. Phase 05 does not claim it; its live check uses `su` over the
   ssh PTY instead, which does prompt (measured, and the block runs clean).
2. **Row 09's exit criterion is unverifiable as written** — "a remote sudo
   edit … with the password absent from every `"i"` event" is trivially
   satisfied when there is no password. Options when 09 is drafted: use a host
   that requires a sudo password, add a password-requiring sudoers rule on
   `pinky` for one command, or restate the criterion around `su`. **This is a
   PE call on a signed-off exit criterion, not the architect's.**

**Pre-dispatch check run; both non-preservation criteria confirmed failing:**
`lib_counts=1951p-0f-6i` (→1961), `new_variants=0` (→3). The three
preservation lines correctly pass today and are labelled as such
(`fingerprint_not_broadened=0`, `needles=5`, `ignore_total=6`). The ten named
tests read `0 passed; 1957 filtered out`, and the § Live verification block
was **run as written** and returned `hostkey_q=1 su_password=1`.

**Both 04d calibrations are folded into this spec:** the E2E criterion now says
*re-capture as the last thing before committing, or the evidence is stale*,
and the no-new-`#[ignore]` line is scoped rather than blanket.

**Next after 05: 06 — new-host-approval**, which owns the interactive
host-key trust flow this row deliberately stops short of.

Fifteen rows in M21; live host `pinky`; `max_turns` 1000.
