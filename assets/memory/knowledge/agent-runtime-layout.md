---
tags: [daemoneye, filesystem, layout, directories, runtime, var, log, agents, sessions, schedules]
summary: Full ~/.daemoneye/ directory layout — paths, purposes, and access notes for every subdirectory and key file
relates_to: [ghost-shell-guide, scheduling-guide, scripts-and-sudoers, runbook-format]
---

# Agent Runtime Layout

`~/.daemoneye/` is the daemon and agent runtime root. All persistent knowledge,
automation, configuration, and logs live under it.

## Directory Tree

```
~/.daemoneye/
  etc/
    config.toml              ← daemon configuration (models, prompt, webhook, ghost, limits)
    prompts/
      sre.toml               ← built-in SRE system prompt (overwritten on --overwrite-prompt)
      <name>.toml            ← additional prompt profiles

  agents/
    <name>/
      config.toml            ← named agent profile (prompt, model, tool policy, memory namespace)
      briefing.md            ← rolling post-session briefing (auto-generated on clean exit)
      mailbox/
        <job_id>.json        ← mailbox result written by child ghost on exit

  bin/                       ← place symlinks / wrappers here (on PATH for systemd service)

  scripts/                   ← executable automation (.sh / .py, chmod 700)
  runbooks/                  ← procedure runbooks (markdown + YAML frontmatter)

  memory/
    session/                 ← user prefs, always injected at session start
    knowledge/               ← technical facts, loaded on-demand via tags
    incident/                ← post-mortems, never auto-loaded

  var/
    run/
      daemoneye.sock         ← Unix domain socket (IPC)
      schedules.json         ← scheduled job store (atomic JSON)
      pane_prefs.json        ← per-session foreground pane preferences

    log/
      daemon.log             ← daemon process log (structured JSON lines)
      events/
        events-<date>.jsonl  ← structured event log (dated segments, searchable via search_repository)
      panes/                 ← archived background-window scrollback (.log files)
      pipe/                  ← live pipe-pane capture logs (ephemeral, ANSI-stripped)
      sessions/
        <id>.jsonl           ← per-session JSONL conversation history (ephemeral)

    sessions/                ← named session persistent store
      index.json             ← session index (name → metadata)
      <name>/
        meta.toml            ← session metadata (saved_name, artifacts_created, …)
        messages.jsonl       ← full conversation history
```

## Access Notes

### Blocked files (read_file blocked — contain API credentials)
- `etc/config.toml`
- `etc/prompts/sre.toml`

### edit_file blocked from entire `~/.daemoneye/` tree
Use dedicated CRUD tools for all daemoneye-managed artifacts:
- Scripts → `write_script` / `read_script` / `list_scripts` / `delete_script`
- Runbooks → `write_runbook` / `read_runbook` / `list_runbooks` / `delete_runbook`
- Memory → `add_memory` / `read_memory` / `update_memory` / `delete_memory` / `list_memories`

### Log and data files (readable via read_file)
- `var/log/panes/<name>.log` — archived background-window output; path shown in
  `[Background Task Completed]` when output is truncated. Page with `read_file(path)`.
- `var/log/events/events-<date>.jsonl` — prefer `search_repository(kind:"events")` for keyword search;
  use `read_file` for raw tail or line-range reads.
- `var/log/daemon.log` — daemon internals; useful for debugging stuck commands or
  missed hooks.
- `var/log/pipe/<id>.log` — live terminal capture for the active foreground pane.
  Up to 50 KB, ANSI-stripped. Used by `read_file` with a local `target_pane`.
- `var/log/sessions/<id>.jsonl` — ephemeral per-session history. Importable with
  `daemoneye session import <id> --name <name>`.
- `var/run/schedules.json` — raw schedule store; prefer `list_schedules` tool.

### Agent mailbox
When a coordinator spawns a specialist with `spawn_ghost_shell(agent: "name", …)`,
the specialist writes its result to `agents/<name>/mailbox/<job_id>.json` on exit.
The coordinator reads it with `await_agent_result(job_id, agent_name)`. Mailbox files
are masked before write and persist until the coordinator reads them.

### Named session store vs. ephemeral session log
- `var/sessions/<name>/` — persistent named sessions (saved via `/session save`).
- `var/log/sessions/<id>.jsonl` — ephemeral; one file per daemon session ID.
  These are NOT the same. The index at `var/sessions/index.json` maps names to metadata.
