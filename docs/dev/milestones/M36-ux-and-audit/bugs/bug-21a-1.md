# Bug 1 on phase-21a: reports fed through the terminal reach the table wrong

**Severity:** major
**Status:** open
**Filed:** 2026-11-09

## What's wrong

The pure tests drive `Table` directly with hand-made records and stamps.
The production path is bytes into `ShellScreen::feed`, then
`ScreenCallbacks::unhandled_osc`, `parse` and `Table::apply`. Three things
go wrong on that path.

1. **`clear` never works.** `State::from_str` (`src/shell/program_status.rs`)
   knows only the five states. A `state=clear` pair is `Err(PairError::State)`,
   and `parse` drops the whole report. `Table::clear_one` has no caller
   outside its test. A program that sends `OSC 7501 ; state=clear ST` keeps
   all its records.
2. **An unknown key erases `kind`.** `parse_pair`'s catch-all arm returns
   `Ok(Pair::Kind(None))`, and `parse` assigns it to `kind`. So
   `state=blocked:kind=permission:future=1` yields a blocked record with no
   kind. The spec says unknown keys are ignored.
3. **The eviction stamp isn't a clock.** The callback stamps each report
   with `self.table.len() as u64`. Once the table is full, every report gets
   the same stamp, and after a `clear` the stamps go backwards. So "the least
   recently updated record goes" picks by `BTreeMap` order among ties, not by
   recency.

Two smaller departures from the digest's Syntax section are on the same
path:

- A key outside `[a-z]+` (`STATE=done`) is lowercased and accepted. It
  should be a malformed pair, skipped.
- An empty pair (`state=done::app=x`) drops the whole report. It should be
  skipped, and the rest applied.

## What should happen

Every rule in the phase doc's Behaviour 1–2 and the digest holds for
reports that arrive as terminal bytes:

- `clear` with an id removes that record and its subtree.
- `clear` without an id empties the table.
- An unknown key leaves the other pairs' values alone.
- At the cap, the record that was updated least recently goes, measured in
  report order.

## Root cause

The parser and the table were each tested in isolation, never joined
through `feed`. Three things follow from that: `clear` was modelled as a
`Table` method rather than as a state the parser hands over; the
unknown-key arm reused `Pair::Kind` as a no-op; and the stamp came from
whatever number was at hand.

## Definition of done

- [ ] `feed_clear_removes_the_subtree_and_clear_without_id_empties` passes.
      It feeds report bytes through `ShellScreen::feed` and reads the
      results back through `programs()`.
- [ ] `feed_unknown_key_keeps_the_kind` passes. `state=blocked:kind=permission:future=1`
      fed as bytes yields kind `permission`.
- [ ] `feed_evicts_the_least_recently_updated_at_the_cap` passes. It fills
      the table through `feed`, re-reports the first record, adds one more,
      and the first record survives while the second goes.
- [ ] `feed_skips_an_uppercase_key_and_an_empty_pair` passes.
- [ ] All four fail or don't exist on the current tree.
