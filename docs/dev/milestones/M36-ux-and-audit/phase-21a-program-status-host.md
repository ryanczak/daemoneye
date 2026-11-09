# Phase 21a — program-status-host

**Milestone:** M36 — Chat UX and audit fixes
**Status:** in-progress
**Drafted:** 2026-11-09
**Depends on:** phase 20b
**Estimated diff:** ~450 lines, half of it tests
**Tags:** language=rust, kind=feature, size=m

## Goal

Decision 9, the consume side's first half. The shell host parses the OSC 7501
reports the programs in its PTY emit into per-shell records. The records
follow the spec's grammar, limits, lifetime and security rules. The host
answers the feature query and writes nothing else back. The `Status` frame
carries the records, and `read_shell_screen`'s header shows them masked.
Phase 21b brings them to `list_shells`, `top` and `watch_shell`. Phase 21c is
the chat client's own emit side.

## Architecture references

- [`osc7501-spec.md`](osc7501-spec.md) is the spec digest and the authority
  for this phase. Read all of it before the Spec. Its Syntax, States, Keys,
  Records, Limits and Security sections are each pinned by a test below.
- `src/shell/screen.rs` (`ShellScreen`, which wraps `vt100::Parser`),
  `src/shell/backend.rs` (`PtyBackend`: the pump thread, `status()`),
  `src/shell/sbx_backend.rs` (`SbxBackend::status`, which rebuilds the
  frame), `src/shell/proto.rs` (`ShellResponse::Status`),
  `src/daemon/executor/knowledge/shell_screen.rs` (`format_screen_read`).
- vt100 0.18 API (in `~/.cargo/registry`): `vt100::Callbacks::unhandled_osc(&mut self, screen, params: &[&[u8]])`,
  `Parser::new_with_callbacks(rows, cols, scrollback, cb)` and
  `Parser::callbacks_mut()`.

## Behaviour

1. **Parsing** happens in a new pure module, `src/shell/program_status.rs`.
   It turns a payload (the bytes after `7501;`) into a report, or drops it,
   exactly as the digest's Syntax, States, Keys and Limits sections say. A
   malformed pair is skipped and the rest applies. Unknown keys are ignored,
   and the last of a repeated key wins. A `progress` that is not an integer
   from 0 to 100 is a malformed pair. A `kind` outside the three is absent,
   and so is any `kind` on a state other than `blocked`. Each of these drops
   the report whole: an unknown or missing `state`, an `id` that fails the
   grammar, a broken limit, bad base64, or decoded text holding a control
   character. A payload over 4088 bytes counts as the 4096-byte sequence
   limit.
2. **The record table** lives in the same module. It holds one record per
   id per shell, and the root record has no id. A report replaces its record
   whole. `clear` with an id removes that record and every record beneath
   it; `clear` without one empties the table. At 256 records the least
   recently updated record goes. A record's effective `app` is its own `app`,
   or else its nearest ancestor's.
3. **Lifetime.** A `working` or `blocked` record is dropped once the program
   that reported it no longer holds the terminal. That covers two cases:
   the shell is back at its own prompt, or a different process group is in
   the foreground. `idle`, `done` and `error` records stay until they are
   replaced or cleared. The check may run lazily when the records are read.
   A read must never return a stale `working` record.
4. **The host.** `ShellScreen` becomes the vt100 callback. A 7501 report goes
   to the table. The query `OSC 7501 ; ? ST` gets the reply `ESC ] 7501 ; ? ESC \`
   written to the shell's PTY input, and that reply is the only byte the host
   ever writes back for this protocol. Every other OSC is ignored as it is
   today.
5. **The frame.** `ShellResponse::Status` gains `programs`, a list of
   records. Each record carries its id (or none), state, kind, progress,
   effective app, title and msg, ordered by id. The field is
   `#[serde(default)]`, so a host from before this phase deserializes as
   empty. `SbxBackend::status` passes the inner host's `programs` through.
6. **`read_shell_screen`.** The header gains one line per record, below the
   existing header line. The line names the app, the state (with its kind),
   the progress, the id when there is one, and the title and msg. Title and
   msg go through `mask_sensitive`, and their controls and bidi characters
   are made visible the way `escape_controls` does it. With no records, the
   header is unchanged.

## Constraints

- **Gotcha: vte splits an OSC on `;`.** A legal report arrives as exactly
  `[b"7501", payload]`, and the query as `[b"7501", b"?"]`. The value grammar
  has no `;`, so any other shape that starts with `7501` is dropped.
- **Gotcha: vte's `std` feature does not cap OSC length.** The callback can
  see a sequence of any size. The limits are this module's job.
- **Lock order.** `PtyBackend::status` takes `shell` and then `screen`. Do
  not lock `shell` while holding `screen`. If the pump needs the shell's
  foreground group to stamp a report, it takes it after the screen lock is
  released.
- Add `base64 = "0.22"`. Version 0.22.1 is already in `Cargo.lock`. Decode
  the standard alphabet with optional padding.
- `msg` and `title` are data. Never interpret, render or log them unmasked.
- Do not change `Foreground`, `classify_record`, `list_shells`,
  `watch_shell`, `top` or the chat client. Those belong to 21b and 21c.

## Spec

1. **Red tests first.** Write the tests in the Test plan. Run
   `cargo test --lib program_status` and `cargo test --lib backend_` and
   paste the real failing output (compile errors count) into an
   `(red runs)` entry. The step ends when both runs are pasted.
2. **Write the parser and table** (Behaviour 1–3). The step ends when
   `cargo test --lib program_status` passes.
3. **Wire the host and frame** (Behaviour 4–5). The step ends when the
   backend, proto and sbx tests pass.
4. **Wire the screen header** (Behaviour 6). The step ends when its test
   passes.
5. **End-to-end:** run § End-to-end verification once, redirected to
   `.rexymcp/output/p21a-e2e.txt`. Paste that file verbatim into an
   `(end-to-end verification)` entry that names it.
6. **Findings:** a `### Findings — <date>` entry of two or three lines, or
   `none`. Name any allowlist raise.
7. **Commit** as
   `feat: OSC 7501 consume — shell hosts keep program-status records, shown in read_shell_screen (M36 phase-21a)`.

## Test plan

Pure tests in `program_status.rs`, each named `program_status_…`:
`…_parses_a_report_into_a_record` (every key, base64 with and without
padding); `…_replaces_a_record_whole`; `…_skips_a_malformed_pair_and_unknown_keys`;
`…_drops_an_unknown_or_missing_state`; `…_drops_a_bad_id` (a 33-byte
segment, 9 levels, a bad character; the root record must be unchanged);
`…_drops_a_report_over_a_limit` (msg, title, app, key, payload, each at its
limit and one past it); `…_drops_bad_base64_and_control_characters` (a
decoded `\n`, `\t` and U+009B); `…_clear_removes_the_subtree` (`clear` on
`a` keeps `ab`, and `clear` without an id empties the table);
`…_evicts_the_least_recently_updated_at_the_cap`;
`…_app_inherits_from_the_nearest_ancestor`.

Host and frame tests: `backend_status_carries_a_working_report_while_its_program_runs`
(real bash: a command that prints a `working` report and then sleeps shows
it during the sleep, and once the command ends it is gone);
`backend_status_keeps_a_done_report_after_its_program_exits`;
`backend_answers_the_feature_query` (a command that sends the query and
reads its stdin receives `ESC]7501;?`); `screen_feed_writes_back_only_the_feature_query`;
`status_frame_without_programs_deserializes_empty`;
`sbx_status_passes_programs_through`; `format_screen_read_lists_program_records_masked`
(an `sk-` key in msg comes back masked, and a bidi override is made
visible).

## Acceptance criteria

- [ ] Every test named in the Test plan passes. Today none exists.
- [ ] `base64_dep=1`: `Cargo.toml` names `base64`. Today 0.
- [ ] Hygiene, `doc_truth`, `bug_tracker` and `lock_invariant` are ok;
      `build_warnings=0`, clippy is clean, `fmt_exit=0`, `failed=0`.

## End-to-end verification

```sh
bash <<'E2E'
cargo test --lib program_status 2>&1 | grep -E '^test result'
for t in backend_status_carries_a_working_report_while_its_program_runs backend_status_keeps_a_done_report_after_its_program_exits backend_answers_the_feature_query screen_feed_writes_back_only_the_feature_query status_frame_without_programs_deserializes_empty sbx_status_passes_programs_through format_screen_read_lists_program_records_masked; do
  cargo test --lib "$t" 2>&1 | grep -E '^test result' | awk -v t="$t" '{p+=$4; f+=$6} END {print t, "passed=" p, "failed=" f}'
done
grep -c '^base64' Cargo.toml | sed 's/^/base64_dep=/'
touch src/main.rs && cargo build 2>&1 | grep -c '^warning' | sed 's/^/build_warnings=/'
for t in hygiene doc_truth bug_tracker lock_invariant; do cargo test --test $t 2>&1 | grep -E '^test result'; done
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1
cargo fmt --all --check; echo "fmt_exit=$?"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
E2E
```

## Authorizations

- **Edit:** `src/shell/program_status.rs` (new) and `src/shell/mod.rs`;
  `src/shell/screen.rs`, `backend.rs`, `backend_tests.rs`, `proto.rs`,
  `sbx_backend.rs`, and any test file that builds a `ShellResponse::Status`
  literal; `src/daemon/executor/knowledge/shell_screen.rs`; `Cargo.toml`
  and `Cargo.lock` for `base64`.
- **Allowlist:** if the hygiene gate reports a file or function over its
  cap, raise its entry and name it in Findings. Do not split code to fit.
- **Scratch files** go under `.rexymcp/output/`.

## Out of scope

- `list_shells`, `ShellInfo`, `top`, `watch_shell` and the program-over-screen
  precedence (21b); the chat client's emit and tmux passthrough (21c).
- RIS clearing the table. vt100 consumes `ESC c` with no callback, and a full
  reset inside a daemon shell is rare. It is noted for 21b's live check.
- `states=`/`kinds=` in the query reply (the spec makes them optional).
- Desktop notifications or any other external effect of a report.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-11-09 21:45 (started)

Status flipped to `in-progress`; milestone README row matches. Executor:
Claude Opus 4.5 (1M), headless.
