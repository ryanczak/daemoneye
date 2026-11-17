# Bug 1 on phase-07a: a command route runs for every notice, whatever its filter

**Severity:** major
**Status:** fixed (2e25075e)
**Filed:** 2026-11-17

## What's wrong

`command_targets` (`src/daemon/notice_sinks.rs`) pushes every configured
`command` route's command without asking whether the route's filter
matches the notice:

```
for route in &cfg.routes {
    if route.sink == crate::config::RouteSink::Command
        && let Some(cmd) = &route.command
    {
        push(cmd.clone(), &mut out);
    }
}
```

The command sink subscribes with `Filter::default()`, so it receives every
notice. A route with `sources = ["watchdog"]` therefore runs its command on
every webhook, scheduler, ghost, background and shell notice too.

The test that should catch this pins it instead.
`command_targets_order_and_dedup` sends a `Source::Scheduler` notice, and
its third route has `sources = ["watchdog"]`, yet the test expects
`echo route-b` in the result.

## What should happen

Spec task 3: "then each **matching** configured `command` route's command,
in config order". A route whose `filter()` does not match `n` contributes
nothing.

## Root cause

The loop checks the sink and the command but never calls
`route.filter().matches(n)`. The test's expected vector was written from
the function's output rather than from the spec.

## Definition of done

- [ ] `command_targets_order_and_dedup` expects exactly
      `["echo alert", "echo route-a"]` for its `Scheduler` notice, with
      `echo route-b` (filtered to `watchdog`) **absent**.
- [ ] A new test `command_targets_skip_a_route_whose_filter_does_not_match`
      exists and passes. It covers a command route with `sources =
      ["watchdog"]`: a `Watchdog` notice yields its command and a
      `Scheduler` notice yields nothing. It fails against the current tree.
- [ ] `cargo test` reports **3007** lib tests passed (3006 + that one).
