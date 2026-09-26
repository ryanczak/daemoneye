# NEXT

**Active phase:** **M21 phase-07a — read-file-host**, drafted 2026-09-26,
status `todo`. Dispatch with `/rexymcp:dispatch phase-07a`.

**Row 07 split into 07a/07b at drafting, on a counted seam.** The read path
is **one** `target_pane` branch (`read.rs:154`); the write path is **four**
(`ops.rs:35, 247, 391, 545`). 07a does the read and builds the shell
transport; 07b does the four writes on top of it and deletes the pane-era
helpers. M21 is nineteen rows.

**Measured at drafting: the marker protocol already exists — only the
transport is wrong.** `build_remote_read_cmd` (`read.rs:19`) already emits
`echo '__DE_S__'; sed -n 'N,Mp' '<path>' …; echo '__DE_E__'`, and `read.rs:176`
already calls `extract_marked(&snap, "__DE_S__", "__DE_E__")`. What runs it is
`remote_run_and_capture` (`file_ops/mod.rs:90`): `send-keys`, then poll
`capture-pane` every 300 ms until a sentinel appears. That races the shell, is
bounded by the visible pane, and needs a tmux pane to exist. 07a swaps the
transport and keeps both halves of the marker contract — **Mutation B breaks
one sentinel and must turn the preservation test red**, because a silent drift
between them is how a remote read starts returning the whole screen.

**`target_pane` goes 2 → 1 here, 1 → 0 in 07b**, which is when the milestone's
*"`target_pane` is gone from the tool surface"* exit criterion closes. Two
criteria pin that `remote_run_and_capture` and `local_read_via_buffer`
**survive** 07a — the four write branches still need them.

**Pre-dispatch check run; both non-preservation criteria confirmed failing**
(`target_pane_params=2`→1, `read_file_host=0`→1), with three preservation
lines correctly passing and the ten named tests reading
`0 passed; 2010 filtered out`.

**The new mechanical step was applied, and it earned its place.** Per the
memory note added at 06d's close: for every `grep -c` criterion on an
identifier the phase introduces, simulate the **post-phase corpus** before
dispatch. Done here — a scratch file carrying test 10's likely shapes
(`p.name == "target_pane"`, `p.name == "host"`) plus a `use` line was
concatenated onto `defs.rs` and both criteria re-run against it. Neither
moved (`target_pane_params` stays 2, `read_file_host` stays 0 in its scoped
range), so neither is contaminated. **Separately verified the `awk` range
does not overrun**: `/name: "read_file"/,/category:/` captures exactly that
one `ToolDef` and its five params — which matters because `open_shell` also
has a `host` param that a sloppier range would have counted.

**Token audit on runnable blocks only** — four `sh` blocks and four inline
commands, zero `su`+`do` hits.

**Next after 07a: 07b — edit-file-host**, then 08, 08b, 09.

Nineteen rows in M21; live host `pinky` (sudo requires a password since
2026-09-22); `max_turns` 1000.
