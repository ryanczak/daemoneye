### Update — 2026-09-12 18:40 (end-to-end verification)

Second, complete run of the phase doc's `## End-to-end verification` block,
verbatim from the repo root, transcript captured mechanically into
`/tmp/e2e-m24-10.txt` and pasted below unedited. (Supersedes the 18:35 entry:
its `one read_turn_log` grep printed 2 because the module rustdoc named
`read_turn_log` in a doc link; after rewording the header the prod count is 1
as pinned, and the rerun includes an extra C2 section showing the real CLI
artifact on an empty runs dir.) All values match the phase's stated numbers.
The wording deviation the phase's Goal asks to be recorded: the command is
`daemoneye run list`, not `daemoneye runs` — the `Run` subcommand group from
phase-09b is the one noun; see § Goal's naming note.

```sh
== A. build ==
real HOME run logs BEFORE: 0
   Compiling daemoneye v1.0.0 (/home/matt/src/daemoneye)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 2.17s
cargo_exit=0
== B. the listing module ==
test daemon::run::list::tests::render_runs_table_empty_says_no_runs ... ok
test daemon::run::list::tests::summarize_takes_the_highest_turn_not_the_count ... ok
test daemon::run::list::tests::summarize_without_a_run_finished_reports_running ... ok
test daemon::run::list::tests::list_runs_on_a_missing_directory_is_empty ... ok
test daemon::run::list::tests::render_runs_table_lists_every_kind ... ok
test daemon::run::list::tests::summarize_reads_the_kind_and_the_terminal_state ... ok
test daemon::run::list::tests::summarize_without_a_run_started_is_none ... ok
test daemon::run::list::tests::render_runs_table_names_each_terminal_state ... ok
test daemon::run::list::tests::list_runs_skips_a_name_that_is_not_a_run_id ... ok
test daemon::run::list::tests::list_runs_reads_a_written_log ... ok
test daemon::run::list::tests::list_runs_skips_a_log_with_no_run_started ... ok
test result: ok. 11 passed; 0 failed; 0 ignored; 0 measured; 1807 filtered out; finished in 0.00s
cargo_exit=0
== C. the CLI surface exists ==
Manage finished runs

Usage: daemoneye run <COMMAND>

Commands:
  list    List every run log in ~/.daemoneye/var/log/runs/, newest first
  resume  Resume a finished ghost run from its briefing plus a new directive
  help    Print this message or the help of the given subcommand(s)

Options:
  -h, --help  Print help
cargo_exit=0
== C2. daemoneye run list on an empty runs dir ==
no runs recorded yet
cargo_exit=0
== D. structural greps (each must print the stated number) ==
list.rs exists             (1): 1
mod list;                  (1): 1
pub use list::             (1): 1
RunSummary                 (1): 1
summarize                  (1): 1
list_runs                  (1): 1
render_runs_table          (1): 1
summarize is pure          (0): 0
render is pure             (0): 0
one read_turn_log          (1): 1
one runs_dir               (1): 1
terminal_outcome unused    (0): 0
empty message literal    (>=1): 2
RunCommands variants       (2): 2
RunCommands::List          (1): 1
slash /runs                (1): 1
slash /run                 (1): 1
slash /stop                (1): 1
slash no read_turn_log     (0): 0
slash no sentinel path     (0): 0
new arms hit no daemon     (0): 0
list.rs unwrap/panic       (0): 0
== E. preservation: log_read 11, resume 20, turn_log 7, guard 8, ghost 21, runbook 15, slash 8 ==
daemon::run::log_read        test result: ok. 11 passed; 0 failed; 0 ignored; 0 measured; 1807 filtered out; finished in 0.01s
daemon::run::resume          test result: ok. 20 passed; 0 failed; 0 ignored; 0 measured; 1798 filtered out; finished in 0.00s
daemon::run::turn_log        test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 1811 filtered out; finished in 0.00s
daemon::run::guard           test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 1810 filtered out; finished in 0.00s
daemon::ghost                test result: ok. 21 passed; 0 failed; 0 ignored; 0 measured; 1797 filtered out; finished in 0.00s
runbook::                    test result: ok. 15 passed; 0 failed; 0 ignored; 0 measured; 1803 filtered out; finished in 0.00s
cli::commands::slash         test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 1810 filtered out; finished in 0.00s
== F. lib suite, three consecutive runs ==
test result: ok. 1814 passed; 0 failed; 4 ignored; 0 measured; 0 filtered out; finished in 8.10s
test result: ok. 1814 passed; 0 failed; 4 ignored; 0 measured; 0 filtered out; finished in 8.10s
test result: ok. 1814 passed; 0 failed; 4 ignored; 0 measured; 0 filtered out; finished in 8.10s
== G. gates ==
fmt_exit=0
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.07s
clippy_exit=0
== H. no HOME litter from the suite ==
real HOME run logs AFTER (==BEFORE): 0
```
