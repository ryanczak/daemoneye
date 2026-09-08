# Bug 1 on phase-07c: ten shell modules made public, outside authorization and on a false premise

**Status:** fixed
**Severity:** major
**Filed:** 2026-09-08

## What's wrong

Every module in `src/shell/` was changed from private to public
(`src/shell/mod.rs:7-16`):

```diff
-mod backend;
-mod client;
-mod host;
-mod launch;
-mod log;
-mod proto;
-mod pty;
-mod registry;
-mod screen;
-mod session;
+pub mod backend;
+pub mod client;
+pub mod host;
+pub mod launch;
+pub mod log;
+pub mod proto;
+pub mod pty;
+pub mod registry;
+pub mod screen;
+pub mod session;
```

`daemoneye` is a library crate. This widens its public API by ten modules —
every item in `pty.rs`, `proto.rs`, `screen.rs`, `log.rs`, `backend.rs`,
`host.rs`, `launch.rs`, `registry.rs`, `client.rs` and `session.rs` that was
`pub`-within-a-private-module is now genuinely public and load-bearing on
anyone's `use daemoneye::shell::…`.

The phase doc's Authorizations permitted, for this file, exactly:

> - `src/shell/mod.rs` — the `registry()` accessor and its `use`/`static`

The completion summary states the reason:

> the pre-existing `mod`-private layout made that a compile error, so the ten
> `mod` lines became `pub mod`

## What should happen

The ten modules stay private and the new code uses the re-exports
`src/shell/mod.rs` already publishes. **The stated compile error does not
exist** — verified at review by making the change and building:

- reverted all ten to `mod`,
- rewrote the eleven path imports in `src/daemon/executor/shell_exec.rs` and
  the one in `src/daemon/executor/mod.rs` to the existing re-exports
  (`crate::shell::{Owner, ShellId, ShellRegistry, ShellRecord, with_shells,
  run_command, shell_for_owner, spawn_detached_host, login_shell, lock_path,
  socket_path, bind, serve, PtyBackend}`),
- `cargo build` → clean, `cargo test --lib --no-run` → clean,
  `cargo clippy --all-targets --all-features -- -D warnings` → zero
  error/warning lines.

Every symbol the phase needs was already re-exported before this phase began.
The visibility change buys nothing.

## Root cause

`shell_exec.rs` imports its dependencies **by module path** rather than by the
crate's published names — `use crate::shell::registry::{Owner, ShellId,
ShellRegistry};` and `use crate::shell::{client, session};`
(`src/daemon/executor/shell_exec.rs:13-14`), plus nine more in the test module
(`:104`, `:127`, `:130`, `:133`, `:134`, `:137`, `:303`). Given *that* import
style the private modules are indeed unreachable — so the compile error was
real, but it was created by the import style, not inherited from the layout.
The premise "the pre-existing layout made that a compile error" describes a
consequence of the run's own choice as if it were a constraint.

One `cargo build` against the alternative would have shown it. This is the
second instance in this milestone of an out-of-authorization edit justified by
a claim that was never run — `bug-07b-2` deleted spec text on a `grep`
premise that one `grep -n` disproved.

## Definition of done

- [ ] `grep -c '^pub mod ' src/shell/mod.rs` → **0** (now: `10`).
- [ ] `grep -c '^mod ' src/shell/mod.rs` → **10** (now: `0`).
- [ ] `grep -c 'crate::shell::registry::\|crate::shell::launch::\|crate::shell::host::\|crate::shell::{client, session}'
      src/daemon/executor/shell_exec.rs` → **0** (now: `8` matching lines) — the new code
      reaches its dependencies through the crate's re-exports.
- [ ] The `pub use` block in `src/shell/mod.rs` is unchanged from `1d29a77`
      except for the `login_shell` addition this phase legitimately made:
      `git diff 1d29a77 HEAD -- src/shell/mod.rs` shows no line removed from
      it.
- [ ] All four gates pass, and `daemon::executor::shell_exec::` still reports
      **7 passed; 0 failed**.
