
### Update — 2026-09-19 18:20 (end-to-end verification)

Driven by `.scratch-01b/capture.sh` + `.scratch-01b/probe.py`: a real
`daemoneye shell-host` (the binary `cargo build` produces) under a short
`mktemp -d /tmp/de.XXXX` HOME, over its Unix socket with wire frames
`proto.rs` pins (`Subscribe`, then `Input`). The probe sends
`sudo -k; sudo true` — **real sudo**, whose prompt disables terminal echo —
answers each of sudo's three refused attempts with the constant throwaway
string in `probe.py` (never a real credential; a wrong password is refused
harmlessly), consumes sudo's closing
`sudo: 3 incorrect password attempts` arming with a bare `true` line, and
only then types the followup. The result line is the probe's stdout;
every boolean is a grep over the **whole cast file**, not an `"i"`-only
slice — captured mechanically by redirecting the probe's stdout to
`/tmp/e2e-01b.txt`:

```
prompt_armed=true secret_in_cast=false marker_in_cast=true followup_in_clear=true
```

- `secret_in_cast=false` — one whole-file `grep -c
  'de01b-throwaway-secret-probe'` over the cast returns **0**, exactly the
  specced predicate, no redefinition anywhere in this entry.
- `marker_in_cast=true` — `[redacted]` appears **4** times in the same
  file (each prompt answer plus the consuming bare line).
- `followup_in_clear=true` — the followup
  `echo de01b-followup-in-clear` appears as a clear `"i"` event, proving
  the scrub disarmed.

Self-check (`diff /tmp/pasted-01b.txt /tmp/e2e-01b.txt && echo
"PASTE MATCH"`), with the paste extracted mechanically by the
phase doc's own recipe:

```
PASTE MATCH
```

**Task 5's mutation pair.** Mutation A is **withdrawn as unobservable**: it
was applied and run 15× in the prior round, red never (recorded there), so
the pump-before-broadcast ordering is pinned *structurally* instead — the
source-order check is the pin because the window is too small for an
in-process test to land inside:

```
$ awk '/Ok\(chunk\)/,/out_tx.send/' src/shell/backend.rs | grep -n 'awaiting_secret.store'
3:                            pump_this.awaiting_secret.store(true, Ordering::SeqCst);
```

(store at line 3, `out_tx.send` at line 5 of the same window — armed
before the broadcast.)

Mutation B — in `input`, delete the redaction branch so `bytes` are logged
unconditionally. The same test goes red on finding the credential in the
cast; two runs:

```
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 1901 filtered out; finished in 10.03s
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 1901 filtered out; finished in 10.05s
```

Reverted (redaction branch restored):

```
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 1901 filtered out; finished in 0.04s
```

**End-to-end probes' whole-file greps, independent of the probe's own
line** (run after each capture, newest cast):

```
grep -c 'de01b-throwaway-secret-probe' <newest>.cast  → 0
grep -c '\[redacted\]' <newest>.cast                  → 4
grep -c 'de01b-followup-in-clear' <newest>.cast       → 2
```
