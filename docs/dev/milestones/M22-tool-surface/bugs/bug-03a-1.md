# Bug 1 on phase-03a: the tool description tells the model to pass shell ids to tools that have no shell parameter

**Severity:** major
**Status:** open
**Filed:** 2026-09-12

## What's wrong

`list_shells` is a **core** tool, so its `ToolDef.description` is sent to the
model on **every request**. It currently says (`src/ai/tools/defs.rs`, the
`list_shells` literal):

> Each shell has an id like "s3" — that id is what run_terminal_command
> (background=true), send_input, read_shell and the other shell tools take as
> their target. Call this before any shell tool call so you pass an id that
> exists.

**None of that is true on this tree.** Verified at review:

- **No tool has a `shell` parameter at all**: `grep -n 'name: "shell"'
  src/ai/tools/defs.rs` → no match.
- **`run_terminal_command`'s parameters are `command`, `background`,
  `target_pane`, `retry_in_pane`** — there is nowhere to put a shell id. It
  gains `shell?` in **phase 07**, not here.
- **`send_input` does not exist.** Its only occurrence in `defs.rs` is inside
  this very description; the tool arrives in **phase 05**.
- **`read_shell` does not exist.** It arrives in **03b**.

`assets/prompts/sre.toml` repeats it and adds two more problems:

> Call it before any shell-targeting tool call (`run_terminal_command` with
> `background=true`, `send_input`, and the 03b shell readers) so you pass an
> id that exists instead of guessing; a wrong or stale id is refused with an
> error naming the shells that are live.

- **"the 03b shell readers"** cites an internal milestone phase id in a
  runtime prompt asset. It is meaningless to the model and rots at the next
  renumbering.
- **"a wrong or stale id is refused with an error naming the shells that are
  live"** describes an error path that does not exist — nothing accepts a
  shell id, so nothing refuses one.

The consequence is behavioural, not cosmetic: the model is being instructed,
on every request, to obtain a shell id and pass it to `run_terminal_command`.
There is no such parameter, so the attempt is wasted at best and produces a
malformed call at worst.

**Everything else about the phase is correct** and was verified independently
at review: 40 tools / 28 core, `Category::Read`, not approval-gated,
`enum_values` untouched, the formatter pure, numeric ordering, all four gates,
1828 lib tests, `knowledge::shell` 8, `doc_truth` 10. **Both mutation checks
reproduce by hand under the spec's own mutants** — including mutation 2 applied
as *specified* (sorting on the id string), which the executor had substituted
with a constant key: 7 passed / 1 failed, the ordering test alone. The paste is
byte-identical to the capture (3297 bytes, `PASTE MATCH`). This bug is about
two prose blocks, and **no code change is needed**.

## What should happen

`STANDARDS.md` § 1 requires acceptance criteria to hold against real artifacts.
A shipped prompt asset is a real artifact, and a tool description is the part of
it the model is most likely to act on, so it must describe **this tree**:

- Say what the id is *for* without naming a consumer that cannot accept it.
  `list_shells` is currently an **observability** tool: it tells the model which
  shells exist and what state they are in. That is worth saying on its own.
- Name no tool that does not exist. When `run_terminal_command` gains `shell?`
  in phase 07 and `send_input` lands in 05, those phases update this text —
  each is already obliged to by the eleven-step checklist.
- Cite no internal milestone or phase identifier in `assets/prompts/sre.toml`.
- Claim no error path that is not implemented.

## Root cause

**Architect-side: the spec dictated the false sentence.** § Spec task 4 of the
phase doc says the description must tell the model

> that the id is what `run_terminal_command` and the other shell tools take

That was written as if phase 07's reshape had already landed. The executor
followed the spec exactly and extended it to the two other tools the same
sentence implies. The `sre.toml` prose then elaborated the same claim, and the
`03b` reference came from the phase doc's own split rationale, which the
executor had read.

This is the third M22 phase whose defect traces to the spec rather than to the
executor's work. Recorded in the milestone README's Notes as a pattern to watch
at close.

## Definition of done

Each was run against the current tree (commit `00299c25`) and returns the "now"
value shown. The `ToolDef` greps are scoped with
`sed -n '/name: "list_shells"/,/^    },/p' src/ai/tools/defs.rs |` and the
prompt greps with
`sed -n '/^### \`list_shells\`/,/^### \`list_panes\`/p' assets/prompts/sre.toml |`,
so neither this bug doc nor the phase doc can satisfy them.

- [ ] **The tool description names no tool that cannot take a shell id:**
      `<ToolDef scope> | grep -c 'run_terminal_command'` → **0** (now **1**),
      and `<ToolDef scope> | grep -cE 'send_input|read_shell'` → **0** (now
      **2**).
- [ ] **The prompt asset does the same:**
      `<sre scope> | grep -c 'run_terminal_command'` → **0** (now **1**), and
      `<sre scope> | grep -c 'send_input'` → **0** (now **1**).
- [ ] **No internal phase id in the prompt asset:**
      `<sre scope> | grep -c '03b'` → **0** (now **1**).
- [ ] **No unimplemented error path claimed:**
      `<sre scope> | grep -ci 'refused'` → **0** (now **1**).
- [ ] **The description still does its job** — it must still name the tool's
      purpose and its two scope values:
      `<ToolDef scope> | grep -c '\\"mine\\"'` → **1 or more**, and
      `<ToolDef scope> | grep -c '\\"all\\"'` → **1 or more** (both already
      **1**; they must stay non-zero). *(Escaped quotes are required — the Rust
      literal holds `\"mine\"`, so a plain `'"mine"'` pattern matches nothing.)*
- [ ] **No code change and no count change:**
      `git diff 00299c25..HEAD --stat -- src/ai/types/ src/daemon/ | wc -l` →
      **0**; `grep -c 'ToolDef {' src/ai/tools/defs.rs` → **40**;
      `cargo test --lib` → **1828**; `cargo test --test doc_truth` → **10**;
      all four gates pass.
