# Bug 1 on phase-11: streaming interrupt drops the daemon `recv` future on every keypress/spinner tick, corrupting the stream

**Severity:** major
**Status:** fixed (architect takeover, 2026-06-27)
**Filed:** 2026-06-27

## What's wrong

The interrupt wiring in `ask_with_session_ratatui` (`src/cli/commands/stream.rs`)
constructs a **fresh** daemon-`recv` future *inside* the `tokio::select!` on **every**
iteration of the inner loop:

```rust
// src/cli/commands/stream.rs  (phase 2, ~lines 223–268; phase 1 is the same shape)
let phase2_result = loop {
    tokio::select! {
        biased;
        key = read_key(stdin) => { /* Ignore → continue; Warn → draw + continue; Abort → break */ }
        res = tokio::time::timeout(std::time::Duration::from_secs(120), recv(&mut rx)) => { … }
    }
};
```

`tokio::select!` **drops the not-selected branch's future** when another branch completes;
`biased` only fixes the *poll order*, it does **not** preserve a dropped branch's progress.
Because `recv(&mut rx)` is called *inside* the select, the recv future is recreated each
iteration and discarded whenever the `read_key` branch wins (any keypress) or — in phase 1 —
whenever the 80 ms spinner timeout fires.

`recv` consumes from a buffered reader:

```rust
// src/cli/commands/ipc_client.rs:53
pub async fn recv(rx: &mut BufReader<OwnedReadHalf>) -> Result<Response> {
    let mut line = String::new();
    let n = rx.read_line(&mut line).await?;   // <-- consumes bytes from the BufReader into `line`
    …
}
```

`read_line` advances the `BufReader` as it appends bytes to the local `line`. When a daemon
`Response` line is split across socket reads, `read_line` consumes the partial bytes, then
returns `Pending` awaiting the rest. If the future is then **dropped** (a keypress wins the
select), those consumed bytes are gone with the dropped `line`. The next loop iteration calls
`recv` again, reads the *remainder* of that line as if it were a whole line, and
`serde_json::from_str` fails → the function returns `Err("Connection error: …")` and the turn
ends.

**Observed (by inspection) consequences:**
- **AC violated:** "a first ESC or Ctrl+C shows a warning in the live region and **streaming
  continues**." With this wiring, the first interrupt press (which should only *warn*) can
  instead **kill the stream** with a connection error whenever it races a partially-buffered
  `Response`. "Streaming continues" is not reliably true.
- "Ignore all other keys while streaming" is also violated: a *non-interrupt* keypress
  (`InterruptAction::Ignore → continue`) likewise drops the in-flight `recv` and can desync
  the JSON stream — the keypress is not actually a no-op.

The executor's "Notes for review" assert the opposite of the truth:
> "Used `tokio::select!` with `biased` flag to ensure keyboard input takes priority — this
> prevents the daemon branch from being silently dropped. The `biased` semantics guarantee
> … the daemon future retains its internal state for the next iteration."

`biased` provides no such guarantee. This is a hallucinated API fact on the exact seam the
phase Pre-flight flagged: *"confirm the real tokio primitive and that it does not drop the
un-selected branch's progress."* The build is green and all unit tests pass because the unit
tests only exercise the synchronous `InterruptState` helper and the `commit_panel` colors —
**none drives the `select!`/`recv` integration**, so the defect is invisible to `cargo test`
(the M2 "green-but-inert" pattern, here as "green-but-subtly-wrong").

## What should happen

A keypress (or a spinner tick) during streaming must **not** drop daemon-stream progress.
The daemon `recv` must survive across select iterations so no partially-read `Response` line
is ever discarded. Per the phase spec AC: a first interrupt press warns and **streaming
continues** intact; non-interrupt keys are true no-ops for the stream.

## How to fix

In `ask_with_session_ratatui` (`src/cli/commands/stream.rs`), **hold a single `recv` future
across loop iterations** instead of recreating it inside the `select!`:

- Create the daemon-read future **once before** the inner spinner/interrupt loop and pin it
  (`tokio::pin!`). In the `select!`, poll it by `&mut` reference. Only build a **new** `recv`
  future after the held one **actually completes** (returns a `Response`/EOF) — never on a
  keypress and never on a spinner tick.
- Drive the spinner animation from a **separate**, freely-recreatable timer (e.g.
  `tokio::time::sleep(80ms)` as its own select branch), so the 80 ms tick no longer cancels
  the in-flight read. The `read_key(stdin)` branch may still be recreated each iteration
  (a `read_byte` dropped while `Pending` has consumed nothing).
- Keep the two-press semantics unchanged (`InterruptState`): Warn → redraw + keep the held
  read alive; Abort → break and drop the socket as today.

Apply the same shape to **both** the phase-1 (spinner) and phase-2 (streaming) blocks.

Secondary (minor, fix while here): the abort path signals via a stringly-typed sentinel
`anyhow::anyhow!("interrupted")` matched with `e.to_string() == "interrupted"`
(`stream.rs` ~213/272). Prefer a typed signal — e.g. have the inner loop yield an explicit
`enum { Msg(Response), Interrupted }` (or a `ControlFlow`) rather than round-tripping through
an error string.

## Verification

- [ ] By inspection/grep: no `recv(&mut rx)` is constructed **inside** the `select!` arms;
      the daemon-read future is created once per message and held (pinned) across keypresses
      and spinner ticks in both phase-1 and phase-2 blocks.
- [ ] A hermetic regression test proves a keypress during streaming does **not** drop a
      daemon message: drive the streaming seam with a fake reader that delivers a `Response`
      **split** across two chunks (partial line, then the remainder) with an interrupt-key
      **and** a non-interrupt key injected in between, and assert the full `Response` is still
      received intact (no parse error, no lost message) and that a single non-abort keypress
      leaves the stream alive. (Reuse the phase-10 `pipe2` + `from_raw_fd` injection pattern in
      `src/cli/input/tty.rs` for the key side. If driving the whole `ask_with_session_ratatui`
      is infeasible, extract the "race key vs. held daemon-recv" step into a small testable
      unit and test that — but the inspection criterion above is mandatory either way.)
- [ ] `cargo build` (0 warnings), `cargo clippy --all-targets --all-features -- -D warnings`,
      `cargo fmt --all`, `cargo test` all still pass.
- [ ] The `InterruptState` unit tests and `commit_panel_uses_blood_red_border_and_yellow_title`
      still pass (the color work is correct — do not change it).
