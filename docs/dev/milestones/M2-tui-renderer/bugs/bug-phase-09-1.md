# Bug 1 on phase-09: Six `///` doc-comment lines dropped during the "verbatim" move

**Severity:** major
**Status:** open
**Filed:** 2026-06-26

## What's wrong

The split was required to move code **verbatim**, preserving "every comment,
doc-comment, and banner line exactly" (Pre-flight 5 & 7, Spec, Out of scope:
"no comment edits, no dropping of banner lines"). Three moved items lost their
leading `///` doc-comments — 6 lines total. This is the exact phase-07 failure
mode the phase doc explicitly pre-warned against.

A sorted-multiset diff of the old `src/config.rs` (normalized for the two
authorized edits) against the four new files shows these lines present in the
old file and absent from the new tree (`grep` confirms count 1→0 for each):

1. **`Config` struct** — `src/config/types.rs:3` (`pub struct Config {`). Both
   doc-comment lines dropped (old `config.rs:5-6`):
   ```
   /// Top-level configuration loaded from `~/.daemoneye/etc/config.toml`.
   /// All sections default to sensible values so the file is optional.
   ```

2. **`overwrite_knowledge_memories`** — `src/config/seeds.rs:101`. Both
   doc-comment lines dropped (old `config.rs:984-985`):
   ```
   /// Re-seed all built-in memory files (knowledge + session), overwriting existing ones.
   /// Called by `daemoneye setup --overwrite-memory`.
   ```

3. **`overwrite_sre_prompt`** — `src/config/seeds.rs:143`. Both doc-comment
   lines dropped (old `config.rs:1028-1029`):
   ```
   /// Overwrite the built-in SRE prompt regardless of whether it already exists.
   /// Called by `daemoneye setup --overwrite-all`.
   ```

In each case the item itself moved correctly; only the doc-comment directly
above it was lost.

## What should happen

Per the phase doc, the move is verbatim except the two authorized edits
(`../assets/` → `../../assets/` on the 14 `include_str!` paths, and the
`SRE_PROMPT_TOML` visibility bump). All `///` doc-comments must survive
character-identical, attached to their original items. This fails the
acceptance criterion "Line-fidelity check (sorted-multiset)": the concatenated
non-blank trimmed lines of the four new files, minus the authorized glue and
normalizing the two authorized edits, must **equal** the old file's. Currently
they do not — six doc-comment lines are missing.

(The banner/bar fidelity criteria pass: all 12 `// ----` banners and 8
`// ──` bars survived. The `include_str!` and visibility edits are correct.
This bug is solely the dropped `///` doc-comments.)

## How to fix

Re-attach the six doc-comment lines verbatim, immediately above their items:

- `src/config/types.rs` — above `#[derive(Debug, Deserialize, Serialize, Clone)]`
  / `pub struct Config {`, restore the two `/// Top-level configuration …` /
  `/// All sections default …` lines.
- `src/config/seeds.rs` — above `pub fn overwrite_knowledge_memories`, restore
  the two `/// Re-seed all …` / `/// Called by … --overwrite-memory` lines.
- `src/config/seeds.rs` — above `pub fn overwrite_sre_prompt`, restore the two
  `/// Overwrite the built-in SRE prompt …` / `/// Called by … --overwrite-all`
  lines.

Then re-run `rustfmt` on only the two touched files and re-verify the
multiset-fidelity criterion. Do not touch anything else.

## Verification

- [ ] `git show 27ebf12^:src/config.rs | grep -c '^/// '` count of dropped
      lines now matches the new tree: for each of the six strings,
      `grep -rhF "<line>" src/config/` returns 1.
- [ ] Sorted-multiset diff (old normalized vs. new concatenated, glue-stripped)
      shows only the intentional `impl Config {` / `}` split lines and the
      per-file `use` glue — no remaining `<` doc-comment deletions.
- [ ] `cargo build`, `cargo clippy --all-targets --all-features -- -D warnings`,
      `cargo fmt --all -- --check`, `cargo test` all still pass (773 unit + 27
      integration, 2 ignored).
