# Phase 01d: session-log-retention — per-session logs get an opt-in retention setting

**Milestone:** M23 — Client decoupling & shell UX
**Status:** todo
**Depends on:** none (M21 carry-forward)
**Estimated diff:** ~60 lines of production code across 3 files; 3 docs; 3 new tests
**Tags:** language=rust, kind=feature, size=s

## Goal

`~/.daemoneye/var/log/sessions/` grows without bound. The per-session chat
logs (`<id>.jsonl`) and their resume state (`<id>.meta.json`) have **no
retention setting at all**. Only the compacted archives (`<id>.archive.jsonl`)
do, via `[sessions] archive_retention_days`. After this phase,
`[sessions] log_retention_days` (default `0`, keep forever) deletes an
inactive session's `<id>.jsonl` **and** its `<id>.meta.json` once the log has
not been written for that many days. It never touches archives or epoch
files, which have their own lifecycle and feed the memory index, and it never
touches an active session.

## Architecture references

- `src/config/lifecycle.rs` — the artifact-lifecycle policy table (one row
  per directory).
- `docs/dev/milestones/M21-host-model/README.md` § Retrospective → "Carried
  forward".

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read this entire phase doc before touching any code.
3. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

Measured on **2026-10-01** at commit `b8fb1dd1`.

Baseline: `cargo test --lib` → **2095 passed / 0 failed / 14 ignored**.

### 1. What lives in the directory

On the PE's machine, `var/log/sessions/` holds four kinds of per-session file:

| Suffix | Count | What it is | Who reads it |
|---|---|---|---|
| `<id>.jsonl` | 160 | the session's chat log | `daemoneye session import` |
| `<id>.meta.json` | 68 | resume state across idle eviction (`src/daemon/session.rs:183-197`): `started_at`, `turn_count`, … | the daemon on resume |
| `<id>.archive.jsonl` | 72 | compacted turns | **the memory index** (turns) and `recall_context` |
| `<id>.epochs.jsonl` | 7 | epoch summaries | **the memory index** (epochs) |

`src/memory/index/mod.rs` reads only `.archive.jsonl` (`:1028-1035`) and
`.epochs.jsonl` (`:989-997`). **No index row comes from `<id>.jsonl` or
`<id>.meta.json`**, so deleting those two strands nothing in the index.

### 2. The sweep to mirror (`src/daemon/utils/mod.rs`)

`sweep_session_archives(retention_days, active_sessions)` lists the
directory, keeps names ending `.archive.jsonl`, skips files whose mtime is
newer than `now − retention_days·86400`, skips ids in `active_sessions`, and
deletes the rest. `0` returns immediately. It is called every 60th cleanup
tick (`src/daemon/mod.rs:959-966`):

```rust
                    if sweep_counter.is_multiple_of(60) {
                        crate::daemon::utils::sweep_event_segments(
                            startup_config.events.retention_days,
                        );
                        crate::daemon::utils::sweep_session_archives(
                            startup_config.sessions.archive_retention_days,
                            &active_ids,
                        );
```

Its test, `sweep_archives_respects_active_and_zero` (`src/daemon/utils/mod.rs:275`),
uses two helpers in the same test module: `create_archive_file(dir, id, data)`
and `set_file_mtime(path, days_ago)`. The new tests reuse `set_file_mtime`,
and write plain files with `std::fs::write`. The test holds
`crate::test_home_guard()` and points `HOME` at a temp dir.

### 3. The config field and its four homes

- `SessionsConfig` (`src/config/types.rs:83-99`), whose last field is:

  ```rust
      /// Delete session archive files whose mtime is older than this many
      /// days. 0 = keep forever (default).
      #[serde(default)]
      pub archive_retention_days: u32,
  ```

  plus its `Default` impl (`:130-139`).
- One struct literal outside the definition: `src/session_store_tests.rs:436-441`.
  It must gain the new field or the tree will not compile.
- The seeded config template, `assets/etc/config.toml:325-326`.
  `tests/doc_truth.rs`'s `seeded_config_template_documents_every_config_field`
  fails if a `Config` field is missing from it:

  ```toml
  # archive_retention_days = 0       # delete archived session data older than this
  #                                  # (0 = keep forever; default: 0)
  ```

- `README.md`: the `[sessions]` table row for `archive_retention_days`
  (`:1040`) and the **Retention.** paragraph (`:362`).

### 4. Gotcha: the key name already exists in `[shells]`

`[shells] log_retention_days` (the PTY shell recordings under
`var/log/shells/*.cast`) is already a key, documented in both
`assets/etc/config.toml` and `README.md`. The new key has the same name in a
different section. This is deliberate: it mirrors `[shells]`, and TOML keeps
the two apart. **Do not edit the `[shells]` entries.** It also means a grep for
the name counts both; the criteria below pin whole-word counts against
today's baseline (template **1**, README **2**).

### 5. The lifecycle row (`src/config/lifecycle.rs:81-89`)

```rust
    LifecycleEntry {
        path: "var/log/sessions",
        intent: LifecycleIntent::Sweep {
            default_retention_days: 0,
        },
        config_key: Some("sessions.archive_retention_days"),
        implemented: ImplementationStatus::Implemented,
        note: "swept but default is 0 (keep forever) — 141 archives back to May 8; fix in phase-09",
        lazy: false,
    },
```

There is one row per directory, with a single `config_key`. Keep the key and
**rewrite the `note`** so it names both keys (Task 3).

## Spec

### Task 1 — the config field (`src/config/types.rs`, `src/session_store_tests.rs`)

1. Add to `SessionsConfig`, after `archive_retention_days`:

   ```rust
       /// Delete an inactive session's chat log (`<id>.jsonl`) and its resume
       /// state (`<id>.meta.json`) once the log has not been written for this
       /// many days. 0 = keep forever (default). Archives and epoch files are
       /// never touched: they have their own lifecycle and feed the index.
       #[serde(default)]
       pub log_retention_days: u32,
   ```

2. Add `log_retention_days: 0,` to the `Default` impl and to the literal at
   `src/session_store_tests.rs:436`.

### Task 2 — the sweep (`src/daemon/utils/mod.rs`, `src/daemon/mod.rs`)

1. Beside `sweep_session_archives`, add
   `pub fn sweep_session_logs(retention_days: u32, active_sessions: &std::collections::HashSet<String>)`.
   It has the same shape: return at `0`, read `crate::config::sessions_dir()`,
   compute the same cutoff, and for each entry:
   - **select only plain logs:** the name ends with `.jsonl` **and does not**
     end with `.archive.jsonl` **or** `.epochs.jsonl`. **Gotcha:** both of
     those also end with `.jsonl`, so a bare `ends_with(".jsonl")` would delete
     indexed data;
   - the session id is the name minus `.jsonl`;
   - skip it if the `.jsonl`'s mtime is not older than the cutoff, or the id is
     in `active_sessions`;
   - otherwise delete `<id>.jsonl`, then `<id>.meta.json` if it exists.
     `log::info!` each deletion, and `log::warn!` a failed delete without
     stopping the sweep, as `sweep_session_archives` does.
2. In `src/daemon/mod.rs`, call it right after `sweep_session_archives` in the
   60-tick block, with `startup_config.sessions.log_retention_days` and the
   same `&active_ids`.

### Task 3 — docs

1. `assets/etc/config.toml`, after the `archive_retention_days` lines:

   ```toml
   # log_retention_days = 0           # delete an inactive session's chat log
   #                                  # (<id>.jsonl + <id>.meta.json) older than this
   #                                  # (0 = keep forever; default: 0)
   ```

2. `README.md`:
   - a `[sessions]` table row after `archive_retention_days`, reading
     `` | `log_retention_days` | integer | `0` | Delete an inactive session's chat log (`<id>.jsonl`) and its resume state (`<id>.meta.json`) once the log is older than this many days. `0` = keep forever. Archives and epoch files are never touched. | ``;
   - in the **Retention.** paragraph, after the archive sentence, add:
     `Per-session chat logs are kept forever unless you set `[sessions] log_retention_days`.`
3. `src/config/lifecycle.rs`: the `var/log/sessions` row's `note` becomes
   `"two keys: sessions.archive_retention_days sweeps <id>.archive.jsonl; sessions.log_retention_days sweeps <id>.jsonl + <id>.meta.json; both default 0 (keep forever)"`.
   `config_key` stays as it is.

### Task 4 — tests (`src/daemon/utils/mod.rs` tests module)

Three tests, names pinned. Each holds `crate::test_home_guard()`, points
`HOME` at a temp dir, creates `crate::config::sessions_dir()`, and ages files
with the module's `set_file_mtime(path, 30)`:

1. `sweep_logs_deletes_expired_inactive_logs_and_their_meta` — with
   `old.jsonl` and `old.meta.json` both aged 30 days,
   `sweep_session_logs(14, &empty)` deletes **both**.
2. `sweep_logs_never_touches_archives_or_epochs` — `old.archive.jsonl` and
   `old.epochs.jsonl`, both aged 30 days, with **no** `old.jsonl`:
   `sweep_session_logs(14, &empty)` leaves **both**.
3. `sweep_logs_respects_active_recent_and_zero` — `active.jsonl` (aged 30,
   `active` in the active set) and `recent.jsonl` (not aged) both survive
   `sweep_session_logs(14, &active)`. Then `stale.jsonl` (aged 30, inactive)
   survives `sweep_session_logs(0, &active)`.

**Test count:** three new, so **2098 passed / 14 ignored**.

### Task 5 — capture the end-to-end evidence

Run the block in § End-to-end verification verbatim. Paste its output **into
this phase doc** as the first fenced block of a new Update Log entry titled
exactly `### Update — <date> (end-to-end verification)`. Then run the paste
check, and **paste the line it prints into the entry on a line of its own**.

## Acceptance criteria

Parenthesised values are what each line produces **against the current
tree**. Every non-preservation line fails today; each was run at drafting.

- [ ] `cargo test --lib` → **2098 passed / 0 failed / 14 ignored** on three
      consecutive runs (current: 2095/0/14). Both counts are exact.
- [ ] `cargo fmt --all` clean; `cargo build` clean;
      `cargo clippy --all-targets --all-features -- -D warnings` exits 0.
- [ ] `cargo test` → every other target unmoved: bug_tracker 6, **doc_truth
      14** (the config-template gate must stay green), integration 32,
      isolation 10 (*preservation*).
- [ ] The three new tests pass: ``cargo test --lib sweep_logs_`` → **3
      passed** (current: `0 passed; … filtered out`).
- [ ] The archive sweep is unchanged (*preservation*):
      ``cargo test --lib sweep_archives_respects_active_and_zero`` → **1
      passed**.
- [ ] The key is documented, without touching the `[shells]` key of the same
      name: ``grep -cw 'log_retention_days' assets/etc/config.toml`` → **2**
      (current: **1**, the `[shells]` line) and
      ``grep -cw 'log_retention_days' README.md`` → **4** (current: **2**).
- [ ] The sweep is wired: ``grep -c 'sweep_session_logs(' src/daemon/mod.rs``
      → **1** (current: 0).
- [ ] Mutations A and B were applied, run, and their observed results recorded.
- [ ] The § End-to-end verification block was run verbatim and pasted as the
      **first fenced block** of the `(end-to-end verification)` entry:
      ``grep -c '^### Update — .* (end-to-end verification)$' <this doc>``
      → **1** (current: 0).
- [ ] The paste-check line is in the doc on its own line:
      ``grep -cx 'PASTE MATCH' <this doc>`` → **1** (current: 0).

## Mutations

Apply each with the `patch` tool (the executor contract forbids `sed -i`,
`perl -i`, `> file` and `rm -rf`), run once, restore, and record the observed
result.

- **Mutation A:** drop the `.archive.jsonl` exclusion from the selection.
  Run test 2. Expected **red** (the archive is deleted).
- **Mutation B:** drop the `active_sessions` check. Run test 3. Expected
  **red** (`active.jsonl` is deleted).

After each restore, confirm `git diff --stat` matches its pre-mutation state.

## Test plan

Three tests. An expired inactive log and its meta are deleted together.
Indexed files (archives, epochs) are never touched. Active sessions, recent
logs and `0` are all left alone.

## End-to-end verification

```sh
cd /home/matt/src/daemoneye
{
  for i in 1 2 3; do
    out=$(cargo test --lib 2>&1)
    printf 'lib%d=%s %s\n' "$i" "$(printf '%s\n' "$out" | grep -E '^test result' | tail -1 | grep -oE 'ok|FAILED')" "$(printf '%s\n' "$out" | grep -E '^test .* FAILED$' | sed -E 's/^test (.*) \.\.\. FAILED$/\1/' | tr '\n' ' ')"
  done
  printf 'lib_counts=%s\n' "$(cargo test --lib 2>&1 | grep -E '^test result' | tail -1 | sed -E 's/.*ok\. ([0-9]+) passed; ([0-9]+) failed; ([0-9]+) ignored.*/\1p-\2f-\3i/')"
  printf 'new_tests=%s\n' "$(cargo test --lib sweep_logs_ 2>&1 | grep -E '^test result' | tail -1 | sed -E 's/.*ok\. ([0-9]+) passed; ([0-9]+) failed.*/\1p-\2f/')"
  printf 'archive_sweep=%s\n' "$(cargo test --lib sweep_archives_respects_active_and_zero 2>&1 | grep -E '^test result' | tail -1 | grep -oE '[0-9]+ passed')"
  printf 'doc_truth=%s\n' "$(cargo test --test doc_truth 2>&1 | grep -E '^test result' | tail -1 | grep -oE '[0-9]+ passed; [0-9]+ failed')"
  printf 'docs=%s/%s\n' "$(grep -cw 'log_retention_days' assets/etc/config.toml)" "$(grep -cw 'log_retention_days' README.md)"
  printf 'wired=%s\n' "$(grep -c 'sweep_session_logs(' src/daemon/mod.rs)"
} > /tmp/e2e-m23-01d.txt
cat /tmp/e2e-m23-01d.txt
```

Expected: `lib1..3=ok` with **nothing after `ok`**, `lib_counts=2098p-0f-14i`,
`new_tests=3p-0f`, `archive_sweep=1 passed`, `doc_truth=14 passed; 0 failed`,
`docs=2/4`, `wired=1`.

**Re-run budget: three** (WORKFLOW.md § End-to-end verification). The known
pre-existing flake is `shell::backend::tests::input_after_a_prompt_logs_a_redaction_marker`.
If it fails, name it and re-run the whole block. After three re-runs, stop,
paste the last capture as it is, and name every failing test. Any other
failing test is a finding: report it by name.

**Run this block with nothing else running `cargo test`.** Paste the file into
this phase doc, then run:

```sh
D=docs/dev/milestones/M23-client-decoupling/phase-01d-session-log-retention.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-m23-01d.txt
diff /tmp/pasted-m23-01d.txt /tmp/e2e-m23-01d.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

Paste the line that command prints into the entry, **on its own line** (the
bare line, not inside backticks and not in a sentence). The mutation results
go in the same entry, after it.

## Authorizations

You may edit these files, and nothing else:

- `src/config/types.rs`, `src/session_store_tests.rs`: Task 1.
- `src/daemon/utils/mod.rs`, `src/daemon/mod.rs`: Task 2 and Task 4's tests.
- `assets/etc/config.toml`, `README.md` (the two spots named), and
  `src/config/lifecycle.rs` (the one `note`): Task 3.
- This phase doc's Update Log.

## Out of scope

- **Sweeping `<id>.epochs.jsonl`.** It is indexed, and deleting it needs the
  index cleaned too. A separate decision.
- **A startup warning** for `log_retention_days = 0`. The archive key already
  warns. A second "retention off" warning on every start is noise, and the
  default is deliberate.
- **Changing either default.** Both stay `0` (keep forever).
- **Saved sessions** under `var/sessions/`, which `session save` creates.
  They are a different directory, kept by design.

## Update Log

<!-- entries appended below this line -->
