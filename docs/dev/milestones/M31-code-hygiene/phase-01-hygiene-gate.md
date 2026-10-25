# Phase 01 — The hygiene gate

**Status:** in-progress (refined re-dispatch after a stall, 2026-10-25)
**Milestone:** M31 — Code hygiene
**Drafted:** 2026-10-25

## Goal

`cargo test` fails when a source file or a function grows past the M31 caps,
except for today's offenders, which are listed in an allowlist that may only
shrink. Every later M31 phase removes entries from it.

## Architecture references

- `docs/dev/milestones/M31-code-hygiene/README.md` — PE decisions 1–3 (the
  caps, the exemptions, the allowlist) and 7 (the findings entry).
- `tests/doc_truth.rs` — an existing gate over the tree, for shape.

## Pre-flight

- The four gates are green at HEAD.
- `syn 2.0.119` and `proc-macro2 1.0.107` are already in `Cargo.lock`
  (build dependencies of serde's derive), so adding them as dev-dependencies
  needs no download.

## Spec

1. **Dev-dependencies.** Add `syn` 2 with features `full` and `visit`, and
   `proc-macro2` 1 with feature `span-locations`. Nothing else.
2. **`tests/hygiene.rs`** walks every `.rs` file under `src/` and `tests/`
   and measures, with `syn::parse_file`:
   - **production lines** of a file: its line count minus the lines spanned
     by items that are test-only. An item is test-only when it carries
     `#[test]`, a `#[…::test]` attribute such as `#[tokio::test]`, or a
     `#[cfg(…)]` whose tokens mention `test` (`cfg(test)`,
     `cfg(all(test, unix))`). An inline `#[cfg(test)] mod tests { … }` is
     test-only; so is a test-only fn or `use` at column 0 with production
     code after it.
   - **function length** of every production fn, free, inherent-impl and
     trait-impl methods, and trait default methods: from the first line of
     its signature (attributes and doc comments excluded) to its closing
     brace. Functions inside test-only items are not measured.
3. **File classes.** A *test file* is any file under `tests/`, any
   `*_tests.rs`, and any `tests.rs`. Test files are capped at **2000**
   total lines and are not measured for production lines or function
   length. Every other file is capped at **800** production lines. The
   **exempt** list, held in `tests/hygiene.rs` with a one-line reason each,
   is exactly `src/ai/types/pending.rs` and `src/ai/tools/args.rs`; exempt
   files are measured for neither cap, and their functions still are.
   Functions are capped at **150** lines.
4. **The allowlist**, `tests/hygiene_allowlist.txt`, one entry per line:
   `file <path> <production lines>`, `test <path> <lines>`, or
   `fn <path> <name> <lines>`, where `<name>` is `fn_name` or
   `Type::method` (for a trait impl, the implementing type). If two
   measured functions in one file share a name, the later ones get `#2`,
   `#3`. `#` starts a comment line. Generate it from the measurer's own
   output against the current tree, sorted by kind then path.
5. **Four integration tests**, each naming every failing entry in its
   message, not just the first:
   - `production_files_fit_the_cap` — every non-test, non-exempt file is at
     most 800 production lines, or is allowlisted at a count at least its
     current one.
   - `test_files_fit_the_cap` — the same for test files at 2000.
   - `functions_fit_the_cap` — the same for functions at 150.
   - `allowlist_has_no_stale_entries` — every entry names a file and, for
     `fn`, a function that exists, and is still over its cap. An entry whose
     subject now fits fails with "remove it", so the list only shrinks.
6. **Unit tests of the measurer**, on inline source fixtures (strings, not
   files), in `tests/hygiene.rs`:
   `production_lines_exclude_an_inline_test_module`,
   `production_after_a_test_only_fn_is_still_counted`,
   `cfg_all_test_counts_as_test_only`,
   `cfg_not_test_is_production` (a `#[cfg(not(test))]` item is production:
   do not match the bare substring `test`),
   `function_length_spans_signature_to_closing_brace`,
   `doc_comments_and_attributes_are_not_function_lines`,
   `impl_and_trait_methods_are_measured_with_their_type`,
   `functions_in_a_test_module_are_not_measured`,
   `duplicate_names_get_a_suffix`.
7. **CLAUDE.md** § "Build & Test" gains two sentences: what
   `tests/hygiene.rs` checks, and that a phase which shrinks an offender
   below its cap deletes its allowlist line.

**Work order.** Do these in order, in the repo, and run the tests after
each step.
1. Spec 1 is already done: the two dev-dependencies are in `Cargo.toml`.
   Do not change them.
2. Write `tests/hygiene.rs` with the measurer and the nine unit tests of
   Spec 6, using the verified API below. Run `cargo test --test hygiene`
   until they pass.
3. Add the four integration tests of Spec 5, with the allowlist read from
   `tests/hygiene_allowlist.txt` (empty at first). Run them once: their
   failure messages list every offender.
4. Write `tests/hygiene_allowlist.txt` from that output. Run again: green.
5. The mutation check, CLAUDE.md, the end-to-end block, the Findings entry.

**Do not build a scratch crate** (`/tmp/...`, `cargo new`) to try the
`syn` API. The API below has been compiled and run against `syn 2.0.119`;
write it straight into `tests/hygiene.rs`. Work outside the repo counts as
no progress, and the first dispatch stalled doing exactly that. Only two
item kinds matter: an **item carrying a test-only attribute** (any kind:
its whole span is test lines), and a **function** (`ItemFn`,
`ImplItemFn`, `TraitItemFn` with a body). No other item kind needs
handling.

**Verified `syn` 2 API** (compiled and run 2026-10-25):

```rust
use syn::punctuated::Punctuated;
use syn::spanned::Spanned;            // .span() on any syntax node
use syn::{Attribute, Meta, Token};

fn meta_is_test(m: &Meta) -> bool {
    match m {
        Meta::Path(p) => p.is_ident("test"),
        Meta::List(l) if l.path.is_ident("all") || l.path.is_ident("any") => l
            .parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)
            .map(|ms| ms.iter().any(meta_is_test))
            .unwrap_or(false),
        _ => false, // not(test), unix, feature = "..": production
    }
}

fn is_test_only(attrs: &[Attribute]) -> bool {
    attrs.iter().any(|a| {
        let p = a.path();
        if p.segments.last().is_some_and(|s| s.ident == "test") {
            return true; // #[test], #[tokio::test]
        }
        p.is_ident("cfg") && a.parse_args::<Meta>().is_ok_and(|m| meta_is_test(&m))
    })
}
```

- An item's lines: `item.span().start().line ..= item.span().end().line`
  (1-based; the span **includes** its attributes and doc comments).
- A function's length: from `f.sig.span().start().line` to
  `f.block.brace_token.span.close().start().line`, inclusive. Measured on
  `/// doc\n#[inline]\npub fn e(x: u8)\n    -> u8 {\n    x\n}` starting at
  line 9: signature line 11, close line 14, so 4 lines.
- An impl's type name: `if let syn::Type::Path(tp) = &*imp.self_ty {
  tp.path.segments.last() }` and take `.ident`. Track it in your
  `visit_item_impl` before calling `syn::visit::visit_item_impl(self, i)`.
- `syn::parse_file(&source)` parses a whole file; a file that fails to
  parse is a test failure naming the file.

**Gotchas.**
- `Span::start()` is only available with `proc-macro2`'s `span-locations`
  feature; without it the build fails, or every line reads 0.
- A `cfg` whose tokens are `not(test)` is production. Match the predicate
  structurally or by token, never with `contains("test")`.
- `syn::visit::Visit` recursion: an overridden `visit_item_mod` /
  `visit_item_fn` must call `syn::visit::visit_item_mod(self, i)` (etc.) to
  descend, and must not descend into a test-only item.

## Acceptance criteria

- [ ] `cargo test --test hygiene` passes against the current tree.
- [ ] The nine unit tests in Spec 6 pass by name.
- [ ] `grep -cE '^fn src/daemon/executor/mod.rs execute_tool_call |^fn src/daemon/stream.rs run_conversation_loop ' tests/hygiene_allowlist.txt` is 2.
- [ ] `grep -cE 'src/ai/types/pending.rs|src/ai/tools/args.rs' tests/hygiene_allowlist.txt` is 0, and both paths appear in the exempt list in `tests/hygiene.rs`.
- [ ] `grep -c '^file src/search.rs ' tests/hygiene_allowlist.txt` equals whatever the measurer says: the Update Log states `src/search.rs`'s measured production lines, which must exceed the 19 a first-`#[cfg(test)]` cut would give.
- [ ] Mutation check, output pasted in the Update Log: lower one allowlisted `file` count by 1 and run `cargo test --test hygiene`; it fails naming that file. Delete one `fn` entry; it fails naming that function. Add a `file src/lib.rs 999` entry; `allowlist_has_no_stale_entries` fails with "remove it". Restore.
- [ ] Preservation: `cargo test` has no failures, and the count of passing tests is the 2.3.0 count plus the new ones.
- [ ] The Update Log has a `### Findings — <date>` entry (README decision 7), `none` allowed.

## Test plan

- The unit tests in Spec 6 and the four integration tests in Spec 5, all
  in `tests/hygiene.rs`.
- The mutation check above is the proof the gate bites.

## End-to-end verification

Run with `bash`. Paste the output byte-exact into the Update Log.

```sh
A=/tmp/e2e-m31-01.txt; : > "$A"
cargo test --test hygiene 2>&1 | grep -E '^test result' | sed 's/finished in [0-9.]*s/finished in <t>s/' >> "$A"
echo "allowlist: file=$(grep -c '^file ' tests/hygiene_allowlist.txt) test=$(grep -c '^test ' tests/hygiene_allowlist.txt) fn=$(grep -c '^fn ' tests/hygiene_allowlist.txt)" >> "$A"
echo "giants=$(grep -cE '^fn src/daemon/executor/mod.rs execute_tool_call |^fn src/daemon/stream.rs run_conversation_loop ' tests/hygiene_allowlist.txt)" >> "$A"
echo "exempt_listed=$(grep -cE 'src/ai/types/pending.rs|src/ai/tools/args.rs' tests/hygiene_allowlist.txt)" >> "$A"
grep -E '^file src/search.rs ' tests/hygiene_allowlist.txt >> "$A" || echo "search.rs not allowlisted" >> "$A"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -1 | sed 's/in [0-9.]*s/in <t>s/' >> "$A"
cargo fmt --all --check; echo "fmt_exit=$?" >> "$A"
cargo test 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "passed=" p, "failed=" f}' >> "$A"
cat "$A"
```

Expected: one `ok.` line with no failures, three nonzero allowlist counts,
`giants=2`, `exempt_listed=0`, a search.rs line or the note, clippy clean,
`fmt_exit=0`, `failed=0`.

## Authorizations

- Create `tests/hygiene.rs` and `tests/hygiene_allowlist.txt`.
- Edit `Cargo.toml` `[dev-dependencies]` (the two crates only) and let
  `Cargo.lock` follow.
- Edit `CLAUDE.md` § "Build & Test" (Spec 7 only).

## Out of scope

- Moving, splitting or editing any file under `src/`. This phase measures;
  it changes no production code.
- Acting on any finding (README decision 6).

## Update Log

<!-- entries appended below this line -->

### Notes for executor — 2026-10-25

The first dispatch stalled after 129 turns (`NoProgressStall`, 120
consecutive read-only turns). It added the dev-dependencies correctly, then
spent about 70 turns in a scratch crate in `/tmp`, printing how every item
kind (`static`, `union`, `enum`, `trait`, …) spans, and never wrote
`tests/hygiene.rs`. The Spec now carries a work order, a compiled `syn` API
excerpt, the two item kinds that matter, and a ban on scratch crates. The
`Cargo.toml` change from the first run is kept; start at work-order step 2.

### Update — 2026-10-25 (escalation)

**Chosen lever:** refined re-dispatch.
**Rationale:** the stall traces to the Spec: it named the behaviour but not
the `syn` API or an order of work, so the executor explored the API without
end instead of writing the file; a verified excerpt and a work order close
that gap, and only `Cargo.toml` was on disk, so there is little to resume.
