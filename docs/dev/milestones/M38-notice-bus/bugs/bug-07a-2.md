# Bug 2 on phase-07a: the docs document a `routes` key that does not exist

**Severity:** minor
**Status:** open
**Filed:** 2026-11-17

## What's wrong

`NotificationsConfig` names its field `routes` with
`#[serde(rename = "route")]`. The doc-truth gates
(`tests/doc_truth.rs`, `struct_fields`) read Rust field names, not serde
names, so they asked for a `routes` key. The phase satisfied them with a
phantom key:

- `assets/etc/config.toml`: `# routes = []`. A user who uncomments it gets
  a key serde ignores silently, because the real key is the
  `[[notifications.route]]` array.
- `assets/memory/knowledge/config-reference.md`: a `notifications.routes`
  row.

## What should happen

The Rust field name matches the TOML key, so every gate and every doc names
`route`.

## Root cause

The serde `rename` splits the Rust name from the TOML name, and the gate
only sees the Rust name.

## Definition of done

- [ ] `grep -n 'rename = "route"' src/config/types.rs` prints nothing. The
      field is named `route`.
- [ ] `grep -n '^# routes = \[\]' assets/etc/config.toml` prints nothing.
- [ ] `grep -n 'notifications.routes' assets/memory/knowledge/config-reference.md`
      prints nothing; the row names `notifications.route`.
- [ ] `seeded_config_template_documents_every_config_field`,
      `seeded_config_template_has_no_phantom_keys` and
      `config_reference_documents_every_config_field` pass.
