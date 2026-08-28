# Bug 1 on phase-03: the "rejects bad records" test does not cover two of the cases it names, and the container module is more public than it needs to be

**Severity:** major
**Status:** open
**Filed:** 2026-08-28

## What's wrong

### 1. Two required-key rejection paths are unguarded (major)

`src/daemon/executor/container.rs:491`:

```rust
let missing_key = "image_id = {id}\nbuilt_at = 1787900000\n";
```

This is a plain string literal, not a `format!` — so the fixture contains the
seven characters `{id}` rather than an interpolated image id. `parse_lock`
therefore rejects it at the **`is_valid_image_id` check**, never reaching the
missing-`image`-key path the fixture is named for. The test passes, and it
passes for the wrong reason.

Measured by mutation at review — each mutation below leaves **all 9 tests
green**, so nothing guards these paths:

| Mutation to `parse_lock` | `cargo test --lib sandbox_lock` |
|---|---|
| `image: image?` → `image: image.unwrap_or_default()` | `ok. 9 passed; 0 failed` |
| `built_at: built_at?` → `built_at: built_at.unwrap_or(0)` | `ok. 9 passed; 0 failed` |

For contrast, the unknown-key path **is** genuinely guarded — neutering it
(`else { return None }` → ignore) fails 1 of 9. So the defect is specific to
the two missing-required-key cases, not to the test as a whole.

§ Test plan named "a missing key" as a required rejection case. It is
currently unverified.

### 2. `container` is `pub` where `pub(crate)` suffices (minor)

`src/daemon/executor/mod.rs:4`:

```rust
pub mod container;
```

`src/lib.rs:10` is `pub mod daemon;` and `src/daemon/mod.rs:33` is
`pub mod executor;`, so this puts the whole module — the lock helpers, the
uid gate, the probe — into the **crate's public API**. The CLI is inside the
crate, so it does not need that.

Measured at review: `pub(crate) mod container;` compiles cleanly
(`cargo build` exit 0) and is sufficient for
`src/cli/commands/sandbox.rs` to reach the helpers.

## What should happen

1. The `missing_key` fixture must interpolate a **valid** image id, so the
   record is rejected for the reason the case is named after. Cover the
   missing-`built_at` case too — the spec's "a missing key" covers both
   required keys, and both are currently unguarded. Keep the test count at 9
   (fix the fixtures inside the existing test; do not add tests).
2. Narrow the module to `pub(crate) mod container;`. Keep the existing
   `#[allow(dead_code)]` and its comment exactly as they are.

## Root cause

**1 is executor-side, with an architect-side contribution.** The fixture is a
copy-paste of the neighbouring `format!` lines with the `format!` dropped —
the sort of slip a compiler cannot catch, because `"…{id}…"` is a perfectly
valid string. What lets it survive is that the test asserts only *that*
`parse_lock` returns `None`, never *why*. A rejection test that does not
distinguish its rejection reasons will pass for any reason at all. My § Test
plan asked for five cases in one test and did not ask for the reasons to be
told apart, which is what made a silent miss possible.

**2 is architect-side.** The spec told the executor to put the helpers in
`container.rs` and call them from `src/cli/commands/sandbox.rs`, but never
said what visibility that required. `pub mod` is the first thing that works;
`pub(crate)` is the correct one. The executor disclosed the change and its
reasoning in both the Update Log and the completion summary, and it was right
that *some* widening was needed — only the degree is wrong.

## Definition of done

Each command was run against the current tree at filing and produced the
"before" value shown.

- [ ] `grep -c 'let missing_key = "image_id = {id}' src/daemon/executor/container.rs`
      prints `0` (**before: 1**) — the fixture interpolates a real id.
- [ ] `grep -c "^pub mod container;" src/daemon/executor/mod.rs` prints `0`
      (**before: 1**).
- [ ] `grep -c "^pub(crate) mod container;" src/daemon/executor/mod.rs` prints
      `1` (**before: 0**).
- [ ] `cargo test --lib sandbox_lock 2>&1 | grep -c "^test .* ok$"` still
      prints `9` — fix the fixtures, do not add tests.
- [ ] **Mutation evidence, both halves, pasted into the Update Log.** Apply
      each mutation, run `cargo test --lib sandbox_lock`, record the result,
      and restore. Both must now **FAIL**; both pass today.
      - `image: image?` → `image: image.unwrap_or_default()`
      - `built_at: built_at?` → `built_at: built_at.unwrap_or(0)`
      Restore the file afterwards and confirm `git status --short` is empty.
- [ ] `cargo test --lib 2>&1 | grep -E "^test result:"` still reports
      `1414 passed; 0 failed; 1 ignored`.
- [ ] `grep -rc "allow(dead_code)" src/ | awk -F: '{s+=$2} END {print s}'`
      still prints `7`.
- [ ] All four gates green: `cargo fmt --all`, `cargo build`,
      `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`.
