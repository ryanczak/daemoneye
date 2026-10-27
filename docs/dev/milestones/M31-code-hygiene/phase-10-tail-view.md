# Phase 10 — Move the tail and log viewers out of `tail.rs`

**Status:** in-progress
**Milestone:** M31 — Code hygiene
**Drafted:** 2026-10-27

## Goal

`src/cli/tail.rs` (988 production lines) drops under the 800-line cap by
moving its viewer section, `open_tail` through `color`, into a new
`src/cli/tail_view.rs` declared as a child module of `tail`. The section
covers opening a model from the first frame, `run_log_viewer`,
`run_tail_viewer`, `tail_loop`, the two status-line builders and
`render_tail`. The child is re-exported, so `crate::cli::tail::open_tail`,
`render_tail`, `run_log_viewer` and `run_tail_viewer` (used by
`cli/attach.rs` and `cli/commands/chat.rs`) still resolve. Pure move:
`TailModel`, its impls and the key-action functions stay in `tail.rs`.

## Architecture references

- `docs/dev/milestones/M31-code-hygiene/README.md` — decisions 5, 6, 7.

## Spec

1. **Cut with the shell — run this as written.** Dry-run on a scratch
   worktree of `b39b5aff`: fmt (a no-op), clippy and the full suite (2586)
   green. **No `sed -i` on source files** (the executor refuses in-place
   edits): the rewrite goes through a temp file and `mv`. Nothing inside
   the moved code changes; every function that crosses the boundary is
   already `pub`.

   ```bash
   F=src/cli/tail.rs; N=src/cli/tail_view.rs
   S=$(grep -n '^/// The model a tail starts from, given ' $F | cut -d: -f1)
   T=$(grep -n '^#\[cfg(test)\]' $F | tail -1 | cut -d: -f1)
   echo "S=$S T=$T"                      # expect 607 989
   { printf '//! The tail and log viewers: opening a model from the first frame, the\n//! viewer loops, and drawing the screen and its status line.\n\nuse super::*;\n\n'
     sed -n "${S},$((T-2))p" $F; } > $N
   { head -n $((S-1)) $F
     printf '#[path = "tail_view.rs"]\nmod view;\npub use view::{open_log, open_tail, render_tail, run_log_viewer, run_tail_viewer, tail_loop};\n\n'
     tail -n +$T $F; } > $F.tmp && mv $F.tmp $F
   ```
2. **Log the cut at once.** Append a `### Update — <date> (cuts done)`
   entry to this doc's Update Log **with the patch or write tool**. The
   stall governor counts only edit-tool calls as progress (rexyMCP issue
   #14).
3. `cargo fmt --all`. No import pruning: the build has no warnings.
4. **Allowlist:** delete the line `file src/cli/tail.rs 988`. No `fn`
   entry names `tail.rs`.
5. **CLAUDE.md** "Key files": add a row for `src/cli/tail_view.rs`: the
   tail and log viewers (`open_tail`, `open_log`, `run_tail_viewer`,
   `run_log_viewer`, `tail_loop`, `render_tail`), a child module of
   `cli/tail.rs`, re-exported from it.
6. **Findings** (decision 7): read the moved section once and record what
   you see, or `none`.
7. **Run the End-to-end block and paste its output into the Update Log
   with the write or patch tool**, as a `### Update — <date> (end-to-end
   verification)` entry holding the contents of `/tmp/e2e-m31-10.txt` in
   a fenced block. Check with `grep -c '^lost_other=0$'` on this doc
   before you commit; it must print 1.

**Work in this checkout.** The spec is already dry-run, so do not repeat
it in a scratch worktree of your own. Phase 09 did, and it cost turns and
added no evidence.

## Acceptance criteria

- [ ] `src/cli/tail_view.rs` defines `open_tail`, `open_log`,
      `run_log_viewer`, `run_tail_viewer`, `tail_loop` and
      `render_tail`. `tail.rs` defines none of them and has at most 800
      production lines (`cargo test --test hygiene` measures it).
- [ ] `grep -c '^file src/cli/tail.rs ' tests/hygiene_allowlist.txt` is 0
      and `cargo test --test hygiene` passes.
- [ ] The End-to-end block prints `lost_other=0` and `added_other=0`.
- [ ] Preservation: `cargo test` passes with exactly 2586 passing tests.
- [ ] The Update Log has a `(cuts done)` entry, a `### Findings — <date>`
      entry and an `(end-to-end verification)` entry containing the
      block's output. An edit tool wrote all three, and all three are in
      this file.

## End-to-end verification

Run with `bash`. Paste the output byte-exact into the Update Log (Spec 7).

```sh
export LC_ALL=C   # sort and comm disagree under the host locale without it
B=b39b5aff; F=src/cli/tail.rs; N=src/cli/tail_view.rs
A=/tmp/e2e-m31-10.txt; : > "$A"
wc -l $F $N | sed 's/^ *//' >> "$A"
echo "allowlisted=$(grep -c '^file src/cli/tail.rs ' tests/hygiene_allowlist.txt)" >> "$A"
norm() { tr -d ' \t' | grep -v '^$' | sort; }
git show $B:$F | norm > /tmp/m31-10-old.txt; cat $F $N | norm > /tmp/m31-10-new.txt
echo "lost_other=$(comm -23 /tmp/m31-10-old.txt /tmp/m31-10-new.txt | wc -l)" >> "$A"
echo "added_other=$(comm -13 /tmp/m31-10-old.txt /tmp/m31-10-new.txt | grep -vE '^(usesuper::\*;|pubuseview::|//!|#\[path=|modview;)' | wc -l)" >> "$A"
cargo test --test hygiene 2>&1 | grep -E '^test result' | sed 's/finished in [0-9.]*s/finished in <t>s/' >> "$A"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1 | sed 's/in [0-9.]*s/in <t>s/' >> "$A"
cargo fmt --all --check; echo "fmt_exit=$?" >> "$A"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}' >> "$A"
cat "$A"
```

Expected (from the dry run): `1442 …tail.rs`, `386 …tail_view.rs`,
`allowlisted=0`, `lost_other=0`, `added_other=0`, hygiene `ok.`, clippy
clean, `fmt_exit=0`, `passed=2586 failed=0`.

## Authorizations

- Edit `src/cli/tail.rs`; create `src/cli/tail_view.rs`. Edit
  `tests/hygiene_allowlist.txt` (one line) and `CLAUDE.md` (one row).

## Out of scope

- `tail.rs`'s inline `mod tests` (stays inline, under the 2000-line cap),
  `TailModel` and the key-action functions.
- Any change inside the moved functions.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-10-27 02:05 (cuts done)

Ran the Spec 1 pipeline as written in this checkout (no scratch worktree —
the spec is already dry-run; see "Work in this checkout"). `S=607 T=989`
printed exactly as the spec expects. The cut produced `src/cli/tail_view.rs`
(386 lines) and left `src/cli/tail.rs` at 1442 total lines. The diff against
`b39b5aff` is a pure move: 381 lines left `tail.rs`, all of them reappearing
in `tail_view.rs`; the only new lines are the module wiring
(`#[path = "tail_view.rs"] mod view;` and the one-line
`pub use view::{…}`) plus the new file's two doc-comment lines and
`use super::*;`. Nothing inside the moved code changed; every function that
crossed the boundary (`open_tail`, `open_log`, `run_log_viewer`,
`run_tail_viewer`, `tail_loop`, `render_tail`) was already `pub`, so no
visibility edit was needed. The `color`, `attached_status` and
`replay_status` helpers stay private inside `tail_view.rs`, which uses them;
`tail.rs`'s inline tests reach the re-exported functions through the `pub
use` and their existing `use super::*;`.

### Findings — 2026-10-27

Read the whole moved section once (`src/cli/tail_view.rs`, 386 lines:
`open_tail`, `open_log`, `run_log_viewer`, `run_tail_viewer`, `tail_loop`,
`attached_status`, `replay_status`, `render_tail`, `color`). Findings, none
acted on per decision 6:

- **`tail_view.rs` · `run_log_viewer`/`run_tail_viewer` — smell, minor.**
  The two viewer entry points duplicate eleven lines of alternate-screen
  plumbing: `EnterAlternateScreen`, the `AltScreenGuard` teardown closure
  (`LeaveAlternateScreen`, `Show`, `renderer.reanchor()`), the
  `ratatui::Terminal::new(CrosstermBackend::new(stdout))` and the two
  `Box::pin(futures_util::stream::unfold(…))` key/resize streams, all
  identical in the two functions — and `attach.rs`'s `run_attach` repeats
  the same four pieces a third time. A shared
  `enter_alt_screen(renderer)` helper would leave each entry point with its
  stream setup only. Not acted on: three call sites is the
  abstract-when-the-fourth-appears boundary, and a helper returning three
  borrowed streams is awkward.
- **`tail_view.rs` · `run_log_viewer` — smell, nit.** Its frame source is
  `tokio::io::BufReader::new(tokio::io::empty()).lines()` — an empty reader
  type-swizzled into `tail_loop`'s `tokio::io::Lines<R>` parameter so a log
  (which has no stream) can share the loop. It works, but the real contract
  ("no frame source") is expressed as a fake source that can never yield; a
  `None`-able frames parameter would say it. Confined to this one call.
- **`tail_view.rs` · `tail_loop` — smell, nit.** The replay branch resets
  `last` after every key, so a replay's clock advances only between keys —
  the in-code comment says so and the skip affordance matches; but nothing
  in `replay_status` tells a user that a paused replay needs a key to keep
  counting time. Display choice, not a bug.
- **`tail_view.rs` · `render_tail` — smell, nit.** The status `match`'s
  guard ordering is load-bearing and easy to disturb: `model.attached`
  wins first, then `replay.is_some()`, then `model.log`, then the
  search/note arms. An attached replay shows the attached status line,
  which is right, but nothing guards the order; a match over a small
  ordered enum would.
- **`tail_view.rs` · `color` — smell, nit.** Two match arms per vt100
  colour where `Idx(n) → Color::Indexed(n)` plus a 16-entry name table
  would read shorter; the explicit arms make the theme mapping visible at
  a glance, which is the stated why. Keep.

No `unwrap`/`expect`/`panic` in the moved production code; the two
`map_err(|msg| anyhow::anyhow!(msg))` calls are the documented
`Result<TailModel, String>` → `anyhow` boundary at the viewer's edge. No
commented-out code, no behaviour change.

### Update — 2026-10-27 (end-to-end verification)

Ran the phase's End-to-end block with `bash` in this checkout; output below,
captured mechanically (redirected to `/tmp/e2e-m31-10.txt` and pasted
byte-exact):

```text
1442 src/cli/tail.rs
386 src/cli/tail_view.rs
1828 total
allowlisted=0
lost_other=0
added_other=0
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in <t>s
    Finished `dev` profile [unoptimized + debuginfo] target(s) in <t>s
fmt_exit=0
passed=2586 failed=0
```
