# Phase 04 — Split `config/types.rs` by section

**Status:** in-progress (refined re-dispatch after a stall, 2026-10-26)
**Milestone:** M31 — Code hygiene
**Drafted:** 2026-10-26

## Goal

`src/config/types.rs` (1466 production lines, twenty config sections) is
split into four files of at most 800 production lines each, by moving whole
sections into three new sibling modules. Pure move: no type, field, default
or doc comment changes, and the public surface of `crate::config` is
identical.

## Architecture references

- `docs/dev/milestones/M31-code-hygiene/README.md` — decisions 5 (moves
  are shell range cuts), 6 (no behaviour change), 7 (the Findings entry).
- `src/config/mod.rs` — declares `mod types;` and re-exports it with
  `pub use types::*;`. Every caller reaches these types as
  `crate::config::Name`, never through `types::`.

## Pre-flight

- The four gates are green; `cargo test --test hygiene` passes, and
  `tests/hygiene_allowlist.txt` has the line `file src/config/types.rs 1466`.

## Spec

1. **Three new modules in `src/config/`**, each declared in `mod.rs` beside
   `mod types;` (private, like it) and re-exported with `pub use <name>::*;`:

   | New file | Moves, in source order, as one contiguous block each |
   |---|---|
   | `sandbox.rs` | from the doc comment of `SandboxLimits` through the end of `impl ExecutionConfig` |
   | `shells.rs` | from the doc comment of `ShellsConfig` through the end of `impl LimitsConfig` |
   | `models.rs` | from the doc comment of `HostEntry` through the end of `impl PromptDef` |

   Each block includes the `default_*` functions and `impl Default` blocks
   that sit inside it. Everything else, `Config` and the remaining
   sections, the test module `budget_tests` included, stays in `types.rs`.
2. **Move with the shell** (README decision 5): find each block's first and
   last line with `grep -n`, copy it with `sed -n 'a,bp'` into the new file
   under a two-line header (a `//!` line naming the sections, then the
   imports the block needs), and delete the range from `types.rs` with
   `sed -i 'a,bd'`. Cut from the bottom block up, so earlier line numbers
   stay valid. Never retype a block.
3. **Imports.** Each new file needs `use serde::{Deserialize, Serialize};`
   and whatever else its block names (`crate::host::HostId`, types from the
   other config files via `use super::*;` or `use super::Name;`). Let the
   compiler name what is missing.
4. **Shared helpers.** A private `default_*` function used by blocks in
   more than one file (for example `default_true`, which serde attributes in
   both the sandbox and webhook sections name) stays in `types.rs` as
   `pub(super)`. Note that `#[serde(default = "…")]` takes a path resolved
   from the struct's own module: a moved struct names such a helper as
   `"super::types::default_true"` or imports it.
5. **Allowlist.** Delete `file src/config/types.rs 1466` from
   `tests/hygiene_allowlist.txt` (the gate fails with "remove it" until you
   do). No new entry may be added.
6. **CLAUDE.md**, the `src/config/` row of "Key files": name the three new
   files and what each holds.
7. **Findings** (README decision 7): you read every config section as it
   moves, so record anything seen: a field whose default disagrees with its
   doc comment, a default duplicated between `impl Default` and its
   `default_*` function, a section no code reads, a validation that is
   missing. `none` is a valid entry.

**The recipe — run it as written (verified 2026-10-26 on a scratch
worktree of `b52feec3`: build, clippy and fmt clean, 2586 passed).** It
computes every boundary from an anchor, so there is nothing to look up
first. Do **not** read `types.rs` section by section to find boundaries:
the first dispatch spent 200 turns doing that and cut nothing.

```bash
F=src/config/types.rs
# First line of an item's doc comment / attributes, found from its `pub struct` line.
docstart() { awk -v pat="$1" '{l[NR]=$0} $0 ~ pat {s=NR-1; while (s>0 && (l[s] ~ /^\/\/\// || l[s] ~ /^#\[/)) s--; print s+1; exit}' $F; }
S1=$(docstart '^pub struct SandboxLimits'); S2=$(docstart '^pub struct ShellsConfig')
S3=$(docstart '^pub struct NotificationsConfig'); S4=$(docstart '^pub struct HostEntry')
T=$(grep -n '^#\[cfg(test)\]' $F | tail -1 | cut -d: -f1)
echo "S1=$S1 S2=$S2 S3=$S3 S4=$S4 T=$T"   # at b52feec3: 476 786 985 1116 1467
# Blocks: sandbox S1..S2-2, shells S2..S3-2, models S4..T-2 (each ends on `}` before a blank line).
hdr() { printf '//! %s\n\nuse super::*;\nuse serde::{Deserialize, Serialize};\n\n' "$1"; }
{ hdr "Config sections: [hosts.*], [models.*] and pricing, [ai], prompts."; sed -n "${S4},$((T-2))p" $F; } > src/config/models.rs
{ hdr "Config sections: [shells] and [limits]."; sed -n "${S2},$((S3-2))p" $F; } > src/config/shells.rs
{ hdr "Config sections: [sandbox] and [execution]."; sed -n "${S1},$((S2-2))p" $F; } > src/config/sandbox.rs
# Delete bottom-up so earlier numbers stay valid.
sed -i "${S4},$((T-2))d" $F; sed -i "${S2},$((S3-2))d" $F; sed -i "${S1},$((S2-2))d" $F
```

Then these edits, each one line, which the compiler asks for in this
order:

1. `src/config/mod.rs`: declare `mod models; mod sandbox; mod shells;`
   beside `mod types;` and add `pub use models::*; pub use sandbox::*;
   pub use shells::*;` beside `pub use types::*;`.
2. `src/config/load.rs`: `use super::types::*;` becomes `use super::*;`
   (it names `ModelEntry` and `PromptDef`, which now live in `models`).
3. `src/config/types.rs`: add `use super::*;` as its first line (it names
   `AiConfig`, `SandboxConfig` and the rest), and make `fn default_true`
   `pub(super) fn default_true` (sandbox's serde attributes name it).
4. `src/config/models.rs`: make `fn default_models` `pub(super)` (`Config`
   names it), and delete its `use super::*;` line, and the same line from
   `shells.rs` (both unused; clippy denies the warning).
5. `tests/doc_truth.rs` reads `src/config/types.rs` by path to collect
   config fields (the `CONFIG_TYPES` constant and two
   `read_to_string(CONFIG_TYPES)` calls). Make `CONFIG_TYPES` the list of
   the four files and read them joined into one string, so
   `seeded_config_template_documents_every_config_field` and
   `seeded_config_template_has_no_phantom_keys` see every section again.
6. `cargo fmt --all`, delete the allowlist line, and the gates.

**Gotchas.**
- `pub use sandbox::*;` and `pub use types::*;` re-export each other's
  names through `config`; a `use super::*;` in a child is fine and is not
  a cycle error.
- Do not reorder fields or items inside a block: serde and the seeded
  config template (`tests/doc_truth.rs` checks that every config field is
  documented) depend on names, not order, but a reordered block is not a
  move.

## Acceptance criteria

- [ ] `src/config/sandbox.rs`, `shells.rs` and `models.rs` exist, and each
      of the four files is at most 800 lines (`wc -l`).
- [ ] `grep -c '^file src/config/types.rs ' tests/hygiene_allowlist.txt`
      is 0, and `cargo test --test hygiene` passes.
- [ ] The block contents are unchanged: concatenating the four files with
      whitespace removed and sorting their lines gives the same multiset as
      the pre-phase `types.rs`, except the two helpers that gain
      `pub(super)` and the new `use` and `//!` lines (the End-to-end block
      checks this).
- [ ] Preservation: `cargo test` passes with 2586 passing tests;
      `cargo test --test doc_truth` passes; `src/config/tests.rs` is
      unchanged.
- [ ] The Update Log has a `### Findings — <date>` entry.

## Test plan

No new tests. The config tests in `src/config/tests.rs` and
`types.rs`'s `budget_tests`, `tests/doc_truth.rs` (seeded template vs
config fields) and the full suite are the proof.

## End-to-end verification

Run with `bash`. Paste the output byte-exact into the Update Log. Set `B`
to the pre-phase commit.

```sh
B=<pre-phase commit>; A=/tmp/e2e-m31-04.txt; : > "$A"
wc -l src/config/types.rs src/config/sandbox.rs src/config/shells.rs src/config/models.rs | sed 's/^ *//' >> "$A"
echo "allowlisted=$(grep -c '^file src/config/types.rs ' tests/hygiene_allowlist.txt)" >> "$A"
norm() { tr -d ' \t' | grep -v '^$' | sort; }
git show $B:src/config/types.rs | norm > /tmp/m31-04-old.txt
cat src/config/types.rs src/config/sandbox.rs src/config/shells.rs src/config/models.rs | norm > /tmp/m31-04-new.txt
echo "lost_lines=$(comm -23 /tmp/m31-04-old.txt /tmp/m31-04-new.txt | wc -l)" >> "$A"
echo "added_other=$(comm -13 /tmp/m31-04-old.txt /tmp/m31-04-new.txt | grep -vE '^(use|pub\(super\)|//!|#\[serde)' | wc -l)" >> "$A"
echo "tests_rs_changed=$(git diff --stat $B -- src/config/tests.rs | wc -l)" >> "$A"
cargo test --test hygiene 2>&1 | grep -E '^test result' | sed 's/finished in [0-9.]*s/finished in <t>s/' >> "$A"
cargo test --test doc_truth 2>&1 | grep -E '^test result' | sed 's/finished in [0-9.]*s/finished in <t>s/' >> "$A"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1 | sed 's/in [0-9.]*s/in <t>s/' >> "$A"
cargo fmt --all --check; echo "fmt_exit=$?" >> "$A"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}' >> "$A"
cat "$A"
```

Expected: four line counts each at most 800, `allowlisted=0`,
`lost_lines=2` (the old `fn default_true` and `fn default_models` lines,
which now begin `pub(super)`), `added_other=0` (any `#[serde(default = …)]` line whose
helper path changed shows as `#[serde` and is allowed; list each such line
in the Update Log), `tests_rs_changed=0`, two `ok.` lines, clippy clean,
`fmt_exit=0`, `passed=2586 failed=0`.

## Authorizations

- Edit `src/config/types.rs`, `src/config/mod.rs` and the one import in
  `src/config/load.rs`; create the three new files. Edit
  `tests/doc_truth.rs` (`CONFIG_TYPES` and its two readers only),
  `tests/hygiene_allowlist.txt` (the one deletion) and `CLAUDE.md` (the
  `src/config/` row).

## Out of scope

- Any change to a field, default, doc comment or validation, even one a
  finding names.
- `config/load.rs`, `seeds.rs` and the other config files.

## Update Log

<!-- entries appended below this line -->

### Notes for executor — 2026-10-26

The first dispatch stalled after 203 turns (`NoProgressStall`, 120
consecutive read-only turns) without writing a file: it read `types.rs` 10
to 20 lines at a time with `awk`, mapping every section boundary by hand.
The Spec now carries a recipe that computes the boundaries from anchors
and cuts all three blocks, plus the six follow-up edits it needs, every one
verified on a scratch worktree of `b52feec3`: 2586 passed, clippy and fmt
clean. Run the recipe first, then the edits in order, then the gates. The
Findings entry is still yours: read each new file once it exists.

### Update — 2026-10-26 (escalation)

**Chosen lever:** refined re-dispatch.
**Rationale:** the Spec named the blocks by symbol but left finding their
lines to the executor, which explored instead of cutting; phases 02 and 03,
whose specs computed every cut line with one command, went through first
try. The refinement gives the computed recipe and the edits the dry run
found (`load.rs`'s import, `default_models`, `doc_truth.rs` reading
`types.rs` by path), none of which the original Spec named.
