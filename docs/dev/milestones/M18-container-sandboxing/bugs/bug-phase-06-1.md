# Bug 1 on phase-06: a refused command still creates a `de-bg-*` window, and the image-mismatch message double-prefixes `sha256:`

**Severity:** major
**Status:** open
**Filed:** 2026-08-28

## What's wrong

### 1. The gate refuses *after* the tmux window is created (major)

`src/daemon/background/run.rs`. The window is created at **line 62**:

```rust
    let pane_id =
        match tmux::off_runtime("create-job-window", move || tmux::create_job_window(&s, &t)).await
```

but the preflight gate does not run until **line 172**, and refuses by
returning at 176:

```rust
            if let Err(reason) =
                crate::daemon::executor::container::sandbox_preflight(&config.sandbox)
            {
                let message = crate::daemon::executor::container::describe_unavailable(&reason);
                log::warn!("refusing sandboxed background command: {message}");
                return message;
            }
```

Measured at review:

```
sandbox_preflight at line 172; create_job_window at line 62
WINDOW_FIRST
```

So every refused sandboxed command **leaks a `de-bg-*` tmux window**. The
early `return` skips the rest of the function, so nothing kills it and nothing
un-registers the pane from the `cmd_id` map populated just after creation. The
user sees windows accumulate, one per refusal, each holding an idle shell —
and refusals are expected in normal operation (a stopped docker service, an
image drifted from the lock).

The phase doc's Task 4 required the opposite, in the same paragraph that
placed the call: *"The refusal must happen **before** any tmux window is
created, so a refused command leaves no `de-bg-*` window behind."*

### 2. `sha256:` is printed twice in the mismatch message (minor)

`src/daemon/executor/container.rs:458`:

```rust
        SandboxUnavailable::Image(ImageCheck::Mismatch { live, .. }) => format!(
            "sandbox unavailable: the live image (sha256:{live}) differs from the lock — …"
        ),
```

`live` already carries the prefix — `check_image_matches` stores
`live_image_id.to_string()` (`container.rs:257`), and `probe_live_image_id`
returns the trimmed `{{.Id}}`, which is `sha256:0d02beb…`. The rendered
message is therefore:

```
sandbox unavailable: the live image (sha256:sha256:0d02beb…) differs from the lock
```

The unit test does not catch it because its fixture uses a bare
`"b".repeat(64)` with no prefix.

## What should happen

1. **Gate first, window second.** Load the config and run
   `sandbox_preflight` *before* `create_job_window`, so a refusal returns
   without having created anything. The refusal message and the `log::warn!`
   stay as they are; only the position changes. Note that `pane_num` and
   `unix_ts` (used for the `ExecSpec.job_id`) are derived from the pane id and
   so are only available *after* creation — the gate does not need them, so
   move only the gate, not the `sandbox_window_command` call.
2. **Print the id once.** Either drop the literal `sha256:` from the format
   string or store the bare hex; the message must contain exactly one
   `sha256:` for a live id that carries the prefix.

## Root cause

**Defect 1 is an architect-side spec contradiction.** Task 4 gave two
placements that cannot both hold: *"inside the existing `if
config.sandbox.enabled { … }` block that phase-05 added"* — which sits at line
166, long after window creation — and *"before any tmux window is created"*.
The executor satisfied the first, which was the concrete instruction, and did
not flag the conflict. A spec that names a location and then names a different
constraint on that location will get whichever the executor reads as more
operational.

**Defect 2 is a straightforward interpolation slip**, invisible because the
test fixture omitted the prefix that production values always carry — the same
shape as the phase-03 fixture defect: a test whose data is unlike the real
data cannot see a formatting bug.

## Definition of done

Each command was run against the current tree at filing and produced the
"before" value shown.

- [ ] The gate precedes window creation. Run:

      ```sh
      P=$(grep -n "sandbox_preflight" src/daemon/background/run.rs | head -1 | cut -d: -f1)
      W=$(grep -n "create_job_window" src/daemon/background/run.rs | head -1 | cut -d: -f1)
      [ "$P" -lt "$W" ] && echo GATE_FIRST || echo WINDOW_FIRST
      ```

      must print `GATE_FIRST` (**before: `WINDOW_FIRST`**, gate at 172,
      window at 62).
- [ ] `grep -c 'sha256:{live}' src/daemon/executor/container.rs` prints `0`
      (**before: 1**).
- [ ] A test pins the single-prefix rendering: with a `Mismatch` whose `live`
      is a **realistic** id (`format!("sha256:{}", "b".repeat(64))`, i.e.
      carrying the prefix as production values do), the message contains
      exactly one occurrence of `sha256:`. Assert the count, not merely that
      the id appears.
- [ ] `cargo test --lib sandbox_gate 2>&1 | grep -c "^test .* ok$"` prints
      `8` (**before: 7**) — the one new test above, no others.
- [ ] `cargo test --lib 2>&1 | grep -E "^test result:"` reports
      `1440 passed; 0 failed; 3 ignored` (**before: 1439/3**).
- [ ] `grep -rc "allow(dead_code)" src/ | awk -F: '{s+=$2} END {print s}'`
      still prints `7`.
- [ ] All four gates green: `cargo fmt --all`, `cargo build`,
      `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`.
