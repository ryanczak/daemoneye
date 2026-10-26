# Phase 06 — Move `run_daemon` out of `daemon/mod.rs`

**Status:** todo
**Milestone:** M31 — Code hygiene
**Drafted:** 2026-10-26

## Goal

`src/daemon/mod.rs` (901 production lines) drops under the 800-line cap by
moving `run_daemon`, the two helpers it alone uses (`tmux_wanted`,
`startup_session`) and their test module `startup_session_tests` into a new
`src/daemon/startup.rs`. Pure move: `run_daemon` keeps its signature and
its public path, `crate::daemon::run_daemon`, through a re-export.
Splitting the 598-line function itself is a later phase.

## Architecture references

- `docs/dev/milestones/M31-code-hygiene/README.md` — decisions 5, 6, 7.
- `src/main.rs` calls `daemon::run_daemon` (the only caller outside the
  module).

## Spec

1. **Cut with the shell — run this as written.** It was dry-run on a
   scratch worktree of `5a6ecbcd`: fmt, clippy and the full suite (2586)
   green with every step below. **Do not use `sed -i`**: the executor
   refuses in-place edits, so the rewrite goes through a temp file and `mv`.

   ```bash
   F=src/daemon/mod.rs
   S=$(awk '{l[NR]=$0} /^pub async fn run_daemon\(/ {s=NR-1; while (s>0 && (l[s] ~ /^\/\/\// || l[s] ~ /^#\[/)) s--; print s+1; exit}' $F)
   T=$(grep -n '^#\[cfg(test)\]' $F | tail -1 | cut -d: -f1)
   echo "S=$S T=$T"                      # expect 270 930
   { printf '//! Daemon startup: `run_daemon` and the tmux-session choice it makes.\n\nuse super::*;\n'
     sed -n '1,15p' $F | grep -v '^pub use '; echo; sed -n "${S},$((T-2))p" $F; } > src/daemon/startup.rs
   { head -n $((S-1)) $F; tail -n +$T $F; } > $F.tmp && mv $F.tmp $F
   ```

   `startup.rs` gets a copy of `mod.rs`'s `use` block (without its two
   `pub use` lines, which `use super::*` already brings in); step 4 prunes it.
2. **Log the cut at once.** Append a `### Update — <date> (cuts done)`
   entry to this doc's Update Log **with the patch or write tool**. The
   stall governor counts only edit-tool calls as progress (rexyMCP issue
   #14).
3. **Four one-line edits, with the patch tool:**
   - `src/daemon/mod.rs`: add `mod startup;` after `pub mod spend;`; after
     `pub mod utils;` add `pub use startup::run_daemon;` and, on the next
     two lines, `#[cfg(test)]` and `use startup::tmux_wanted;` (the
     `mod tests` left in `mod.rs` calls `tmux_wanted`).
   - `src/daemon/startup.rs`: `fn tmux_wanted(` becomes
     `pub(super) fn tmux_wanted(`.
4. **Prune imports:** `cargo fix --lib --allow-dirty`, which removes every
   unused import in both files (the dry run needed nothing else). If the
   classifier refuses it, delete each import the build warns about with the
   patch tool instead. Then `cargo fmt --all`.
5. **Allowlist:** delete `file src/daemon/mod.rs 901`, and change
   `fn src/daemon/mod.rs run_daemon 598` to
   `fn src/daemon/startup.rs run_daemon 598` (the function moved, unchanged).
6. **CLAUDE.md** "Key files": add a row for `src/daemon/startup.rs`:
   `run_daemon()`, the daemon's startup and accept loop, with
   `tmux_wanted()` / `startup_session()`, the tmux-session choice.
7. **Findings** (decision 7): you move all of `run_daemon`; record what
   you see while it scrolls past, or `none`. Note especially anything that
   would make splitting it hard, such as state shared across its sections.

## Acceptance criteria

- [ ] `src/daemon/startup.rs` exists and holds `pub async fn run_daemon(`,
      `fn tmux_wanted(`, `fn startup_session(` and `mod startup_session_tests`;
      `src/daemon/mod.rs` holds none of them and is at most 800 lines.
- [ ] `grep -c '^file src/daemon/mod.rs ' tests/hygiene_allowlist.txt` is 0,
      `grep -c '^fn src/daemon/startup.rs run_daemon 598$'` is 1, and
      `cargo test --test hygiene` passes.
- [ ] The End-to-end block prints `lost_other=0` and `added_other=0`.
- [ ] Preservation: `cargo test` passes with exactly 2586 passing tests.
- [ ] The Update Log has a `(cuts done)` entry written by an edit tool and a
      `### Findings — <date>` entry.

## End-to-end verification

Run with `bash`. Paste the output byte-exact into the Update Log.

```sh
export LC_ALL=C   # sort and comm disagree under the host locale without it
B=5a6ecbcd; A=/tmp/e2e-m31-06.txt; : > "$A"
wc -l src/daemon/mod.rs src/daemon/startup.rs | sed 's/^ *//' >> "$A"
echo "allowlisted=$(grep -c '^file src/daemon/mod.rs ' tests/hygiene_allowlist.txt) fn_moved=$(grep -c '^fn src/daemon/startup.rs run_daemon 598$' tests/hygiene_allowlist.txt)" >> "$A"
norm() { tr -d ' \t' | grep -v '^$' | sort; }
git show $B:src/daemon/mod.rs | norm > /tmp/m31-06-old.txt
cat src/daemon/mod.rs src/daemon/startup.rs | norm > /tmp/m31-06-new.txt
echo "lost_other=$(comm -23 /tmp/m31-06-old.txt /tmp/m31-06-new.txt | grep -vE '^(use|pubuse|fntmux_wanted\()' | wc -l)" >> "$A"
echo "added_other=$(comm -13 /tmp/m31-06-old.txt /tmp/m31-06-new.txt | grep -vE '^(use|pubuse|//!|pub\(super\)|#\[cfg\(test\)\]|modstartup;)' | wc -l)" >> "$A"
cargo test --test hygiene 2>&1 | grep -E '^test result' | sed 's/finished in [0-9.]*s/finished in <t>s/' >> "$A"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1 | sed 's/in [0-9.]*s/in <t>s/' >> "$A"
cargo fmt --all --check; echo "fmt_exit=$?" >> "$A"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}' >> "$A"
cat "$A"
```

Expected (from the dry run): `619 src/daemon/mod.rs`, `672
src/daemon/startup.rs`, `allowlisted=0 fn_moved=1`, `lost_other=0`,
`added_other=0`, hygiene `ok.`, clippy clean, `fmt_exit=0`,
`passed=2586 failed=0`. Small differences in the two line counts are fine if
`cargo fix` pruned differently; the rest must match.

## Authorizations

- Edit `src/daemon/mod.rs`; create `src/daemon/startup.rs`. Edit
  `tests/hygiene_allowlist.txt` (the two lines) and `CLAUDE.md` (one row).

## Out of scope

- Any change inside `run_daemon`, `supervise`, `detect_session` or
  `daemon_liveness`, even one a finding names.
- `src/main.rs`: its `daemon::run_daemon` call resolves through the
  re-export unchanged.

## Update Log

<!-- entries appended below this line -->
