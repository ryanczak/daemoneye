# Bug 1 on phase-21d: the attach tests drive a test-only copy of the opener, and an `#[allow(dead_code)]` hides the held write half

**Severity:** major
**Status:** fixed (phase-21d rework, 2026-11-12)
**Filed:** 2026-11-12

## What's wrong

1. **The tests never run the production opener.** Spec step 3 asked for
   one seam, a function that takes an already-connected
   `tokio::net::UnixStream` and does everything after the connect, so the
   tests could drive *the code production runs*. That function exists:
   `attach_after_connect` (`src/cli/commands/top_tui_attach.rs:109`). But
   the tests call a second function, `attach_after_connect_with_frames`
   (line 172, `#[cfg(test)]`). It repeats the request send and the task
   spawn, and it skips the part that matters: reading the first frame off
   the socket. `opened_unix` hands the model its first frame directly.
   `attach_after_connect` never runs under any test, so a regression in it
   (a wrong request, a mis-read first frame, a write half that isn't the
   connection's) passes every test.
   A side effect is that `top_attach_a_frame_after_open_reaches_the_view`
   has to explain away a frame "the opener's first read already consumed",
   which never happens on its path.
2. **A lint-silencing shim.** `AttachView.write` carries
   `#[allow(dead_code)]` (line 40). STANDARDS.md § Definition of Done
   forbids `#[allow(...)]` used to mask a diagnostic, and the file had none
   before this phase.

## What should happen

- Both new tests (`top_attach_a_frame_after_open_reaches_the_view`,
  `top_attach_close_hangs_up`) build their view through
  `attach_after_connect` itself. The fake daemon, on the other end of a
  `UnixStream::pair()`, reads the request line and **writes the
  `ShellScreen` first frame** that `attach_after_connect` reads. The opener
  and the fake daemon must run concurrently: `tokio::join!` or a spawned
  task, with every read bounded by `tokio::time::timeout`.
- `attach_after_connect_with_frames` is gone. No `#[cfg(test)]` copy of
  production logic remains.
- No `#[allow` in the file. A field held only so that dropping it has an
  effect still has to compile warning-free without a lint attribute.

## Root cause

The first frame was injected instead of written by the fake daemon. That
created the need for a variant that skips the read, and the variant then
replaced the seam in the tests. Nothing reads the field, so the compiler
warned, and the warning was silenced rather than designed away.

## Definition of done

- [ ] `grep -c 'attach_after_connect_with_frames' src/cli/commands/top_tui_attach.rs`
      prints 0. Today 2.
- [ ] `grep -c '#\[allow' src/cli/commands/top_tui_attach.rs` prints 0.
      Today 1.
- [ ] `top_attach_a_frame_after_open_reaches_the_view` and
      `top_attach_close_hangs_up` pass, each calling `attach_after_connect`
      (`grep -c 'attach_after_connect(' src/cli/commands/top_tui_attach.rs`
      prints 3 or more: the definition, the production call, and the test
      helper both tests use). Today 2.
- [ ] `cargo test` reports `passed=2887`, **not more**. This fix moves
      tests onto the real seam and adds none.
