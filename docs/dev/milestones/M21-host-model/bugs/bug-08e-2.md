# Bug 2 on phase-08e: a ghost's edit_file connects to any host before edit_file refuses ghosts

**Severity:** major
**Status:** closed 2026-09-29 — verified fixed at review. The `EditFile` arm connects only when `edit_file_connects_first(is_ghost, host)` is true, and that is false for every ghost. `a_ghost_edit_file_never_connects` passes, and dropping `!is_ghost` from the gate turns it red. Chat still connects first (`callers=1-3-2-1` unchanged).
**Filed:** 2026-09-28

## What's wrong

Spec Task 3.2: "Place it **after** each arm's existing ghost `hosts:` gate,
**so a refused host is never connected to**."

The `EditFile` arm has no ghost gate of its own — `edit_file` refuses every
ghost *inside* `run_edit_file`
(`src/daemon/executor/file_ops/write.rs:89-93`, "file operations require user
approval and cannot run in a Ghost Shell"). The connect step was placed at the
top of the arm (`src/daemon/executor/mod.rs:1015-1027`), **before** that
refusal:

```rust
            if let Err(msg) =
                connect_named_host(host.as_deref(), is_ghost, id.as_str(), tx, rx).await
```

So a ghost calling `edit_file { host: "<anything>" }` now runs
`ssh -M -N -f -o BatchMode=yes <anything>` — `host_entry_or_bare` accepts an
unconfigured name as its own destination — and leaves a ControlMaster up for
10 minutes, to a host its runbook's `hosts:` list never named, before being
told file operations are refused. Before this phase the same call made no
network connection at all. 08d's whole row is the fence around which hosts a
ghost may reach; this opens a side door through a tool ghosts cannot even use.

(The spec's § Current state 5 implied every arm had a gate; `EditFile`
doesn't. The invariant it stated — a refused call is never connected — is
what governs.)

## What should happen

A ghost's `edit_file` call never starts an ssh process; it returns the
existing "cannot run in a Ghost Shell" refusal exactly as before this phase.
Chat `edit_file` keeps connecting first, as specced.

## Root cause

`executor/mod.rs:1015` calls `connect_named_host` unconditionally for the
`EditFile` arm, and the only ghost refusal for this tool lives downstream in
`file_ops/write.rs:89`.

## Definition of done

- [ ] A test proves that for a ghost (`is_ghost = true`), the `edit_file`
      path with `host: Some("<a name that is not configured>")` returns a
      result containing `cannot run in a Ghost Shell` and **never reaches
      `ensure_connected`** (the executor chooses how to make that observable
      — e.g. a small extracted decision function over `(is_ghost, host)`
      with a unit test). Confirm the test fails, or cannot be written,
      against the current tree before fixing.
- [ ] Chat (`is_ghost = false`) `edit_file` with a named host still connects
      first (the three `connect_named_host(` call sites in `executor/mod.rs`
      remain, or the phase doc's `callers=` criterion is re-pinned with the
      reason).
- [ ] `cargo test --lib` → 0 failed, ignored count **12**.

Authorization for this fix: `src/daemon/executor/mod.rs` (the `EditFile` arm
and a test); `src/daemon/executor/file_ops/write.rs` only if the chosen fix
needs it.
