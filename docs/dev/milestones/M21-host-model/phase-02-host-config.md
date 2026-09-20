# Phase 02: `[hosts.<name>]` becomes real config

**Milestone:** M21 — Host model & SSH
**Status:** in-progress
**Depends on:** none in M21 — this is pure and additive. 01a/01b are `done`
but nothing here touches them.
**Estimated diff:** ~320 lines
**Tags:** language=rust, kind=feature, size=s

## Goal

A remote host stops being a string buried in a runbook and becomes a named,
validated config entry the rest of M21 can resolve against:

```toml
[hosts.web01]
ssh = "matt@web01.lan"
tags = ["web", "prod"]

[hosts.db01]
ssh = "db01"
sudo_scripts = ["pg-failover"]
```

Plus `HostId`, and a runbook `hosts: [...]` frontmatter field so a ghost can
later be restricted to named hosts.

**Nothing executes anything.** No SSH, no shell spawning, no ControlMaster.
This phase is types, parsing, validation and documentation only — the surface
rows 03–08 build on.

## Architecture references

- `docs/design/daemoneye-2.0.md` § 2.2 "Host model" — the `[hosts]` shape
  quoted verbatim in the Goal above, and *"`Host::Local` is implicit. Every
  configured host is an execution target; none stores artifacts."*
- `docs/architecture.md` § 2.4 — the remote execution model, **unchanged**:
  the daemon host stores every managed artifact; remotes are execution targets.
- § 3 "Security model deltas" — *"Runbooks gain `hosts: [...]` — a ghost may
  open shells only on listed hosts."* This phase adds the field and parses it;
  **enforcement is row 06.**

## Pre-flight

Run these and confirm, at HEAD `6fdfe57d`:

```sh
git rev-parse --short HEAD                                   # 6fdfe57d
ls src/host.rs                                               # does not exist
grep -c 'pub hosts' src/config/types.rs                      # 0
grep -c 'hosts' src/runbook.rs                               # 0
cargo test --lib | tail -3                                   # 1898 passed, 4 ignored
cargo test --test doc_truth | tail -3                        # 14 passed
cargo test --lib 'config' | tail -3                          # 104 passed
cargo test --lib 'runbook' | tail -3                         # 36 passed
```

Measured, not estimates. Task 6's tests land on top of **104** and **36**.

## Current state

### The struct name decides whether the doc gate sees this section at all

**This was probed at drafting, not reasoned about.** `tests/doc_truth.rs`
discovers config sections by walking `Config`'s fields and keeping those whose
type name **ends with `Config` or `Entry`**:

```rust
        if ty.ends_with("Config") || ty.ends_with("Entry") {
            out.push((name.trim().to_string(), ty.to_string()));
        }
```

The probe: add `pub hosts: HashMap<String, X>` to `Config` and run
`cargo test --test doc_truth`.

| Struct named | `doc_truth` result |
|---|---|
| `HostSpec` | **14 passed** — the section is invisible to the gate; a user-facing config section ships wholly undocumented and nothing complains |
| `HostEntry` | **FAILED**, naming `"[hosts] ssh"` and `"[hosts] tags"` as *config fields with no example in assets/etc/config.toml* |

**So the struct is named `HostEntry`, matching `ModelEntry`**, and the failure
above is the gate doing its job — you make it pass by documenting the fields in
the template, not by renaming the struct.

**The milestone README calls this type `HostSpec`. That is wrong and this phase
supersedes it** — the README was written from the design's prose before the
gate's discovery rule was measured. Do not "fix" the name back.

### The phantom-key half does *not* cover `[hosts.*]`, and must be made to

Second probe: with the struct named `HostEntry`, a deliberately invented key
added under a `[hosts.web01]` block in the template —

```toml
# [hosts.web01]
# ssh = "matt@web01.lan"
# totally_made_up_key = "should be caught as phantom"
```

— `cargo test --test doc_truth` still reports **14 passed**. The phantom key is
**not** caught. The reason is in the check itself:

```rust
        let lookup = if section.starts_with("models") {
            "models"
        } else if section.contains('.') {
            continue;
        } else {
            base.as_str()
        };
```

`[hosts.web01]` contains a `.`, so it is skipped. `models` has an explicit
special-case; `hosts` needs the same one. Task 5 adds it, and Task 6's mutation
proves it works by re-introducing exactly the phantom key above.

### The worked example to copy

`ModelEntry` (`src/config/types.rs:968`) is the same shape — a named free-form
map of entry structs, with a doc comment carrying a TOML example:

```rust
/// Per-model AI provider configuration.  Define one or more named entries in
/// `config.toml` under `[models.<name>]`.  A `[models.default]` entry is
/// required; it is used when no model override is in effect.
///
/// Example:
/// ```toml
/// [models.default]
/// provider = "anthropic"
/// model    = "claude-sonnet-4-6"
/// ```
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ModelEntry {
    /// "anthropic" | "openai" | "gemini" | "ollama" | "lmstudio"
    #[serde(default = "default_provider")]
    pub provider: String,
```

and its field on `Config` (`types.rs:12`):

```rust
    /// Named model configurations.  At minimum a `[models.default]` entry should
    /// be present; it is used when no session-level override is active.
    #[serde(default = "default_models")]
    pub models: std::collections::HashMap<String, ModelEntry>,
```

**`Config` has a hand-written `Default` impl.** Adding a field to the struct
without adding it there is an `E0063 missing field` build error — measured at
drafting when the probe hit exactly that. `hosts` defaults to an empty map, so
no `[hosts]` section is required in anyone's config.

### The runbook frontmatter parser

`src/runbook.rs:85` is flat manual parsing with one helper per type. Adding a
list field is one line plus one struct field:

```rust
        let auto_approve_scripts = parse_list_field(frontmatter, "auto_approve_scripts");
        let ssh_target = parse_string_field(frontmatter, "ssh_target");
        let agent = parse_string_field(frontmatter, "agent");

        let ghost_config = GhostConfig {
            enabled,
            auto_approve_scripts,
            …
```

`GhostConfig` lives in `src/ipc.rs:81`, not in `runbook.rs`. Its existing
`ssh_target` field stays untouched — row 08 maps it onto `HostId`, not this
phase.

## Spec

### Task 1 — `src/host.rs`

New module. Two public types and nothing else:

```rust
/// The name of a configured host, or the implicit local host.
///
/// `Local` is implicit and always available — it is not a `[hosts]` entry
/// (design § 2.2). A `Named` id is a `[hosts.<name>]` key, validated by
/// `HostId::parse`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum HostId {
    Local,
    Named(String),
}
```

`HostId::parse(&str) -> Result<HostId, String>`:

- `"local"` (any case) → `HostId::Local`. The name is **reserved**; a
  `[hosts.local]` entry is a config error, because it could otherwise shadow
  the implicit host.
- Otherwise a name of `[A-Za-z0-9._-]+`, 1–64 chars → `HostId::Named`.
- Anything else → `Err` naming the offending value **and** the accepted shape.

`HostId::as_str()` returns `"local"` or the name, and `Display` matches it.

**Negative cases this must satisfy** — they are the point of `parse`:

- `parse("")` → `Err`. An empty host name must never become a `Named("")`.
- `parse("web01 prod")` → `Err`. Whitespace is not in the character class; a
  space would let one name look like two.
- `parse("../etc")` → `Err`. `/` is not in the class. This name reaches path
  and argv construction in later rows.
- `parse("LOCAL")` → `Ok(HostId::Local)`, not `Named("LOCAL")` — the reserved
  check is case-insensitive.

### Task 2 — `HostEntry` on `Config`

In `src/config/types.rs`, beside `ModelEntry`:

```rust
/// A remote execution target.  Define one or more named entries in
/// `config.toml` under `[hosts.<name>]`.  The local host is implicit and is
/// **not** configured here.
///
/// Example:
/// ```toml
/// [hosts.web01]
/// ssh = "matt@web01.lan"
/// tags = ["web", "prod"]
///
/// [hosts.db01]
/// ssh = "db01"
/// sudo_scripts = ["pg-failover"]
/// ```
#[derive(Debug, Deserialize, Serialize, Clone, Default)]
pub struct HostEntry {
    /// Any `ssh(1)` destination; `~/.ssh/config` is honoured, so a bare
    /// `Host` alias is valid. Required — an entry without it is a config
    /// error.
    #[serde(default)]
    pub ssh: String,
    /// Free-form labels for grouping. Not interpreted by the daemon.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Scripts that may be run under `sudo` on this host, the 1.x
    /// remote-sudoers model. Parsed here; enforced in a later phase.
    #[serde(default)]
    pub sudo_scripts: Vec<String>,
    /// M18 container profile to run this host's commands under, or `"none"`.
    /// Parsed here; **not enforced** — the container work meets the host
    /// model in a later milestone.
    #[serde(default)]
    pub sandbox_profile: Option<String>,
}
```

and the field on `Config`, **plus the matching line in the hand-written
`Default` impl** or the build fails with `E0063`:

```rust
    /// Named remote execution targets, keyed by host name.
    /// Empty by default — no `[hosts]` section is required.
    #[serde(default)]
    pub hosts: std::collections::HashMap<String, HostEntry>,
```

**The name is `HostEntry`, not `HostSpec`.** See § Current state; the milestone
README's `HostSpec` is superseded.

### Task 3 — validation

`Config::validate_hosts(&self) -> Vec<String>`, returning one human-readable
problem per bad entry, empty when clean. Follow `ExecutionConfig::validate`
(`types.rs:662`) for the house style: **warn, never panic, never return an
error that stops the daemon.**

Three problems to report, each naming the host:

1. An entry whose `ssh` is empty or whitespace.
2. A key that `HostId::parse` rejects, quoting `parse`'s message.
3. The reserved key `local` (in any case) — it shadows the implicit host.

Call it where `ExecutionConfig::validate` is already called, so the warnings
reach `daemon.log` at startup. **Do not** make a bad entry fatal: a typo in one
host must not stop the daemon.

### Task 4 — runbook `hosts:`

`GhostConfig` (`src/ipc.rs`) gains:

```rust
    /// Hosts this ghost may open shells on, by `[hosts.<name>]` key.
    /// Empty = unrestricted, preserving every existing runbook's behaviour.
    /// Parsed here; **enforced in a later phase**.
    #[serde(default)]
    pub hosts: Vec<String>,
```

and `runbook.rs`'s `parse_frontmatter` fills it with
`parse_list_field(frontmatter, "hosts")`, beside the existing
`auto_approve_scripts` line.

**Empty means unrestricted.** Every runbook in the wild has no `hosts:` line;
if empty meant "no hosts allowed", row 06 would later break all of them at
once. Say so in the doc comment.

### Task 5 — the config template and its gate

Add a commented `[hosts.<name>]` block to `assets/etc/config.toml` documenting
**every** `HostEntry` field. Follow the file's existing style: commented
sections with one `# key = value` per field.

Then extend `tests/doc_truth.rs`'s `seeded_config_template_has_no_phantom_keys`
so `[hosts.*]` is checked the way `[models.*]` is:

```rust
        let lookup = if section.starts_with("models") {
            "models"
        } else if section.starts_with("hosts") {
            "hosts"
        } else if section.contains('.') {
```

Without this, a typo'd key under `[hosts.web01]` is silently unchecked —
measured at drafting (§ Current state).

### Task 6 — tests

Name and behaviour, not placement.

In `src/host.rs`:
- `host_id_parses_a_named_host`
- `host_id_treats_local_as_reserved_and_case_insensitive` — `"local"`,
  `"Local"` and `"LOCAL"` all give `HostId::Local`.
- `host_id_rejects_empty_whitespace_and_path_characters` — the four negative
  cases in Task 1, each asserting `is_err()`, and the message naming the input.

In `src/config/`:
- `hosts_default_to_empty` — a config with no `[hosts]` section parses, and
  `hosts` is empty. Pins that the section is optional.
- `hosts_parse_from_toml` — the two-host example from Task 2's doc comment
  round-trips, including `tags` and `sudo_scripts`.
- `validate_hosts_reports_empty_ssh_and_reserved_local` — a config with one
  `ssh`-less entry and one `local` entry yields **two** problems, each naming
  its host. Assert on the host names appearing in the messages, not on exact
  wording.

In `src/runbook.rs`:
- `runbook_without_hosts_is_unrestricted` — no `hosts:` line yields an empty
  vec. **This is the negative that protects every existing runbook.**
- `runbook_hosts_frontmatter_parses` — `hosts: [web01, db01]` yields both.

### Task 7 — mutation pair

Run both, paste all four `test result:` lines into the Update Log **after** the
capture block.

**Mutation A — let the reserved name through.** In `HostId::parse`, delete the
`local` check so `"local"` becomes `Named("local")`.
`host_id_treats_local_as_reserved_and_case_insensitive` must go **red**.

**Mutation B — stop checking phantom keys under `[hosts.*]`.** Remove the
`hosts` branch Task 5 adds to `doc_truth.rs`, and add
`totally_made_up_key = "x"` under the template's `[hosts.<name>]` block.
`seeded_config_template_has_no_phantom_keys` must go **red**. Revert both.

**Mutation B is known to work**: the drafting probe ran exactly this shape and
confirmed that *without* the `hosts` branch the phantom key passes unnoticed.

### Task 8 — end-to-end capture

The artifact here is a config file the running binary loads, so verify against
one rather than against a fixture. Write a `[hosts.*]` section into a throwaway
`HOME`'s `etc/config.toml`, have the binary load it, and print the parse
result.

The cheapest honest route is the seeded template itself: `daemoneye setup`
seeds `etc/config.toml` from `assets/etc/config.toml` into a fresh `HOME`.
Capture one line, asserting each fact:

```
hosts_section_documented=true template_parses=true hosts_parsed=2 reserved_rejected=true
```

- `hosts_section_documented` — `[hosts.` appears in the seeded
  `etc/config.toml`, i.e. what a real user receives documents the section.
- `template_parses` — that seeded file loads through `Config` without error.
- `hosts_parsed` — with the two example hosts uncommented, `config.hosts.len()`
  is **2**.
- `reserved_rejected` — `HostId::parse("local")` gives `HostId::Local`, and a
  `[hosts.local]` entry is reported by `validate_hosts`.

Use a short `HOME` (`mktemp -d /tmp/de.XXXX`) as earlier phases do, and clean
up by matching the process by executable name if you start one.

Paste the captured block into an Update Log entry titled
`### Update — <date> (end-to-end verification)` **as that entry's first fenced
block**, then run:

```sh
D=docs/dev/milestones/M21-host-model/phase-02-host-config.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-02.txt
diff /tmp/pasted-02.txt /tmp/e2e-02.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

The literal `PASTE MATCH` line goes **into** the entry.

**If a fact cannot be captured, report it unmet.** Do not redefine a term
beneath its own line — that is what bounced the previous phase.

## Acceptance criteria

Patterns are **anchored** so they cannot prefix-match test names.

Each was run against HEAD `6fdfe57d` and **fails** there; "now" is the measured
current value.

- [ ] `ls src/host.rs` → exists (now absent)
- [ ] `grep -cE '^pub enum HostId' src/host.rs` → **1**
- [ ] `grep -cE '^    pub fn parse\(' src/host.rs` → **1**
- [ ] `grep -cE '^pub struct HostEntry' src/config/types.rs` → **1** (now 0) — **`HostEntry`, not `HostSpec`**; the name is what makes `doc_truth` see the section
- [ ] `grep -cE '^    pub hosts: std::collections::HashMap<String, HostEntry>,' src/config/types.rs` → **1** (now 0)
- [ ] `grep -cE '^    pub fn validate_hosts' src/config/types.rs` → **1** (now 0)
- [ ] `grep -cE '^    pub hosts: Vec<String>,' src/ipc.rs` → **1** (now 0)
- [ ] `grep -c 'parse_list_field(frontmatter, "hosts")' src/runbook.rs` → **1** (now 0)
- [ ] `grep -c '\[hosts\.' assets/etc/config.toml` → **at least 1** (now 0)
- [ ] `grep -c 'starts_with("hosts")' tests/doc_truth.rs` → **1** (now 0)
- [ ] `grep -c 'mod host' src/lib.rs` → **1** (now 0) — the module is wired in
- [ ] `cargo test --test doc_truth` reports **14 passed, unchanged** — this phase adds no doc_truth test, it extends one
- [ ] `cargo test --lib` reports **1906** (now 1898) — the eight Task 6 tests
- [ ] Task 8's `PASTE MATCH` block reads exactly `hosts_section_documented=true template_parses=true hosts_parsed=2 reserved_rejected=true`, **or** the Update Log states plainly which fact could not be captured and why
- [ ] The literal `PASTE MATCH` line appears in the end-to-end entry
- [ ] Task 7's mutation pair, all four `test result:` lines, in the Update Log
- [ ] `cargo fmt --all` clean, `cargo build` clean, `cargo clippy --all-targets --all-features -- -D warnings` clean

**Must-survive criteria** — these pass now and must still pass after:

- [ ] `grep -cE '^    pub ssh_target: Option<String>,' src/ipc.rs` → **1, unchanged** — `GhostConfig.ssh_target` stays; row 08 maps it onto `HostId`, not this phase
- [ ] `grep -cE '^pub fn wrap_remote|^    pub fn wrap_remote' src/daemon/policy.rs` → **1, unchanged**
- [ ] `git diff --stat 6fdfe57d..HEAD -- src/shell/ src/daemon/executor/` is **empty** — nothing executes in this phase
- [ ] `grep -cE '^pub struct ModelEntry' src/config/types.rs` → **1, unchanged**, and `cargo test --lib 'config'` shows no *existing* test name removed
- [ ] `grep -c '^        name: "' src/ai/tools/defs.rs` → **42, unchanged**; `grep -c 'deferred_group: None' src/ai/tools/defs.rs` → **28, unchanged** — no tool changes

## Test plan

**Added (8):** three in `src/host.rs`, three in `src/config/`, two in
`src/runbook.rs`, all named in Task 6. `cargo test --lib` therefore goes
1898 → **1906**.

**If you land on a different number, say so in the Update Log and state why —
do not pad to hit it.** A test added because the count demanded one is worse
than an honest note.

`doc_truth` stays at **14** — Task 5 extends an existing test, it does not add
one. **Removed: none.**

## End-to-end verification

The Task 8 capture **as the first fenced block** of an entry titled
`### Update — <date> (end-to-end verification)`, then the Task 7 mutation
pairs. Per `STANDARDS.md` §1 the server-authored `(complete)` entry does **not**
satisfy this.

## Authorizations

Edits are authorized in exactly these files:

- `src/host.rs` — new
- `src/lib.rs` — the `mod host;` line **only**
- `src/config/types.rs` — `HostEntry`, the `Config` field, the `Default` impl
  line, `validate_hosts`, and its call site; plus tests
- `src/ipc.rs` — the `GhostConfig.hosts` field **only**
- `src/runbook.rs` — the `hosts` parse line and tests **only**
- `assets/etc/config.toml` — the new `[hosts.<name>]` block **only**
- `tests/doc_truth.rs` — the `hosts` branch in
  `seeded_config_template_has_no_phantom_keys` **only**
- This phase doc's Update Log.

**Not authorized:** `src/shell/`, `src/daemon/`, `src/ai/`, `src/cli/`,
`CLAUDE.md`, `README.md`, and everything under `assets/` except
`etc/config.toml`.

**If the lint gate reports an orphan:** do **not** mask it with `#[allow]`, and
do **not** attribute a decision to anyone. Name it in the Update Log; if it is
in an authorized file, fix it and record that you did.

## Out of scope

- **Anything that executes.** No SSH spawn, no ControlMaster, no
  `var/run/ssh/`, no shell carrying a host — rows 03 and 04.
- **Enforcing `hosts:`.** The field is parsed here and enforced in row 06,
  where the approval classification lands. Empty stays unrestricted.
- **Enforcing `sandbox_profile` or `sudo_scripts`.** Both are parsed and
  inert; `sudo_scripts` meets the ghost path in row 08.
- **Mapping `GhostConfig.ssh_target` onto `HostId`** — row 08. It keeps its
  current meaning untouched here, pinned by a must-survive criterion.
- **`[HOSTS]` in the prompt context** — M23. This phase gives it something to
  render; it renders nothing.

## Update Log

<!-- entries appended below this line -->

### Update — 2026-09-20 02:20 (started)

**Executor:** executor (headless run).
Pre-flight confirmed at HEAD `6fdfe57d`... apart from the HEAD short-hash: the
tip is now `ddfd7dcc`, the drafting commit itself, with `6fdfe57d` its parent —
the tree is unchanged from what was probed. `src/host.rs` absent, no `pub
hosts` in `config/types.rs`, no `hosts` in `runbook.rs`; `cargo test --lib` →
1898 passed, 4 ignored; `--test doc_truth` → 14 passed. Status flipped to
`in-progress` here and in the milestone README; working through the spec in
task order.


### Update — 2026-09-20 02:55 (Mutation B, first attempt — red attempt badly designed, discarding)

**What happened:** ran the mutation as sketched (branch removed + phantom
template key) and got a **green** — 1 passed. The discard reason is in the log
above: `seeded_config_template_has_no_phantom_keys` has no self-check that
HostEntry reached its `known` map, so a `[hosts.*]` key skates through the
`section.contains('.') → continue` and the mutation is unobservable. The DAG
has no arrow to anything that would have caught this earlier. (Memory note:
the two low-signal risks that bit here were an untested cleanup and an
unverified assertion — both in the same block.)

**Doing instead:** re-run the mutation with two self-check assertions first
added to the test (`config_sections(ssh)` in `known`, `hosts` section
registered) — those must stay green logically, and give an unambiguous
(a) state. Then the mutation run is one run, one red, and the green revert.

```
mutation B, pre-check (branch removed, phantom key removed, asserts added):
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 13 filtered out

test result: ok. 14 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

(meta: original sketch's figures — not claimed as a real mutation run.)
