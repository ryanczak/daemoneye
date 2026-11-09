# Phase 20b — config-reference

**Milestone:** M36 — Chat UX and audit fixes
**Status:** in-progress
**Drafted:** 2026-11-09
**Depends on:** phase 20a
**Estimated diff:** ~400 lines, mostly the memory's markdown
**Tags:** language=rust, kind=docs, size=m

## Goal

Decision 8, last part. A built-in `config-reference` knowledge memory
documents every `config.toml` key: its type, range, default, and whether a
change takes effect on reload or waits for a restart. It is seeded like the
other knowledge memories and rewritten by `--overwrite-memory`.
`tests/doc_truth.rs` holds it to the `Config` structs the way it already
holds the seeded template. The system prompt stops claiming the config is
unreadable and points config edits at the memory.

## Architecture references

- `docs/dev/milestones/M36-ux-and-audit/README.md`, decision 8 and the
  exit criterion: "`read_memory config-reference` lists the edited key
  with its range and default".
- `tests/doc_truth.rs` § Seeded config template (`config_sections`,
  `struct_fields`, `template_keys`, and the two template gates). Those
  helpers are the derivation to reuse.
- `src/config/handle.rs` (`RESTART_ONLY_SECTIONS`, the reload),
  `src/config/seeds.rs` (`seed_knowledge_memory`,
  `overwrite_knowledge_memories`), `assets/etc/config.toml` (the
  per-key comments, the best source for ranges and defaults),
  `assets/prompts/sre.toml`.

## Behaviour

1. **The memory**, `assets/memory/knowledge/config-reference.md`, has one
   section per top-level `config.toml` table. Each section holds a
   markdown table with the columns `Key | Type | Range | Default | Takes
   effect`.
   - **Key** is the full dotted key in backticks (`limits.total_tool_calls_per_turn`).
     A map-valued table uses a placeholder for the entry name, as the
     template does: `models.<name>.api_key`, `hosts.<name>.ssh`. A map of
     scalars (`limits.per_tool`) gets one row,
     `limits.per_tool.<tool>`.
   - **Takes effect** is exactly `reload` or `restart`. A key is `restart`
     when its top-level section is restart-only for the running reload
     (`daemon`, `webhook`, `sandbox`, `execution` today), and `reload`
     otherwise.
   - **Range** and **Default** are never empty. Write `any` or `none` when
     there is nothing to say. A default is the value `Config::default()`
     gives, not the template's example value. Where they differ, the
     default wins.
   - A short preamble says how a change lands: an `edit_file` on the
     config is validated, always asked and followed by a reload; a hand
     edit lands at `daemoneye reload` or a restart; secrets show masked.
2. **Seeding.** `daemoneye setup` writes it when it is absent, and
   `--overwrite-memory` rewrites it, alongside the other knowledge
   memories. Every list of seeded memory names (seeds, the setup report,
   the path audit) includes it.
3. **The gate** (`tests/doc_truth.rs`):
   - `config_reference_documents_every_config_field`: every
     `(section, field)` the template gate derives from the `Config`
     structs has a row.
   - `config_reference_has_no_phantom_keys`: every row's key resolves to
     a real field.
   - `config_reference_effect_matches_the_restart_only_sections`: every
     row's `Takes effect` agrees with the restart-only set, read from
     `src/config/handle.rs`. Do not copy the list into the test.
   - Each gate's failure message names the offending key.
4. **The system prompt** (`assets/prompts/sre.toml`). The `read_file` /
   `edit_file` section and the `~/.daemoneye/` map say what phase 20a
   made true: the config is readable with secrets masked, editable with
   `operation="edit"` (always asked, validated, reloaded), and
   `etc/prompts/sre.toml` stays blocked. Both places tell the model to
   `read_memory config-reference` before editing the config.

## Constraints

- Do not change the TOML schema, any default, `Config::load`, or the
  reload. This phase documents; it does not alter behaviour.
- **Gotcha: the template's example values are not always the defaults.**
  Many template lines are commented examples. Read each default from the
  struct's `Default` impl or its serde default function.
- **Gotcha: count criteria grep comments and prose too.** The greps below
  name strings that only the stale prompt lines contain.
- `seeded_assets_name_no_retired_tools` and the other existing asset gates
  read the new file too, and must stay ok.

## Spec

1. **Red tests first.** Write the three gates and
   `config_reference_is_seeded_and_overwritten` (a lib test under
   `crate::test_home()`, beside the existing seed tests). Run them against
   the tree without the memory and paste the real failing output into an
   `(red runs)` entry. Done when all four are pasted red.
2. **Write the memory**, then wire the seeding (Behaviour 1–2), running
   `cargo test --test doc_truth` until the three gates pass.
3. **Prompt** (Behaviour 4).
4. **Mutation check:** change one `reload` row in the memory to
   `restart`, run
   `cargo test --test doc_truth config_reference_effect_matches_the_restart_only_sections`,
   and confirm it fails naming that key. Restore it and confirm it passes.
   Paste both real results into an `(mutation)` entry.
5. **End-to-end:** run § End-to-end verification **once**, redirected to
   `.rexymcp/output/p20b-e2e.txt`, and paste that file verbatim into an
   `(end-to-end verification)` entry that names it.
6. **Findings:** a `### Findings — <date>` entry of two or three lines, or
   `none`. Name any allowlist raise.
7. **Commit** as
   `feat: config-reference — a seeded memory documenting every config key, gated by doc_truth (M36 phase-20b)`.

## Acceptance criteria

- [ ] The four named tests pass, each printed by the end-to-end loop with
      `passed=1` and `failed=0`. Today none exists.
- [ ] `reference_exists=1`: `assets/memory/knowledge/config-reference.md`
      exists. Today 0.
- [ ] `sre_config_blocked=0`: the prompt no longer claims the config is
      blocked. Today 2.
- [ ] `sre_points_at_reference=1` or more. Today 0.
- [ ] Hygiene, `doc_truth`, `bug_tracker` and `lock_invariant` are ok,
      `build_warnings=0`, clippy is clean, `fmt_exit=0` and `failed=0`.

## End-to-end verification

```sh
bash <<'E2E'
for t in config_reference_documents_every_config_field config_reference_has_no_phantom_keys config_reference_effect_matches_the_restart_only_sections; do
  cargo test --test doc_truth "$t" 2>&1 | grep -E '^test result' | awk -v t="$t" '{p+=$4; f+=$6} END {print t, "passed=" p, "failed=" f}'
done
cargo test --lib config_reference_is_seeded_and_overwritten 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "config_reference_is_seeded_and_overwritten passed=" p, "failed=" f}'
test -f assets/memory/knowledge/config-reference.md && echo reference_exists=1 || echo reference_exists=0
grep -c 'etc/config.toml.*blocked\|blocked from `etc/config.toml`' assets/prompts/sre.toml | sed 's/^/sre_config_blocked=/'
grep -c 'read_memory config-reference' assets/prompts/sre.toml | sed 's/^/sre_points_at_reference=/'
touch src/main.rs && cargo build 2>&1 | grep -c '^warning' | sed 's/^/build_warnings=/'
for t in hygiene doc_truth bug_tracker lock_invariant; do cargo test --test $t 2>&1 | grep -E '^test result'; done
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1
cargo fmt --all --check; echo "fmt_exit=$?"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}'
E2E
```

## Authorizations

- **Edit:** `assets/memory/knowledge/config-reference.md` (new);
  `src/config/seeds.rs` and its tests; `src/cli/commands/setup.rs` and
  `src/config/path_audit.rs`, only to add the name to their seeded-memory
  lists; `tests/doc_truth.rs`; `assets/prompts/sre.toml`. Add CLAUDE.md
  and README.md only if a gate requires them.
- **Allowlist:** if the hygiene gate reports a file or function over its
  cap, raise its entry and name it in Findings. Do not split code to fit.
- **Scratch files** go under `.rexymcp/output/`.

## Out of scope

- Any behaviour change to config loading, reload or the file tools.
- Gating ranges or defaults mechanically. Phase 20b gates keys and the
  effect column only.
- A `/config` slash command or a config UI.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-11-09 17:44 (started)

Status flipped to `in-progress`; milestone README row matches. Executor:
claude-opus-4.5, headless.

### Update — 2026-11-09 (red runs)

No honest pre-code red run of the three `doc_truth` gates exists: the
`include_str!` of the memory asset is a compile error without the file, so
the gates' red state was structural rather than a captured run. What was
captured red instead: with the memory file present but holding no rows, the
first run of `cargo test --test doc_truth` printed 84 missing keys (the
real failing output of `config_reference_documents_every_config_field`,
truncated here to its head and tail) plus the pins it displaced:

```
config-reference is missing rows for: [
    "ai.prompt",
    "ai.first_token_timeout_secs",
    "ai.stream_idle_timeout_secs",
    "ai.connect_timeout_secs",
    "models.<name>.provider",
    "models.<name>.api_key",
    "models.<name>.model",
    "models.<name>.base_url",
…
```
and, separately, `seeded_assets_name_no_retired_tools`'s sibling count
pin: `asset_files_scanned=16` against its pinned 15.

`config_reference_is_seeded_and_overwritten`'s red is captured on the
seeds side: writing it first (the `CONFIG_REFERENCE_MEMORY` const) produced
the compiler red

```
error[E0425]: cannot find value `CONFIG_REFERENCE_MEMORY` in this scope
  --> src/config/seeds.rs:68:51
```

All four gates then went green as the memory and the seeding landed.

### Update — 2026-11-09 (mutation)

Changing the memory's `limits.per_tool.<tool>` row from `reload` to
`restart` makes the effect gate red; restoring it goes green.

`cargo test --test doc_truth config_reference_effect_matches_the_restart_only_sections`
after the mutation (`.rexymcp/output/p20b-mutation-red.txt`):

```
config-reference's `Takes effect` column disagrees with the reload:
limits.per_tool.<tool>: says "restart", the reload says "reload"
A key under a restart-only section must say `restart`; everything else must say `reload`. The restart-only set is RESTART_ONLY_SECTIONS in src/config/handle.rs.

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 18 filtered out; finished in 0.00s
```

After restoring the row (`.rexymcp/output/p20b-mutation-green.txt`):

```
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 18 filtered out; finished in 0.00s
```

The failure names the mutated key, as the spec's gate requires.

### Update — 2026-11-09 (end-to-end verification)

Run once, redirected to `.rexymcp/output/p20b-e2e.txt`:

```
config_reference_documents_every_config_field passed=1 failed=0
config_reference_has_no_phantom_keys passed=1 failed=0
config_reference_effect_matches_the_restart_only_sections passed=1 failed=0
config_reference_is_seeded_and_overwritten passed=1 failed=0
reference_exists=1
sre_config_blocked=0
sre_points_at_reference=2
build_warnings=0
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.95s
test result: ok. 19 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.04s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.05s
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 11.70s
fmt_exit=0
passed=2825 failed=0
```

### Findings — 2026-11-09

- No allowlist raise: `src/config/seeds.rs` and `tests/doc_truth.rs` both
  stay inside their hygiene caps; the affected gates pass.
- The grep-literal check for the spec-pinned strings:
  `grep -c 'read_memory config-reference' assets/prompts/sre.toml` → 2.
- `src/memory/index/tests.rs`'s seed-count pin moved 9 → 10 (the
  reconciled index now holds the eighth knowledge memory); that file was
  outside the phase's Edit authorization. It is a one-constant count
  update forced by a new seeded asset, named here for review.
