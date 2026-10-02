# Phase 04c: hosts-context-block — the model sees a `[HOSTS]` line naming the configured hosts

**Milestone:** M23 — Client decoupling & shell UX
**Status:** todo
**Depends on:** 04b (done)
**Estimated diff:** ~55 lines of production code across 3 files; 3 new tests
**Tags:** language=rust, kind=feature, size=s

## Goal

Today the model learns which remote hosts exist only by calling a tool or
from earlier conversation. After this phase, every chat prompt (first turn and
later turns) carries one ambient line, placed just before `[SHELLS]`:

```text
[HOSTS] local (1 shell) · db01 (ssh admin@db01, no shells) · web01 (ssh matt@web01.lan, 2 shells)
```

That is the implicit `local` host, then each configured `[hosts.<name>]`
entry **sorted by name**, with its ssh destination and its count of **live**
shells across every owner. The block is **empty** when no hosts are
configured, so single-host users pay no tokens for it.

This is the `[HOSTS]` block the 2.0 design names
(`docs/design/daemoneye-2.0.md`, line ~353). It was assigned to M22 but never
built: no `[HOSTS]` string exists anywhere in `src/` today.

## Architecture references

- `docs/design/daemoneye-2.0.md` § 2.5, the ambient context example:
  `[HOSTS] local · web01 (ssh matt@web01.lan, 2 shells) · db01 (configured, idle)`.
  This phase keeps that shape. It drops "idle/configured": that would need a
  ControlMaster probe, and the block must not do I/O (below). It shows the
  shell count instead.
- `docs/dev/milestones/M23-client-decoupling/README.md` — row 04c.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read this entire phase doc before touching any code.
3. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

Measured on **2026-10-02** at commit `76628b4b`. Baseline:
`cargo test --lib` → **2130 passed / 0 failed / 14 ignored**.

**The whole change below was dry-run in this tree, then reverted.** Results
on the dry run:
- the full suite gave lib **2133**/0/14 on three runs, with every other
  target unmoved;
- fmt and clippy `-D warnings` were clean;
- Mutations A–D each went red;
- **both** prompt builders were exercised by one test, which runs in about
  0.02 s.

### The analogue: `[SHELLS]` (`src/daemon/executor/knowledge/shell.rs:195-297`)

The ambient shell block is built by a **pure** `format_shell_context(rows,
owner_key, now_unix)` over `ShellRow`s, with a thin public wrapper
`shell_context_block(caller)` that reads the registry. Its doc comment states
the rule this phase follows: the block runs **on every prompt assembly and
must do no I/O**. It also returns the empty string when there is nothing to
say, which is "pure token cost for no information" otherwise.

`ShellRow.host` is a `crate::host::HostId` (`Local` or `Named(String)`), and
`collect_row(&ShellRecord) -> ShellRow` (`:112`) builds one.

### Where the block goes (`src/daemon/prompt.rs`)

`PromptCtx` already carries `config: &crate::config::Config`, so the hosts are
`ctx.config.hosts` (`HashMap<String, HostEntry>`, `HostEntry.ssh` the
destination). **Do not call `Config::load()`.**

Both builders compute the shell block the same way (`:137` and `:219`):

```rust
    let shell_block = crate::daemon::executor::knowledge::shell_context_block(
        &crate::shell::Owner::Chat(ctx.session_id.unwrap_or_default().to_string()),
    );
```

They splice it in three format strings: `:155` (first turn,
`         {shell_block}\`), and `:262` / `:269` (later turns,
`…{fg_target_line}{shell_block}{tool_restriction_block}…`).

`prompt.rs` has **no** test module today.

### The re-export (`src/daemon/executor/knowledge/mod.rs:25`)

```rust
pub use shell::{ContextBreadth, resolve_context_scope, shell_context_block, shells_only_context};
```

## Spec

### Task 1 — the block (`src/daemon/executor/knowledge/shell.rs`)

Insert this **above** the doc-comment line that begins `/// The breadth a` (the line before `pub enum ContextBreadth`)
(`:299`). It is the dry-run code, formatted and clippy-clean:

```rust
/// Format the ambient hosts block: the implicit `local` host, then each
/// configured `[hosts.<name>]` entry by name, with its ssh destination and how
/// many **live** shells run on it, across every owner. Pure: tests hand-build
/// the hosts and the rows.
///
/// Returns the **empty string** when no hosts are configured: a lone
/// `local` says nothing the model does not already know, and costs tokens on
/// every turn. No ssh probe here either: this runs on every prompt assembly,
/// and ControlMaster state is `/hosts`' job.
fn format_hosts_context(
    hosts: &std::collections::HashMap<String, crate::config::HostEntry>,
    rows: &[ShellRow],
) -> String {
    if hosts.is_empty() {
        return String::new();
    }
    let shells_on = |host: &crate::host::HostId| match rows
        .iter()
        .filter(|row| &row.host == host && row.exited_unix.is_none())
        .count()
    {
        0 => "no shells".to_string(),
        1 => "1 shell".to_string(),
        n => format!("{n} shells"),
    };
    let mut names: Vec<&String> = hosts.keys().collect();
    names.sort();
    let mut parts = vec![format!(
        "local ({})",
        shells_on(&crate::host::HostId::Local)
    )];
    for name in names {
        let host = crate::host::HostId::Named(name.clone());
        parts.push(format!(
            "{name} (ssh {}, {})",
            hosts[name].ssh,
            shells_on(&host)
        ));
    }
    format!("[HOSTS] {}\n", parts.join(" · "))
}

/// The ambient `[HOSTS]` block for a prompt, or the empty string when
/// `config` has no `[hosts]` entries. Counts come from every shell in the
/// registry, whoever owns it.
pub fn hosts_context_block(config: &crate::config::Config) -> String {
    let rows = with_shells(crate::shell::registry(), |store| {
        store.values().map(collect_row).collect::<Vec<_>>()
    });
    format_hosts_context(&config.hosts, &rows)
}
```

### Task 2 — re-export (`src/daemon/executor/knowledge/mod.rs:25`)

Add `hosts_context_block` to that `pub use shell::{…}` line. `cargo fmt`
will wrap it.

### Task 3 — splice it into both prompts (`src/daemon/prompt.rs`)

1. Right after **each** of the two `let shell_block = …;` statements
   (`:137`, `:219`), add:

   ```rust
    let hosts_block = crate::daemon::executor::knowledge::hosts_context_block(ctx.config);
   ```

2. In all **three** format strings, put `{hosts_block}` immediately before
   `{shell_block}`:
   - `:155` becomes `         {hosts_block}{shell_block}\`;
   - `:262` and `:269` become `…{fg_target_line}{hosts_block}{shell_block}…`.

### Task 4 — tests

Three tests, names pinned.

**In `src/daemon/executor/knowledge/shell.rs`'s `mod tests`**, right above
`fn row_on(` (`:2141`; the tests use it and the existing `row(…)` helper and
`NOW`), add:

```rust
fn host_entry(ssh: &str) -> crate::config::HostEntry {
    crate::config::HostEntry {
        ssh: ssh.to_string(),
        ..Default::default()
    }
}

#[test]
fn hosts_context_lists_local_then_each_host_with_live_shells() {
    let mut hosts = std::collections::HashMap::new();
    hosts.insert("web01".to_string(), host_entry("matt@web01.lan"));
    hosts.insert("db01".to_string(), host_entry("admin@db01"));
    let web = crate::host::HostId::Named("web01".to_string());
    let mut gone = row_on("s4", "chat:a", web.clone(), NOW);
    gone.exited_unix = Some(NOW);
    let rows = vec![
        row("s0", "chat:a", NOW, false, false, None),
        row_on("s2", "chat:a", web.clone(), NOW),
        row_on("s3", "ghost:job", web, NOW),
        gone,
    ];
    assert_eq!(
        format_hosts_context(&hosts, &rows),
        "[HOSTS] local (1 shell) · db01 (ssh admin@db01, no shells) · web01 (ssh matt@web01.lan, 2 shells)\n"
    );
}

#[test]
fn hosts_context_is_empty_without_configured_hosts() {
    let rows = vec![row("s0", "chat:a", NOW, false, false, None)];
    assert_eq!(
        format_hosts_context(&std::collections::HashMap::new(), &rows),
        ""
    );
}
```

**At the end of `src/daemon/prompt.rs`**, add this new test module. It
builds **both** real prompts with a temp `HOME`; `SessionCache` is already
imported at `:1`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn ctx<'a>(cache: &'a SessionCache, config: &'a crate::config::Config) -> PromptCtx<'a> {
        PromptCtx {
            client_pane: None,
            chat_pane: None,
            default_target_pane: None,
            cache,
            config,
            chat_width: None,
            safe_query: "hello",
            last_prompt_tokens: 0,
            history_count: 0,
            this_turn_count: 0,
            ghost_turn_limit: None,
            inject_snapshot: false,
            memory_namespaces: &["global"],
            session_id: None,
            tool_policy: None,
            agent_name: None,
        }
    }

    #[test]
    fn prompts_carry_the_hosts_block() {
        let _home = crate::test_home_guard();
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("HOME", dir.path()) };
        let cache = SessionCache::new("t");
        let mut config = crate::config::Config::default();
        config.hosts.insert(
            "pinky".to_string(),
            crate::config::HostEntry {
                ssh: "pinky".to_string(),
                ..Default::default()
            },
        );
        let prompt = build_subsequent_turn_prompt(&ctx(&cache, &config));
        assert!(prompt.contains("[HOSTS] local ("), "{prompt}");
        assert!(prompt.contains("pinky (ssh pinky, "), "{prompt}");
        assert!(
            prompt.find("[HOSTS]") < prompt.find("User: hello"),
            "the block is in the context, ahead of the user's text: {prompt}"
        );

        let first = build_first_turn_prompt(&ctx(&cache, &config));
        assert!(first.contains("pinky (ssh pinky, "), "{first}");

        let plain = crate::config::Config::default();
        let without = build_subsequent_turn_prompt(&ctx(&cache, &plain));
        assert!(
            !without.contains("[HOSTS]"),
            "no hosts, no block: {without}"
        );
    }
}
```

**Test count:** three new, so **2133 passed / 14 ignored**.

### Task 5 — capture the end-to-end evidence

Run the block in § End-to-end verification verbatim. Then create a new Update
Log entry titled exactly `### Update — <date> (end-to-end verification)`.
**Its first fenced block is the contents of `/tmp/e2e-m23-04c.txt`**: the
file's lines, copied by a tool. **Never paste the script itself**, and never
retype the lines (the real lines end `lib1=ok ` **with a trailing space**).
Then run the paste check once, put the line it prints into the entry **on its
own line**, and record the four mutations after it.

## Acceptance criteria

Parenthesised values are what each line produces against the current tree.
Every non-preservation line fails today, and each was run at drafting.

- [ ] `cargo test --lib` → **2133 passed / 0 failed / 14 ignored** on three
      consecutive runs (current: 2130/0/14). Both counts are exact.
- [ ] `cargo fmt --all` clean; `cargo build` clean;
      `cargo clippy --all-targets --all-features -- -D warnings` exits 0.
- [ ] `cargo test` → other targets unmoved: bug_tracker 6, doc_truth 14,
      integration 32, isolation 11 (*preservation*).
- [ ] The three new tests pass:
      ``cargo test --lib -- hosts_context prompts_carry_the_hosts_block`` →
      **3 passed** (current: `0 passed`).
- [ ] All three prompt strings carry the block:
      ``grep -c '{hosts_block}{shell_block}' src/daemon/prompt.rs`` → **3**
      (current: 0).
- [ ] Mutations A–D were applied, run, and their observed results recorded
      **in this doc**.
- [ ] The end-to-end entry exists:
      ``grep -c '^### Update — .* (end-to-end verification)$' <this doc>`` →
      **1** (current: 0). Its first fenced block is the capture file's contents,
      and the paste-check line is on its own line:
      ``grep -cx 'PASTE MATCH' <this doc>`` → **1** (current: 0).

## Mutations

Apply each with the `patch` tool (the executor contract forbids `sed -i`,
`perl -i`, `> file` and `rm -rf`). Run once, restore, and **record the
observed result**. Each was measured red at drafting.

- **Mutation A:** in `format_hosts_context`, change
  `.filter(|row| &row.host == host && row.exited_unix.is_none())` to
  `.filter(|row| &row.host == host)`. Run
  `hosts_context_lists_local_then_each_host_with_live_shells`. Expected
  **red** (the exited web01 shell is counted).
- **Mutation B:** in the first-turn format string, change
  `{hosts_block}{shell_block}` back to `{shell_block}`. Run
  `prompts_carry_the_hosts_block`. Expected **red**.
- **Mutation C:** in the later-turn format string that ends `User: {}",`
  (the one used when `inject_snapshot` is false), make the same change. Run
  `prompts_carry_the_hosts_block`. Expected **red**.
- **Mutation D:** delete the `if hosts.is_empty() { return String::new(); }`
  guard. Run `hosts_context_is_empty_without_configured_hosts`. Expected
  **red**.

After each restore, confirm `git diff --stat` matches its pre-mutation state.

## Test plan

Three tests:
- the exact line: `local` first, hosts sorted by name, ssh destinations, and
  live shells counted across owners (exited shells excluded);
- no block when no hosts are configured;
- both real prompt builders carry the line, and do not when the config has
  no hosts.

## End-to-end verification

```sh
cd /home/matt/src/daemoneye
{
  for i in 1 2 3; do
    out=$(cargo test --lib 2>&1)
    printf 'lib%d=%s %s\n' "$i" "$(printf '%s\n' "$out" | grep -E '^test result' | tail -1 | grep -oE 'ok|FAILED')" "$(printf '%s\n' "$out" | grep -E '^test .* FAILED$' | sed -E 's/^test (.*) \.\.\. FAILED$/\1/' | tr '\n' ' ')"
  done
  printf 'lib_counts=%s\n' "$(cargo test --lib 2>&1 | grep -E '^test result' | tail -1 | sed -E 's/.*ok\. ([0-9]+) passed; ([0-9]+) failed; ([0-9]+) ignored.*/\1p-\2f-\3i/')"
  printf 'new_tests=%s\n' "$(cargo test --lib -- hosts_context prompts_carry_the_hosts_block 2>&1 | grep -E '^test result' | tail -1 | sed -E 's/.*ok\. ([0-9]+) passed; ([0-9]+) failed.*/\1p-\2f/')"
  printf 'isolation=%s\n' "$(cargo test --test isolation 2>&1 | grep -E '^test result' | tail -1 | grep -oE '[0-9]+ passed; [0-9]+ failed')"
  printf 'splices=%s\n' "$(grep -c '{hosts_block}{shell_block}' src/daemon/prompt.rs)"
} > /tmp/e2e-m23-04c.txt
cat /tmp/e2e-m23-04c.txt
```

Expected:
- `lib1..3=ok` with **nothing after `ok`**;
- `lib_counts=2133p-0f-14i`;
- `new_tests=3p-0f`;
- `isolation=11 passed; 0 failed`;
- `splices=3`.

**Re-run budget: three** (WORKFLOW.md § End-to-end verification). The known
pre-existing flake is `shell::backend::tests::input_after_a_prompt_logs_a_redaction_marker`.
If it fails, name it and re-run the whole block. After three re-runs, stop,
paste the last capture as it is, and name every failing test. Any other
failing test is a finding: report it by name.

**Run this block with nothing else running `cargo test`.** Then make the
entry's first fenced block the **contents of** `/tmp/e2e-m23-04c.txt`
(copied, not retyped; **never the script above**), and run:

```sh
D=docs/dev/milestones/M23-client-decoupling/phase-04c-hosts-context-block.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-m23-04c.txt
diff /tmp/pasted-m23-04c.txt /tmp/e2e-m23-04c.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

If it prints `PASTE MISMATCH`, run
`diff /tmp/pasted-m23-04c.txt /tmp/e2e-m23-04c.txt` once and fix what it
shows. **Run the check at most three times.** Put the line it prints into the
entry on its own line.

## Authorizations

You may edit these files, and nothing else:

- `src/daemon/executor/knowledge/shell.rs`: Task 1 and its tests.
- `src/daemon/executor/knowledge/mod.rs`: the one re-export.
- `src/daemon/prompt.rs`: Task 3 and the new test module.
- This phase doc's Update Log.

## Out of scope

- **Ghost-shell prompts.** Only the two chat prompt builders change.
- **ControlMaster state in the block**: `/hosts` (04b) shows it; the block
  must not spawn `ssh` every turn.
- **`sre.toml` prose.** The line is self-describing, and the prompt asset is
  guarded by `doc_truth`.

## Update Log

<!-- entries appended below this line -->
