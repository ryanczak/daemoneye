# Bug 1 on phase-09: `dead_code` acceptance grep still returns 2 hits — diagnostics suppressed, not eliminated

**Severity:** major
**Status:** open
**Filed:** 2026-06-23

## What's wrong

Acceptance criterion #2 requires:

```
grep -rn '#\[allow(dead_code' $(find src -name '*.rs' | grep -v '_tests.rs')
```

to return **zero hits**. It returns **two**:

```
src/search.rs:20:#[allow(dead_code)]
src/header.rs:294:#[allow(dead_code)] // Used in #[cfg(test)] module; dead_code warning emitted during non-test builds
```

Both suppress a real `dead_code` diagnostic rather than eliminating it, which is
exactly what this phase exists to remove (STANDARDS §1: "No `#[allow(...)]` …
shims to mask diagnostics"). The criterion is verifiably unmet.

**`src/header.rs:294` — `parse_yaml_frontmatter`.** Spec 2a directed the executor
to *delete* this allow, calling it "stale." The classification was wrong: the
function (defined at line 295, **outside** the `#[cfg(test)] mod tests` that
starts at line 365) has **no production callers** — its only callers are the four
tests at `header.rs:542,552,560,568`. Deleting the bare allow therefore produces
a genuine `dead_code` warning in non-test builds, which `clippy -D warnings`
rejects. The executor reacted by *re-adding* the allow with a justification
comment. That keeps the build green but leaves the suppression in place — a
diagnostic that can be fully eliminated instead of masked. Per STANDARDS §7, an
acceptance criterion that is impossible *as the spec prescribes the fix* is a
blocker to raise, not something to improvise a suppression around.

**`src/search.rs:20` — `search_repository`.** Pre-existing (commit `3014908`,
a prior G2 phase) and absent from this phase's Spec-2 inventory of 8 sites — the
executor noted this in "Notes for review." But the acceptance grep is global and
does not carve it out, so the criterion fails regardless of origin. The 3-arg
`search::search_repository` wrapper is a **dead production symbol**: production
dispatch calls `knowledge::search_repository` (`src/daemon/executor/mod.rs:466`),
while `search::search_repository` is invoked only by its own four tests
(`search.rs:342,364,388,395`). STANDARDS §2.2: "If a symbol is unused, delete it."

## What should happen

The acceptance grep returns zero hits, with the underlying `dead_code` diagnostics
**eliminated** (not suppressed), and `cargo build` / `clippy -D warnings` / `cargo
test` all still green.

## How to fix

1. **`src/header.rs`** — remove the `#[allow(dead_code)]` at line 294 and instead
   gate the function for test builds: put `#[cfg(test)]` immediately above
   `pub fn parse_yaml_frontmatter` (line 295). All callers are already inside
   `#[cfg(test)]`, so the function compiles only when its callers do — no
   `dead_code` warning in non-test builds, no suppression attribute.

2. **`src/search.rs`** — remove the `#[allow(dead_code)]` at line 20. Because the
   3-arg `search_repository` wrapper is unused in production, either delete it
   (callers `search.rs:342,364,388,395` are tests that can call
   `search_repository_with_namespaces(query, kind, ctx, &["global"])` directly)
   or gate it with `#[cfg(test)]`. Deletion is preferred per STANDARDS §2.2.
   If you believe this site is genuinely out of phase-09 scope, that is a
   spec/acceptance-criterion conflict — raise it as a blocker for the architect
   to amend criterion #2; do **not** leave the suppression in place under the
   current criterion.

Do not reintroduce `#[allow(dead_code)]` at either site.

## Verification

- [ ] `grep -rn '#\[allow(dead_code' $(find src -name '*.rs' | grep -v '_tests.rs')` returns zero hits
- [ ] `cargo build` succeeds with zero warnings
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` passes
- [ ] `cargo test` passes
