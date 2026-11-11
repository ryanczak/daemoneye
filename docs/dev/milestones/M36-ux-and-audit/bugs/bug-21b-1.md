# Bug 1 on phase-21b: three Test plan tests are missing, and a silent host wipes stored records

**Severity:** major
**Status:** fixed (phase-21b resume, 2026-11-11)
**Filed:** 2026-11-10

## What's wrong

1. **Three tests named in the Test plan don't exist:**
   - `program_status_surface_list_shells_shows_a_blocked_report_without_a_screen_read`.
     This is the milestone's exit criterion: a real host whose program
     reported `blocked:kind=permission:app=x` is listed with `x` and
     `blocked`, and nothing ever called `read_shell_screen`.
   - `program_status_surface_list_shells_survives_a_host_that_does_not_answer`.
   - `program_status_surface_shell_info_from_an_older_daemon_reads_empty_programs`.

   `cargo test --lib program_status_surface` runs 9 tests, and two of
   those are extras. The end-to-end run never named the missing three.
2. **A host that doesn't answer clears the stored records.**
   `refresh_programs` (`src/daemon/executor/knowledge/shell.rs`) maps
   every non-`Status` result, a timeout included, to `Vec::new()`, and then
   stores that. Behaviour 1 says such a host "keeps whatever was stored
   before". The function's own doc comment says the same thing ("keeps the
   records it had"), so the code contradicts its documentation.

## What should happen

- All three tests exist and pass.
- The no-answer test proves both halves of Behaviour 1: the call returns
  within the bound and lists the shell, and records stored before the
  silent probe are still listed afterwards.

## Root cause

The executor implemented ahead of its tests and never re-checked the Test
plan's names. Folding the failure into an empty list was the shortest
match arm.

## Definition of done

- [x] The three named tests exist and pass. (2026-11-11 resume: all three
      in `shell_observer_tests.rs` / `ipc_tests.rs`, printed by the phase's
      end-to-end run.)
- [x] `…_list_shells_survives_a_host_that_does_not_answer` stores a record
      on a shell whose host doesn't answer, calls `list_shells`, and still
      sees the record. The mutation check (storing `Some(Vec::new())` on a
      non-`Status` reply) makes it fail; the committed code stores `None`
      and keeps what was there.
