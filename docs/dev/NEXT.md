# NEXT

**Active phase:** `docs/dev/milestones/M21-host-model/phase-01b-input-scrub-on-prompt.md`
— 01b, input-scrub-on-prompt. **Drafted 2026-09-19**, status `todo`.

01a made the credential reach the shell. It also reaches
`var/log/shells/*.cast` in clear, as a plaintext `"i"` event. 01b stops that:
while the screen shows a secret prompt, the host writes `[redacted]` instead of
the bytes. The M21 exit criterion — the password absent from the shell log —
cannot be met without it.

**Small and single-file:** every edit is in `src/shell/backend.rs`, because the
leak has exactly one choke point. `PtyBackend::input` is the only place a byte
written toward the shell becomes a log event.

**Drafting measured the race I flagged at scoping, and it is real.** The output
pump broadcasts each chunk to the daemon *before* it feeds the screen. So a
daemon that answers the instant it sees a prompt can have the host write the
credential before the screen shows the prompt at all — a screen-based check
would log it in clear. The flag is therefore armed from the chunk itself,
ahead of the broadcast, and one of the two mutations moves that line to prove
the ordering is what holds the property.

**A polarity note the spec makes explicit,** because getting it backwards is a
leak: 01a's detector matches only its own sentinel, since a false positive
there injects a credential into an ordinary command. The scrub is the
opposite — a false positive costs one line of log fidelity, a false negative
writes a password to disk. So the host-side matcher is deliberately broad, and
a must-survive criterion keeps 01a's narrow detector from being widened.

Every acceptance criterion was run against HEAD `52a59c4e` and fails there.
Baselines measured: `shell::backend` 10 → 15, `shell::log` 21 unchanged,
`daemon::utils::sudo` 14 unchanged, lib 1893 → 1898.

**Criteria are anchored this time** (`^fn name(` rather than `fn name`). That
is the 01a calibration item: three of its criteria over-counted by
prefix-matching the test names the phase itself created.

Dispatch with `/rexymcp:dispatch phase-01b`.

**After 01b:** 02 host-config. Ten rows in M21; live host `pinky`;
`max_turns` now 1000.
