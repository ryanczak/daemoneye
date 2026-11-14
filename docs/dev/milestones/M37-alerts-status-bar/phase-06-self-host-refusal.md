# Phase 06 — self-host-refusal

**Milestone:** M37 — Alerts out of history, onto the status bar and `top`
**Status:** todo
**Drafted:** 2026-11-14
**Depends on:** nothing in M37
**Estimated diff:** ~160 lines, about half of it tests
**Tags:** language=rust, kind=bugfix, size=s

## Goal

GitHub issue #8. A `[hosts.<name>]` entry whose `ssh` destination is the
daemon's own machine makes `host: "<name>"` open an ssh hop to itself,
which fails in a non-interactive PTY on the host-key prompt. PE decision
(2026-11-14): **warn and refuse**, no hostname heuristics decide where a
command runs. Config load warns once per such entry; a tool call naming
that host is refused before any prompt, with one sentence telling the
model to omit `host`. Nothing is routed local on the daemon's guess.

## Architecture references

- `src/host.rs`: `HostId`, `HostId::parse`.
- `src/daemon/utils/host.rs`: `daemon_hostname()`.
- `src/config/types.rs`: `Config::validate_hosts` (returns one problem
  string per bad entry; `run_daemon` logs each at `warn`).
- `src/daemon/executor/host_gate.rs`: `resolve_host`, the prologue every
  host-taking arm calls before its prompt. `src/daemon/executor/file_ops/write.rs`:
  `resolve_edit_host`, its `edit_file` mirror.

## Spec

Numbered tasks in execution order. The executor's task list is seeded from
this section: mark each one `active` when you start it and `done` when its
`cargo build` is green.

1. **The predicate** — `pub fn is_self_destination(ssh: &str,
   daemon_hostname: &str) -> bool` in `src/host.rs`, pure. The host part of
   `ssh` is what remains after dropping an `ssh://` scheme, a `user@`
   prefix and, only in the scheme form, a `:port` suffix; surrounding
   whitespace is trimmed. It is a self destination when, case-insensitively,
   it equals `daemon_hostname`, or equals `daemon_hostname`'s first
   dot-separated label, or is `localhost`, `127.0.0.1`, `::1` or `[::1]`.
   Nothing else matches: `pinky2`, `pinky-gpu`, `notpinky`, `pinky.lan`
   when the daemon hostname is `pinky` (an alias that may or may not be this
   machine is **not** second-guessed), and a bare `~/.ssh/config` alias that
   does not spell the hostname. An empty `daemon_hostname` or `"unknown"`
   matches only the loopback names.
2. **The refusal** — `pub(crate) fn self_host_refusal(host: &HostId, cfg:
   &Config, daemon_hostname: &str) -> Option<String>` in `host_gate.rs`,
   pure: `None` for `HostId::Local` and for a name `cfg.hosts` does not
   hold; for a configured entry whose `ssh` is a self destination, the
   sentence: `Error: host <name> is this machine ([hosts.<name>] ssh =
   "<ssh>" names the daemon host), so there is nothing to connect to. Omit
   \`host\` to run here.` `resolve_host` and `resolve_edit_host` call it
   after a successful parse with `config::current()` and
   `daemon_hostname()`, so every host-taking arm
   (`run_terminal_command`, `read_file`, `edit_file`, `open_shell`) refuses
   before its prompt and no arm changes. The two resolvers' tests that pin
   "an unconfigured host is not refused" keep passing: an unconfigured name
   has no `ssh` to judge.
3. **The load warning** — `Config::validate_hosts` is unchanged in
   signature and output. A new `pub fn self_host_problems(&self,
   daemon_hostname: &str) -> Vec<String>` beside it returns, per entry
   whose `ssh` is a self destination: `[hosts.<name>] ssh = "<ssh>" names
   the daemon host (<daemon_hostname>); host: "<name>" will be refused —
   omit host to run here, or point ssh at another machine.` `run_daemon`
   logs each at `warn` in the same loop that logs `validate_hosts`. The
   client-side `Config::load` path does not log it (a CLI has no daemon
   hostname to compare against that matters).
4. **Docs** — README § `[hosts.<name>]`: one sentence after the "unknown
   host key is refused" paragraph saying an entry that names the daemon's
   own machine is warned about at startup and its `host:` is refused.
   Mention the refusal in the `run_terminal_command` row's `host?` clause of
   CLAUDE.md's tool table ("a configured host that is the daemon's own
   machine is refused").
5. **Capture the evidence** — run the mutation check and the § End-to-end
   block verbatim, and paste both into one Update Log entry.

## Constraints

- No routing decision: a self destination is **refused**, never silently
  run locally. The daemon never compares anything but the `ssh` string
  against `daemon_hostname()`; no DNS, no `getaddrinfo`, no reading
  `~/.ssh/config`.
- The resolvers read `config::current()` (never `Config::load()`) and
  `daemon_hostname()`; `self_host_refusal` and `is_self_destination` take
  the config and the hostname as arguments, so their tests pass a `Config`
  parsed from a TOML string and a fixed hostname — never the machine's
  real hostname, never an installed handle (unit tests never install one;
  uninstalled, `current()` loads from the test's throwaway `$HOME`, which
  holds no `[hosts]`, so the existing resolver tests keep seeing no host).
- A ghost's `ssh_target` resolution (`src/host.rs`, `SshTargetResolution`)
  is untouched; `refuse_ghost_host` is untouched.
- Tests are hermetic: `is_self_destination` tests are pure;
  `self_host_problems` tests build a `Config` from a TOML string;
  the `self_host_refusal` tests call the pure function directly.

## Test plan

In `src/host.rs` (its test module):

- `self_destination_matches_the_daemon_hostname_and_loopbacks`: `pinky`,
  `PINKY`, `matt@pinky`, `ssh://matt@pinky:2222`, `localhost`,
  `127.0.0.1`, `::1`, `[::1]` against hostname `pinky.example.org`'s first
  label and against `pinky` all return true.
- `self_destination_does_not_match_other_hosts_or_aliases`: `pinky2`,
  `pinky-gpu`, `notpinky`, `pinky.lan`, `zoidberg`, `matt@zoidberg`, `gpu`
  (an alias) against `pinky` all return false; `pinky` against `zoidberg`
  returns false.

In `src/config/tests.rs`, following `validate_hosts_reports_empty_ssh_and_reserved_local`:

- `self_host_problems_names_the_entry_that_is_this_machine`: two entries,
  one `ssh = "pinky"` and one `ssh = "zoidberg"`, hostname `pinky`: one
  problem, naming `[hosts.<name>]`, the `ssh` value and `pinky`.
- `self_host_problems_is_empty_when_no_entry_is_this_machine`.

In `src/daemon/executor/host_gate.rs` (its test module), following
`an_unconfigured_host_is_not_refused_before_the_prompt`:

- `a_configured_self_host_is_refused_before_the_prompt`: with a `Config`
  parsed from `[hosts.me]\nssh = "pinky"` and hostname `pinky`,
  `self_host_refusal(&HostId::Named("me"), &cfg, "pinky")` is `Some` and
  the sentence names `me`, `pinky` and says to omit `host`.
- `a_configured_remote_host_is_not_refused_before_the_prompt`: the same
  with `ssh = "zoidberg"` is `None`; so is `HostId::Local`, and so is a
  name the config does not hold.

In `src/daemon/executor/file_ops/` (where `resolve_edit_host`'s existing
host tests live):

- `resolve_edit_host_consults_the_self_host_refusal`: a test at the seam
  the executor finds simplest — either `resolve_edit_host` reads the
  config through an injectable parameter it gains for the purpose, or the
  acceptance grep below is the pin (the call must be present; the
  behaviour is the pure function's, already tested).

## Acceptance criteria

- [ ] Every test in the test plan passes, and
      `an_unconfigured_host_is_not_refused_before_the_prompt` passes
      unchanged.
- [ ] `grep -c 'is_self_destination' src/host.rs` prints at least 2 (today
      0); `grep -c 'self_host_refusal' src/daemon/executor/host_gate.rs
      src/daemon/executor/file_ops/write.rs` prints a non-zero count for
      both files (today 0 and 0); `grep -c 'self_host_problems'
      src/daemon/startup.rs` prints at least 1 (today 0).
- [ ] Mutation check, pasted with its `test result:` line: make
      `is_self_destination` return `false` unconditionally, confirm
      `a_configured_self_host_is_refused_before_the_prompt` fails, then
      revert.
- [ ] fmt, build (0 warnings), clippy and `cargo test` are green, with
      `failed=0` and more than 2965 passed. Hygiene, `doc_truth` and
      `bug_tracker` are ok.

## Pre-flight

Baseline at drafting (2026-11-14, `9003f675`): `cargo test` passed=2965, failed=0.

## End-to-end verification

Run mechanically, with output redirected to a file and pasted:

```sh
cargo test --lib self_destination 2>&1 | grep -E '^test result'
cargo test --lib self_host 2>&1 | grep -E '^test result'
grep -c 'self_host_problems' src/daemon/startup.rs | sed 's/^/startup_warns=/'
cargo build 2>&1 | grep -c '^warning' | sed 's/^/build_warnings=/'
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
```

## Authorizations

- Edit `src/host.rs`, `src/config/types.rs`, `src/config/tests.rs`,
  `src/daemon/executor/host_gate.rs`, `src/daemon/executor/file_ops/write.rs`
  and its test file, `src/daemon/startup.rs` (the warning loop only),
  `README.md`, `CLAUDE.md`.

## Out of scope

- Routing a self destination locally, with or without an opt-in key
  (option 3 in #8, declined).
- `daemoneye doctor` and `/hosts` surfacing the warning.
- A ghost runbook `ssh_target` that names the daemon host.
- Any DNS or `~/.ssh/config` resolution.

## Update Log

<!-- entries below this line -->
