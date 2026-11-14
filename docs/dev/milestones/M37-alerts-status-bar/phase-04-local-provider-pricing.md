# Phase 04 — local-provider-pricing

**Milestone:** M37 — Alerts out of history, onto the status bar and `top`
**Status:** in-progress
**Drafted:** 2026-11-14
**Depends on:** nothing in M37
**Estimated diff:** ~80 lines, most of it tests and docs
**Tags:** language=rust, kind=bugfix, size=s

## Goal

Fix GitHub issue #6. A `[models.<name>]` entry with `provider = "ollama"`
or `"lmstudio"` that sets any `*_cost_per_mtok` rate is priced at those
rates, with `PricingSource::UserConfig`. Today `ModelEntry::pricing()`
returns `Pricing::zero()` for local providers before it looks at the user's
rates, so every `ai_cost` event, `daemoneye costs`, the status bar's cost
slot and the daily spend cap see `$0` for a self-hosted model the user
priced. A local entry with **no** rate set stays free (`PricingSource::Local`,
all zero), as today.

## Architecture references

- `src/config/models.rs`: `ModelEntry::pricing`, `PricingSource`,
  `Pricing::zero`.
- `src/cost.rs`: `emit_cost` (the single cost site; unchanged),
  `compute_cost`.
- `src/config/load.rs`: `validate_pricing` (skips local providers; unchanged).

## Spec

Numbered tasks in execution order. The executor's task list is seeded from
this section: mark each one `active` when you start it and `done` when its
`cargo build` is green.

1. **Precedence** — in `ModelEntry::pricing()`, user rates win over the
   local-provider zero. Any one of the four rate fields being `Some` counts
   as "user rates set" (unset fields price at 0.0, as for remote providers
   today). Only when none is set does a local provider return
   `Pricing::zero()`. A remote provider with no rates still returns `None`.
   Update the doc comments on `pricing()` and on `PricingSource::Local` so
   neither claims local providers are always zero.
2. **Tests** — in `src/config/tests.rs`, keep
   `local_provider_pricing_is_zero` passing unchanged (it is the
   no-rates case) and add the tests in § Test plan.
3. **Docs** — `assets/etc/config.toml`: the local section's heading drops
   "always $0" and says local models are free unless rates are set (a
   one-line commented example rate is fine). `README.md` § the "Local
   models always $0.00" bullet: the same change. (`src/config/seeds.rs`
   seeds the template with `include_str!`, so it needs no edit.)
4. **Capture the evidence** — run the mutation check and the
   § End-to-end block verbatim, and paste both into one Update Log entry.

## Constraints

- `emit_cost`, `compute_cost`, `validate_pricing` and the `PricingSource`
  variants and their serde names do not change. Records already on disk
  must still deserialize.
- Tests construct `ModelEntry` values directly; they read no config file
  and write no event log.

## Test plan

- `local_provider_with_user_rates_uses_them`: for both `ollama` and
  `lmstudio`, an entry with all four rates set resolves to those rates with
  `PricingSource::UserConfig`.
- `local_provider_partial_rates_price_unset_buckets_at_zero`: an `ollama`
  entry with only `output_cost_per_mtok` set resolves to that output rate,
  0.0 for the other three, `UserConfig`.
- `local_provider_user_rates_produce_nonzero_cost`: an `ollama` entry with
  input 10.0 / output 50.0 per Mtok, fed through `compute_cost` with a
  `TokenBreakdown` of 1 000 000 input and 100 000 output tokens, gives a
  total of 15.0 (within 1e-9). Put it wherever `compute_cost` is reachable
  from a test — `src/cost.rs`'s test module is fine.

## Acceptance criteria

- [ ] Every test in the test plan passes, and `local_provider_pricing_is_zero`
      still passes.
- [ ] `grep -c 'always \$0' assets/etc/config.toml` prints 0 (today 1).
- [ ] Mutation check, pasted with its `test result:` line: move the
      local-provider early return back above the user-rate check, confirm
      `local_provider_with_user_rates_uses_them` fails, then revert.
- [ ] fmt, build (0 warnings), clippy and `cargo test` are green, with
      `failed=0` and more than 2956 passed. Hygiene,
      `doc_truth` and `bug_tracker` are ok.

## Pre-flight

Baseline at drafting (2026-11-14, `1e9c920c`): `cargo test` passed=2956, failed=0.

## End-to-end verification

Run mechanically, with output redirected to a file and pasted:

```sh
cargo test --lib local_provider 2>&1 | grep -E '^test result'
grep -c 'always \$0' assets/etc/config.toml | sed 's/^/always_zero=/'
cargo build 2>&1 | grep -c '^warning' | sed 's/^/build_warnings=/'
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
```

## Authorizations

- Edit `src/config/models.rs`, `src/config/tests.rs`, `src/cost.rs` (tests
  only), `assets/etc/config.toml` and `README.md`.

## Out of scope

- Rewriting historical `ai_cost` records (they stay `$0`; forward-only).
- Any change to how `daemoneye costs`, the status bar or the spend ledger
  consume a `CostRecord`.
- Built-in price tables for any provider.

## Update Log

<!-- entries below this line -->

### Update — 2026-11-14 19:05 (started)

**Executor:** executor (headless)
**Status:** in-progress

Fixing GitHub issue #6: `ModelEntry::pricing()` gives user `*_cost_per_mtok`
rates precedence over the local-provider zero for `ollama`/`lmstudio`.

### Update — 2026-11-14 11:24 (mutation check)

Moved the local-provider early return back above the user-rate check and
re-ran the pinned test:

```text
$ cargo test --lib local_provider_with_user_rates_uses_them 2>&1 | grep -E '^(test result|test config::tests::local_provider_with_user_rates|---- )'
test config::tests::local_provider_with_user_rates_uses_them ... FAILED
---- config::tests::local_provider_with_user_rates_uses_them stdout ----
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 2868 filtered out; finished in 0.00s
```

Reverted the mutation; `pricing()` is back to user-rates-first. The test
drives the production `ModelEntry::pricing()` the ghost's cost site calls
(break the precedence and it fails), so the mutation check proves it pins
the behaviour.

### Update — 2026-11-14 11:24 (end-to-end verification)

Ran the phase's end-to-end block mechanically, output redirected to
`/tmp/p04_e2e.txt` and pasted verbatim:

```text
$ cargo test --lib local_provider 2>&1 | grep -E '^test result'
test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 2864 filtered out; finished in 0.00s
$ grep -c 'always \$0' assets/etc/config.toml | sed 's/^/always_zero=/'
always_zero=0
$ cargo build 2>&1 | grep -c '^warning' | sed 's/^/build_warnings=/'
build_warnings=0
$ cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
passed=2959 failed=0
```

Hygiene `--test hygiene` ok (16 passed), `doc_truth` ok (19 passed),
`bug_tracker` ok (6 passed). Baseline was passed=2956; now 2959
(three new tests, `local_provider_pricing_is_zero` untouched).

