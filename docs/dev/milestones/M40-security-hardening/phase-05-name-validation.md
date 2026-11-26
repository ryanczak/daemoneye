# Phase 05 — name-validation

**Milestone:** M40 — Security hardening
**Status:** todo
**Drafted:** 2026-11-26
**Depends on:** none
**Estimated diff:** ~350 lines
**Tags:** language=rust, kind=bugfix, size=m

## Goal

Audit row **S8**. Several names are joined into paths under `~/.daemoneye/`
without any check:
- a memory namespace (`agents/<ns>/memory/…`);
- a session id (`var/log/sessions/<id>.jsonl`, `.meta.json`, `.archive.jsonl`);
- a prompt name (`etc/prompts/<name>.toml`);
- a saved-session name on load (`var/sessions/<name>/…`).

The architect **reproduced the worst one** on 2026-11-25 with a scratch
test, since reverted. Under a temp `HOME`:
- `save_agent` accepted `memory_namespace = "../../outside"`;
- `load_agent` returned that namespace unchanged;
- `add_memory(…, "../../outside")` returned `Ok` and created
  `$HOME/outside/`, outside `~/.daemoneye/`.

The model sets `memory_namespace` and `read_namespaces` through the
`create_agent` tool. The prompt path for `"../config"` is
`etc/prompts/../config.toml`, so it is unconfined too.

## Statement of work

1. **One validator.** Add `pub fn validate_path_name(kind: &str, name: &str)
   -> Result<(), String>` to `src/util.rs`:
   - a name is valid when it is non-empty, at most **128** bytes, and every
     byte is ASCII `A–Z`, `a–z`, `0–9`, `-` or `_`;
   - the error names `kind` (for example `"memory namespace"`) and says
     what is allowed.
2. **Memory namespaces.**
   - `agents::save_agent` and `agents::load_agent` refuse a config whose
     `memory_namespace` (after the empty-means-the-name default) or any
     `read_namespaces` entry is invalid.
   - In `src/memory.rs`, every public function that takes a namespace
     refuses an invalid one before it touches the disk: `add_memory`,
     `delete_memory`, `read_memory`, `update_memory` (its args' namespace),
     `list_memories` and `list_memories_with_tags`.
   - `load_session_memory_block` cannot fail. It **skips** an invalid
     namespace with a `log::warn!` and still loads the valid ones.
   - `"global"` stays valid.
3. **Session ids at IPC ingress.**
   - Add `fn ingress_name_error(req: &Request) -> Option<String>`, in
     `src/daemon/server/mod.rs` or a new child module `ingress.rs`. It
     returns an error for every `Request` variant whose `session_id`
     fails the validator (`Ask`'s is an `Option`), and for an `Ask` whose
     `prompt` is `Some` and invalid. `grep -n session_id src/ipc.rs` lists
     the variants.
   - `handle_client` calls it right after the request parses. On an
     error, it replies `Response::Error` in the existing
     `"Invalid request: …"` form and returns, as the parse-failure branch
     does.
4. **Prompt names.** `config::load_named_prompt` does not read the disk
   for an invalid name. It falls back exactly as for a missing file.
5. **Saved-session names.** In `src/session_store.rs`, `load_session_meta`,
   `load_session_messages`, `delete_session`, and `rename_session`'s
   **old** name refuse a name that fails `validate_path_name`. Use the new
   validator here, **not** `validate_session_name`: that one also refuses
   reserved names and the `auto-` prefix, which is stricter than a load
   needs.
6. **Docs.** Add a short `### 9. Names that become paths` guard to
   `docs/security.md` § "Guards, in order of strength". It names the
   character set, the four kinds of name, and where each is checked.

**Constraints:**

- No `unsafe` (except the existing `set_var` test idiom), `.unwrap()`,
  `.expect()` or `panic!` in production code.
- **Do not validate inside `daemon::session::session_file`, or in the meta
  and archive path builders.** Ghost session ids are
  `ghost-<runbook name>-<hex>`, and runbook names may contain `.` and
  spaces. Only **client-sent** ids are checked, at ingress.
- Ids the daemon and CLI mint today must pass:
  - the 16-hex `new_session_id()`;
  - a hyphenated UUID;
  - agent names (`[a-z0-9-]`);
  - `"global"` and `"sre"`.
- **Tests change `HOME`:** use each file's existing temp-home helper, or
  `crate::test_home_guard()` taken **once** at the top of a test.
- **No existing test is removed.** Phase 04's migration deleted one; this
  phase's last criterion checks for it.

**Closed file list:**
- `src/util.rs`;
- `src/agents/mod.rs`;
- `src/memory.rs` and `src/memory_tests.rs`;
- `src/daemon/server/mod.rs`, plus a new `src/daemon/server/ingress.rs`
  if you split it out;
- `src/config/load.rs` and `src/config/tests.rs`;
- `src/session_store.rs` and `src/session_store_tests.rs`;
- `docs/security.md`.

Read at most **3** files before your first edit.

**Edit order:** the first red test, then the validator, then memory and
agents, then ingress, then prompts, then sessions, then docs.

**Budget.** You have **1200 turns**, so use them efficiently. Your
**first tool call** after reading writes the first red test.

## First red test

`add_memory_refuses_a_traversing_namespace`, in `src/memory_tests.rs`.
- **Setup:** a temp `HOME`.
- **Call:** `add_memory("k", "v", MemoryCategory::Knowledge, "../../outside")`.
- **Asserts:** the call is `Err`, and `$HOME/outside` does not exist.

It is red today: the call returns `Ok` and creates the directory. Paste the
red run.

## Acceptance criteria

- [ ] **Validator:** `path_name_validator_accepts_and_refuses` passes, in
      `src/util.rs`.
      - **Accepts:** `global`, `sre`, `266f80f28f5db7e6`,
        `3f2a9c1e-7b4d-4e8a-9f00-12ab34cd56ef`, `my_prompt`, and a
        128-byte name.
      - **Refuses:** `""`, `..`, `.`, `../x`, `a/b`, `a.b`, `a b`,
        `a\0b`, `é`, and a 129-byte name.
- [ ] **Memory:** `add_memory_refuses_a_traversing_namespace` passes. Red
      run pasted. Also, `read_memory`, `delete_memory` and `list_memories`
      with `"../../outside"` each return `Err`.
- [ ] **Memory block:** `session_memory_block_skips_an_invalid_namespace`
      passes. `load_session_memory_block(&["global", "../../x"])` still
      contains a memory stored under `global`.
- [ ] **Agents:** `save_agent_refuses_a_traversing_namespace` passes. It
      checks both `memory_namespace = "../../outside"` and
      `read_namespaces = ["../x"]`. Red run pasted.
- [ ] **Agents, hand-edited config:** `load_agent_refuses_a_traversing_namespace`
      passes. It writes a `config.toml` with
      `memory_namespace = "../../outside"` directly, and `load_agent`
      returns `Err`. Red run pasted.
- [ ] **Ingress:** `ingress_refuses_traversing_names` passes.
      - **Some error:** `Ask` with `session_id: Some("../x")`; `Ask` with
        `prompt: Some("../config")`; `SetModel` with `session_id: "a/b"`.
      - **None:** `Ask` with a 16-hex id and `prompt: None`; `Ask` with a
        UUID id and `prompt: Some("sre")`; `Status`.
- [ ] **Ingress wired:** run this once. It prints at least **2** (it
      prints 0 today), one definition and one call in `handle_client`:
      `grep -rn "ingress_name_error" src/daemon/server/ | wc -l`
- [ ] **Prompts:** `load_named_prompt_ignores_a_traversing_name` passes,
      in `src/config/tests.rs`.
      - **Setup:** a valid prompt file written at `etc/evil.toml`, whose
        system text is `EVIL`.
      - **Assert:** `load_named_prompt("../evil")` does **not** return
        `EVIL`.
      - Red run pasted: today it does.
- [ ] **Saved sessions:** `load_session_meta_refuses_a_traversing_name`
      passes.
      - **Setup:** a valid `meta.toml` planted where `"../../x"` resolves,
        `config_dir()/x/meta.toml`.
      - **Assert:** `load_session_meta("../../x")` is `Err`.
      - Red run pasted: today it loads.
- [ ] **Preserved:** the existing `agents`, `memory_tests`,
      `session_store_tests` and `config::tests` suites pass unchanged.
- [ ] **No test removed:** run this once. It prints **0**:
      `git diff 8c8ddcf0 -- src | grep -cE '^-\s*#\[(tokio::)?test\]'`
- [ ] **Gates:** all four pass, run once each. The lib suite reports **more
      than 3149** passed (3149 today).

## Spec

1. **Break down:** write the first red test and run it red. Then write a
   numbered task list into an Update Log entry, one line per task, each
   naming the acceptance item it closes. Done when that entry is written.
2. **Implement:** in the edit order, writing each red test before its fix.
   Done when `cargo build --tests` is green.
3. **Meet the acceptance criteria:** check off every item. Run each
   command **once**. Done when every item is checked with its evidence
   named.
4. **Capture the evidence:** run the four gates, once each. Paste into a
   new Update Log entry:
   - each `test result:` line, without the per-test `… ok` lines;
   - the red runs, **as output, not summarized**;
   - the two greps.

   Commit as one `fix:` commit.

## Authorizations

- You may touch the files in the closed file list, and no others.
- You may add `src/daemon/server/ingress.rs`.

## Out of scope

- **Run ids:** `RunId::parse` already holds them to a hex grammar.
- **Runbook, script and agent names:** each has its own validator.
- **Ghost session ids:** they are daemon-minted from validated runbook
  names, and are not checked at ingress.
- **Changing `memory_dir_for_namespace`** or the session path builders to
  return a `Result`.
- **`validate_session_name`'s rules** for saving a session.

## Update Log

<!-- entries appended below this line -->
