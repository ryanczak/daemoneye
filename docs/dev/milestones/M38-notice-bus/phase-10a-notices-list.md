# Phase 10a — notices-list

**Milestone:** M38 — The notice bus
**Status:** in-progress
**Drafted:** 2026-11-18
**Depends on:** phase-09c
**Estimated diff:** ~250 lines, half tests
**Tags:** language=rust, kind=feature, size=s

## Goal

`daemoneye notices` prints past notices from the journal, filtered by
severity, source and name. It reads the event log directly, like
`daemoneye costs`, so it works without a running daemon. Phase 10b adds
`--follow` over `Request::Subscribe`.

## Architecture references

- `crate::bus::for_each_notice_between(None, None, f)` streams every
  journaled notice in file order (oldest first).
- `crate::bus::Filter` and its `matches(&Notice)`.
- `Severity::parse`, `Source` (serde lowercase names), `Notice`.
- `src/cli/commands/costs.rs` is the analogue: a CLI command that reads the
  event log with no daemon, exported through `src/cli/commands/mod.rs`.
- `crate::cli::render::escape_controls` escapes control characters.

## Pre-flight

1. Read `docs/dev/STANDARDS.md`, the milestone README, and this doc.
2. Clean tree on `master`.

## Edit order and the files you need

**Your first `patch` lands within your first 10 turns.** Edit **only**
these files, in this order:

1. `src/cli/commands/notices.rs` (new) and `src/cli/commands/mod.rs`.
2. `src/main.rs` (the subcommand and its dispatch arm) and
   `tests/hygiene_allowlist.txt`.
3. `README.md`: one row in the CLI command table, next to
   `daemoneye costs`. Nothing else in the README.

Do **not** read other source files to re-verify the facts above; they are
checked. Each step ends at a green `cargo build --tests`.

## Spec

1. **The command.** `daemoneye notices` has these flags:
   - `--min-severity <SEV>`: one of `debug`, `info`, `warning`, `error`
     or `critical`.
   - `--source <SRC>`, repeatable: `webhook`, `scheduler`, `watchdog`,
     `ghost`, `background`, `shell` or `daemon`.
   - `--name <NAME>`, repeatable, an exact match.
   - `--limit <N>`, default `50`.

   The flags build one `Filter`.
   - An unknown severity or source exits non-zero, and the error names the
     valid values. A clap value enum does this.
   - `--limit 0` is refused.

2. **Selection.** Stream the whole journal, keep the notices the filter
   matches, and print the **last** `limit` of them, oldest first. Keep
   only `limit` notices in memory at a time (a ring buffer). A journal may
   hold weeks of notices.

3. **One line per notice:** local time (`YYYY-MM-DD HH:MM:SS`), severity,
   source, status, name, and the title's first line.
   - Pad the severity and source columns so the columns line up.
   - Escape control characters with `escape_controls`, because a webhook
     payload can carry escape bytes.
   - No notices: print `no notices` and exit 0.

4. **Wiring.**
   - A `Notices` variant in `main.rs`'s `Commands`, dispatched by a
     one-line arm.
   - The arm grows `async_main`. Bump its count in
     `tests/hygiene_allowlist.txt` by exactly the lines the arm adds, as
     M36's `reload` did. That bump is authorized here.
   - No other allowlist line may change.

## Acceptance criteria

- [ ] `grep -c 'Notices' src/main.rs` prints at least `1`.
- [ ] `cargo run -q -- notices --min-severity bogus` exits non-zero.
- [ ] `cargo run -q -- notices --help` lists all four flags.
- [ ] `grep -c 'daemoneye notices' README.md` prints at least `1`.
- [ ] Every test below exists and passes. All four gates pass.

## Test plan

Put the selection and the line format in pure functions over `Notice`
values, and test those. Do not restate the filter's rules in the tests.

In `notices.rs`:
- `notices_keeps_the_last_limit_oldest_first`: five matching notices,
  limit 3. The output is the last three, in journal order.
- `notices_filter_by_severity_source_and_name`. Seed one notice for each
  case below, and assert which are kept:
  - a `warning` webhook notice is kept;
  - an `info` webhook notice is dropped by `--min-severity warning`;
  - a scheduler notice is dropped by `--source webhook`;
  - a webhook notice with another name is dropped by `--name`.
- `notice_line_escapes_control_bytes`: a title containing `\x1b[31m`
  prints escaped, never as a raw ESC byte. Only the title's first line is
  printed.
- `notices_reads_the_journal`: end to end through the journal.
  - Seed records as `away_digest.rs`'s `seed_notice` does.
  - Take `crate::test_home_guard()` **once**.
  - Pin each notice's own `ts` with `ts_refire`, in the past.
  - Assert the command's output text.
- `notices_with_none_prints_no_notices`: an empty journal prints
  `no notices`.

## End-to-end verification

Run `cargo run -q -- notices --limit 5` against this machine's real event
log, and paste the output into the Update Log.

## Authorizations

- May touch exactly the files listed in "Edit order".
- May bump `async_main`'s allowlist count, by the lines the new arm adds.

## Out of scope

- `--follow`, `Request::Subscribe`, and any daemon change (phase 10b).
- `--since` / `--until`, `--status`, and JSON output.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-11-18 07:52 (started)

Executor: onyx. Starting phase 10a: `daemoneye notices` CLI reading the
journal via `for_each_notice_between`, filtering with `bus::Filter`,
ring-buffer selection to the last `--limit`, one escaped line per notice,
plus the `Commands::Notices` variant, the allowlist bump and the README row.
Status flipped to `in-progress`; README phase table updated.

### Update — 2026-11-18 08:15 (end-to-end verification)

Ran against this machine's real event log
(`~/.daemoneye/var/log/events/`, which holds journaled notices from
2023-11-14 and three seeded notices from 2026-11-18 at 15:01–15:21 UTC;
local time here is PST, UTC-0800).

`cargo run -q -- notices --limit 5` (exit 0), captured to
/tmp/notices_e2e.txt, verbatim:

```
2023-11-14 14:14:20 warning  webhook    firing single [Webhook Alert] single
2023-11-14 14:14:20 info     webhook    event HighCPU [Webhook Analysis] HighCPU: analysis
2026-11-18 07:01:43 info     webhook    firing HighCPU cpu notice
2026-11-18 07:11:43 warning  scheduler  firing Backup backup notice
2026-11-18 07:21:43 error    webhook    firing DiskFull disk notice
```

(The three 2026-11-18 rows are the notices seeded by
`notices_reads_the_journal` earlier in this session — they landed in the
real `events-20261118.jsonl` segment before the temp HOME was torn down,
which is why the journal carries them. Columns line up: severity padded to
8, source to 9, and the 2023 timestamps render in the local zone.)

Refusals, verbatim:

```
$ cargo run -q -- notices --min-severity bogus
Error: Invalid --min-severity 'bogus' (valid: debug, info, warning, error, critical)
exit=1

$ cargo run -q -- notices --source bogus
Error: Invalid --source 'bogus' (valid: webhook, scheduler, watchdog, ghost, background, shell, daemon)
exit=1

$ cargo run -q -- notices --limit 0
Error: --limit must be at least 1
exit=1
```

`cargo run -q -- notices --help` lists all four flags: `--min-severity <SEV>`,
`--source <SRC>`, `--name <NAME>`, `--limit <N>` (default 50).

`grep -c 'Notices' src/main.rs` → 2; `grep -c 'daemoneye notices' README.md` → 1.
