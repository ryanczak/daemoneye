# Phase 14 — Small tidies

**Status:** todo
**Milestone:** M32 — Findings fixes
**Drafted:** 2026-10-30
**Depends on:** phase-13
**Estimated diff:** ~45 insertions, ~26 deletions
**Tags:** language=rust, kind=bugfix, size=s
**Findings:** M32 `FINDINGS.md` #1; M31 `FINDINGS.md` #10, #17, #32, #33,
#48 (part; the rest is M32 #16)

## Goal

One defect and five tidies, the milestone's last phase. After this phase:

- **M32 #1 (bug):** a ghost refused for an unreadable config is told why.
  `ghost_foreground_refusal` in `dispatch_foreground.rs` prints the load
  error with `{e:#}`. `Config::load` wraps the TOML parse error in a
  `parsing <path>` context, and plain `{e}` printed only that outer
  layer, so the parse error itself was dropped. The remedy stops naming
  the path a second time. A new test,
  `ghost_foreground_refusal_names_the_error_under_its_context`, gives the
  gate an error with a context layer, as `Config::load` does.
- **#10:** `SessionsConfig` and `ApprovalsConfig`'s `Default` impls call
  `default_true()`, the function their serde attributes name, instead of
  writing a literal `true`.
- **#32, #33:** `ContainerInfo`'s docs describe the sbx sandbox it now
  carries: the sandbox name, its `de-ghost-` / `de-chat-` prefix, and the
  `image` field, which is always `"shell"` and kept only as wire format.
- **#48 (part):** `ArtifactHit.kind`, `ArtifactHit.score` and
  `EpochHit.score`, which nothing reads, go with their `#[allow(dead_code)]`.
- **#17:** `src/daemon/startup.rs` names its imports instead of taking
  them through `use super::*`.

The suite goes from **2610** to **exactly 2611**: one test added, none
removed.

**Not in this phase: `EventHit`'s two fields.** At drafting, removing them
broke `search_events_returns_tail_not_head_when_segment_exceeds_cap`. The
events FTS table is contentless (`content=''`), so `e.event` reads back
NULL, the `String` decode fails on every row, and `search_events` has
returned no hits at all. A probe at drafting confirmed it: the index held
the row, the `event` column read `None`, and `search_events` returned 0.
The events search has worked only through its segment-scan fallback.
Removing the "dead" field would switch the index path on and change what
the search returns. That is M32 FINDINGS #16, a decision for the PE, not a
tidy.

## Architecture references

- `src/config/load.rs` § `Config::load`: the `parsing <path>` context the
  refusal must see through.
- M32 README, decisions 1 and 2.

## Spec

1. **Run this, once, as your first command after the started entry:**

   ```sh
   python3 docs/dev/milestones/M32-findings-fixes/scripts/phase-14-small-tidies.py && cargo fmt --all
   ```

   **Do not open the script, and do not read any source file before
   running it.** It was run on a scratch worktree of `0fbb19f3`: fmt and
   clippy clean, hygiene passing, the full suite at
   `passed=2611 failed=0`. It asserts its own anchors and prints five
   lines, the last `startup.rs imports by name`. If an assert fires, stop
   and report it in the Update Log. Do not hand-edit anything it touches,
   and do not edit the script.
2. **Log it at once.** Append an `### Update — <date> (cuts done)` entry
   quoting the script's output, with an edit tool.
3. **Findings** go in their own `### Findings — <date>` entry. Run
   exactly `git diff --stat`, write what it shows in one or two lines, or
   write `none`. Open no file.
4. **Run the End-to-end block** and paste its output into an
   `### Update — <date> (end-to-end verification)` entry, as a fenced
   block of the output itself, not a description of it.
5. **Commit** as `fix:`.

The script is
`docs/dev/milestones/M32-findings-fixes/scripts/phase-14-small-tidies.py`
(sha256 begins `0a6c5c98fb0f5b6d`), kept in the repo as the record of the change.

### The red run is done — do not repeat it

The new test was run against the pre-phase refusal at drafting. It
failed on its first assertion, because the refusal carried only the
context layer:

```
the refusal must carry the parse error itself: Error: foreground execution is refused because the configuration could not be read: parsing /home/op/.daemoneye/etc/config.toml. Fix or remove /home/…/config.toml, then retry; nothing was run.
test result: FAILED. 0 passed; 1 failed
```

With the change, it passes.

## Acceptance criteria

- [ ] The End-to-end block prints
      `chain_fmt=1 chain_test=1 literal_true=0 docker_docs=0 dead_allows=2 glob_import=0`
      (today: `chain_fmt=0 chain_test=0 literal_true=2 docker_docs=3
      dead_allows=5 glob_import=1`). The two `dead_code` allows left are
      `EventHit`'s.
- [ ] `cargo test` reports **exactly** `passed=2611 failed=0` (today
      2610).
- [ ] Preservation: `tests/hygiene.rs` and `cargo test --test bug_tracker`
      pass; `tests/hygiene_allowlist.txt` is unchanged. fmt, build and
      clippy are clean.
- [ ] `grep -c '^### Findings — '` on this doc prints `1`, and the
      end-to-end entry holds the fenced output.

## End-to-end verification

```sh
bash <<'E2E'
echo "chain_fmt=$(grep -c 'could not be read: {e:#}' src/daemon/executor/dispatch_foreground.rs) chain_test=$(grep -c 'fn ghost_foreground_refusal_names_the_error_under_its_context' src/daemon/executor/dispatch_foreground.rs) literal_true=$(grep -cE '^ +(auto_name_enabled|commands): true,' src/config/types.rs) docker_docs=$(grep -cE 'de\.session|de\.ghost=1|as docker displays it' src/ipc_types.rs) dead_allows=$(grep -c 'allow(dead_code)' src/memory/index/search.rs) glob_import=$(grep -c '^use super::\*;' src/daemon/startup.rs)"
git diff --quiet HEAD -- tests/hygiene_allowlist.txt && echo allowlist=unchanged
cargo test --test hygiene 2>&1 | grep -E '^test result'
cargo test --test bug_tracker 2>&1 | grep -E '^test result'
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1
cargo fmt --all --check; echo "fmt_exit=$?"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
E2E
```

Expected: the shape line above, `allowlist=unchanged`, hygiene
`ok. 16 passed`, bug tracker `ok. 6 passed`, clippy clean, `fmt_exit=0`,
`passed=2611 failed=0`. If the suite stops early on a failure, paste that
output, run the block once more and paste both.

## Authorizations

- Run `scripts/phase-14-small-tidies.py`; do not edit it.
- Edit the five files it names, through the script only:
  `src/daemon/executor/dispatch_foreground.rs`, `src/config/types.rs`,
  `src/ipc_types.rs`, `src/memory/index/search.rs`,
  `src/daemon/startup.rs`.

## Out of scope

- `EventHit` and the events FTS search (M32 FINDINGS #16).
- Removing `ContainerInfo.image`: it is wire format.

## Update Log

<!-- entries appended below this line -->
