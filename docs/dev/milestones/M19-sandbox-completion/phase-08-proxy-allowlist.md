# Phase 08: The egress allowlist — rules, precedence, and closing the CONNECT port hole

**Milestone:** M19 — Sandbox Completion
**Status:** todo
**Depends on:** phase-07 (the per-job proxy and the filter file it mounts)
**Estimated diff:** ~330 lines including tests, across six files
**Tags:** language=rust, kind=feature, size=m

## Goal

Phase-07 mounts an **empty** filter, so a `network = "proxy"` job currently
reaches nothing. This phase renders the profile's `proxy_allow` / `proxy_deny`
into that file, so the allowlist actually governs egress — and closes a hole
the drafting measurements found: **without a `ConnectPort` directive tinyproxy
opens CONNECT to any port on an allowlisted host**, which makes the
milestone's "egress is HTTP(S) only" contract false as shipped.

**Scope note — this phase is narrower than the README's 08 intent line.** That
line bundles three things: the allowlist, the `events.jsonl` audit record, and
sentinel credential injection. Each is substantial, and all three together are
far past one executor session. This phase is the allowlist; audit and
credentials are now phases **13** and **14** in the milestone README, both
before the phase-10 close-out. The same narrowing happened to 06 for the same
reason and is recorded there.

## Architecture references

- `docs/design/agent-container-sandboxing.md` § "D5 — Network policy": *"the
  proxy enforces the per-profile hostname allowlist and logs every request to
  the event log."* This phase is the enforcement half; the logging half is
  phase-13.
- `docs/dev/milestones/M19-sandbox-completion/README.md` § Phases, **08
  intent**, decision 1 — *"Exact host, `*.domain` wildcard, `host:port`
  suffix; a `proxy_deny` list beside `proxy_allow`, and **deny always beats
  allow**. Do not invent a fourth form."* All four are implemented here; §
  Live measurements below records what `host:port` can and cannot mean.
- Same README, **06 intent**: *"egress is HTTP(S) only … raw TCP (`ssh`,
  `git@`), UDP and ICMP are not forwarded."* Task 3 is what makes that true.

## Pre-flight

1. Read `docs/dev/STANDARDS.md` top to bottom.
2. Read the architecture references above.
3. Read this entire phase doc before touching any file.
4. Confirm the repo is on a clean branch with no uncommitted changes.

## Current state

Measured on the tree at drafting time (2026-08-30, commit `0b13bb5`). **The
whole change was prototyped end-to-end before this doc was written, built,
linted, tested and mutated, then reverted** — every block in § Spec is that
prototype after `cargo fmt --all`, and every count in § Acceptance criteria
was read off it.

- `cargo test --lib` → **1499 passed; 0 failed; 4 ignored**. All four gates
  green.
- `SandboxProfile` has `network` and `proxy_allow`; **there is no
  `proxy_deny`** — `grep -c 'proxy_deny'` is **0** in
  `src/config/types.rs`, `assets/etc/config.toml`, `src/config/mod.rs` and
  `src/daemon/executor/container.rs` alike.
- Every new symbol is absent: `grep -c` on
  `src/daemon/executor/container.rs` for `enum ProxyRule`,
  `fn parse_proxy_rule(`, `fn is_subdomain_of(`, `fn deny_covers(`,
  `fn render_proxy_filter(` and `fn filter_for_profile(` → **0**, six for
  six. `grep -c 'filter_for_profile' src/daemon/background/run.rs` → **0**.
- `start_proxy` writes an empty file today. Its body opens:

  ```rust
      let filter = proxy_filter_path(job_id);
      if let Some(parent) = filter.parent()
          && let Err(e) = std::fs::create_dir_all(parent)
      {
          return Err(format!("sandbox egress filter directory failed: {e}"));
      }
      if let Err(e) = std::fs::write(&filter, b"") {
          return Err(format!("sandbox egress filter write failed: {e}"));
      }
  ```

  Task 4 gives it a `filter: &str` parameter and renames the local binding to
  `path`, because the two would otherwise collide.
- `grep -c '^ConnectPort' containers/proxy/tinyproxy.conf` → **0**.
- `grep -rc "allow(dead_code)" src/ | awk -F: '{s+=$2} END {print s}'` → **6**.

### Live measurements (architect, rootless Docker on the daemon host)

Run against real containers on 2026-08-30 with the committed proxy image and
removed afterwards (`docker ps -a --filter label=de.sandbox=1` → empty,
`docker network ls --filter label=de.sandbox=1` → empty). **These four facts
define the rule model; nothing in this phase spawns docker during tests.**

1. **A filter line matches the host, exactly.** Filter `example.com`:

   ```
     http://example.com/       200        https://example.com/       200
     http://www.example.com/   403        https://www.example.com/   000 (CONNECT refused)
   ```

2. **`*.domain` matches subdomains and NOT the domain itself.** Filter
   `*.example.com`:

   ```
     http://example.com/       403        https://example.com/       000
     http://www.example.com/   200        https://www.example.com/   200
   ```

   So allowing a domain *and* its subdomains takes **two** rules. This is why
   `render_proxy_filter` never rewrites one form into the other.

3. **A `host:port` filter line matches nothing at all.** Filter
   `example.com:80` — every one of the four requests above returned `403` or
   `000`, including plain `http://example.com/`. The filter is tested against
   the host alone, so a port simply cannot be enforced there. **This is why
   `parse_proxy_rule` accepts a `:80` / `:443` suffix and rejects every other
   port** rather than rendering the bare host, which would silently grant more
   than the operator asked for.

4. **The hole this phase closes.** With filter `example.com` and *no*
   `ConnectPort` directive, the proxy happily dialled arbitrary ports:

   ```
   CONNECT example.com:22   → INFO opensock: opening connection to example.com:22
   CONNECT example.com:25   → INFO opensock: opening connection to example.com:25
   CONNECT example.com:3306 → INFO opensock: opening connection to example.com:3306
   ```

   The `000` those requests return is a *connection timeout*, not a refusal —
   the proxy tried. An agent allowed one HTTPS host could therefore tunnel SSH
   or MySQL to it. With `ConnectPort 443` and `ConnectPort 563` added, the
   same probe produces **one** `opensock` line, for `:443`, and ordinary HTTPS
   still returns `200`:

   ```
     normal https (must still be 200): 200
     CONNECT example.com:22    000   (no opensock line — refused before dialling)
     CONNECT example.com:25    000
     CONNECT example.com:3306  000
   ```

## Gotchas

1. **Adding `proxy_deny` to `SandboxProfile` breaks three struct literals**,
   and only the compiler will tell you: the manual `impl Default` in
   `src/config/types.rs`, a test literal in `src/config/mod.rs`, and the
   `cfg_with_profile` helper in `container.rs`'s test module. Task 1 does all
   three. It also fails `tests/doc_truth.rs` until
   `assets/etc/config.toml` documents the field — same trap phase-06 hit with
   `proxy_image`. Four edits, one field.

2. **Never `git checkout` a file to restore a mutation.** The architect did
   exactly this while prototyping and destroyed ~300 uncommitted lines,
   because the phase's own work was not committed. Restore a mutation with the
   **inverse `patch`**, always. This is in your contract and it is in this
   Gotcha because it really happens.

3. **Rendering fails closed, in both directions.** An unparseable rule is
   dropped, never approximated; an allow list where nothing survives renders
   the **empty string**, which is deny-all (measured in phase-06). Do not add
   a "if the filter is empty, allow everything" convenience — it would invert
   the entire mechanism.

4. **A deny inside a wildcard allow drops the whole wildcard.** tinyproxy's
   filter is an allow list with no exception form, so `*.example.com` minus
   `secret.example.com` cannot be expressed. Dropping the wildcard is the only
   rendering that honours "deny beats allow" without leaving the denied host
   reachable. It is deliberately blunt; a test pins it, and a second assertion
   in the same test pins that a *sibling* deny leaves the wildcard alone.

5. **`*.d` never matches `d`** (§ Live measurement 2), so a deny of the apex
   does **not** drop a `*.apex` allow. That asymmetry is pinned by its own
   assertion; do not "fix" it into symmetry.

6. **`start_proxy`'s new `filter` parameter collides with its local
   binding.** Rename the local `let filter = proxy_filter_path(job_id)` to
   `path` and update its three uses, exactly as Task 4 shows. A partial rename
   compiles into the wrong thing in one place — the `-v` mount argument.

7. **Do not touch the audit record or credentials.** Both moved to phases 13
   and 14 (§ Goal). If you find yourself writing to `events.jsonl` or reading
   `docker logs`, stop — it is not this phase.

## Spec

### Task 1 — `proxy_deny`, in four places

**`src/config/types.rs`** — directly after the `proxy_allow` field of
`SandboxProfile`:

```rust
    /// Hostnames this profile may never reach, even when an allow rule would
    /// cover them. Deny always beats allow. Default: empty.
    #[serde(default)]
    pub proxy_deny: Vec<String>,
```

and in its `impl Default`, directly after `proxy_allow: Vec::new(),`:

```rust
            proxy_deny: Vec::new(),
```

**`src/config/mod.rs`** — the test literal that names every field gains the
same line, directly after its own `proxy_allow: Vec::new(),`:

```rust
                    proxy_deny: Vec::new(),
```

**`assets/etc/config.toml`** — replace the single line

```toml
# proxy_allow  = ["crates.io"]   # only consulted when network = "proxy"
```

with

```toml
# proxy_allow  = ["crates.io", "*.crates.io"]  # only consulted when network =
#                           # "proxy". Exact host, or "*.domain" for subdomains
#                           # (which does NOT include the domain itself); an
#                           # optional :80 / :443 suffix is allowed.
# proxy_deny   = []         # hosts this profile may never reach; deny always
#                           # beats allow, and a deny inside a wildcard allow
#                           # drops that wildcard
```

### Task 2 — The rule model, in `src/daemon/executor/container.rs`

Insert this block **directly before** the line

```rust
/// argv creating the job's egress network. `--internal` is the isolation
```

(the doc comment of `network_create_args`). Prototype verbatim, after
`cargo fmt --all`:

```rust
/// One parsed egress rule from `proxy_allow` / `proxy_deny`.
///
/// The variants are exactly what tinyproxy's filter can express, measured
/// 2026-08-30: a filter line is an fnmatch pattern tested against the **host
/// alone**, so `example.com` matches only `example.com`, `*.example.com`
/// matches its subdomains but **not** the apex, and a `host:port` line
/// matches nothing at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProxyRule {
    /// Exactly this host.
    Host(String),
    /// Every subdomain of this domain — not the domain itself.
    Subdomains(String),
    /// Cannot be enforced; carries the operator-facing reason.
    Unsupported(String),
}

/// Parse one rule. Fails closed: anything not certainly expressible becomes
/// [`ProxyRule::Unsupported`] and is dropped from the rendered filter rather
/// than approximated into a broader grant.
///
/// A `host:port` suffix is accepted for ports **80** and **443** only — the
/// two the proxy can actually reach (`ConnectPort 443`/`563` caps CONNECT,
/// measured). Any other port is unsupported: rendering just the host would
/// silently grant more than was asked for.
pub fn parse_proxy_rule(rule: &str) -> ProxyRule {
    let text = rule.trim();
    if text.is_empty() {
        return ProxyRule::Unsupported("empty rule".to_string());
    }
    if text.contains('/') || text.split_whitespace().count() > 1 {
        return ProxyRule::Unsupported(format!(
            "{text:?} is not a hostname — rules are hosts, not URLs"
        ));
    }
    let (host, port) = match text.rsplit_once(':') {
        Some((h, p)) => match p.parse::<u16>() {
            Ok(port) => (h, Some(port)),
            Err(_) => {
                return ProxyRule::Unsupported(format!("{text:?} has an unparseable port"));
            }
        },
        None => (text, None),
    };
    if let Some(port) = port
        && port != 80
        && port != 443
    {
        return ProxyRule::Unsupported(format!(
            "{text:?} names port {port}; only 80 and 443 are reachable through the proxy"
        ));
    }
    if host.is_empty() {
        return ProxyRule::Unsupported(format!("{text:?} has no host"));
    }
    if let Some(domain) = host.strip_prefix("*.") {
        if domain.is_empty() || domain.contains('*') {
            return ProxyRule::Unsupported(format!("{text:?} is not a usable wildcard"));
        }
        return ProxyRule::Subdomains(domain.to_string());
    }
    if host.contains('*') {
        return ProxyRule::Unsupported(format!(
            "{text:?} — the only wildcard form is a leading \"*.\""
        ));
    }
    ProxyRule::Host(host.to_string())
}

/// Is `host` a strict subdomain of `domain`?
fn is_subdomain_of(host: &str, domain: &str) -> bool {
    host.len() > domain.len() + 1 && host.ends_with(domain) && {
        let boundary = host.len() - domain.len() - 1;
        host.as_bytes()[boundary] == b'.'
    }
}

/// Does `deny` forbid anything `allow` would grant?
///
/// A deny that lands **inside** a wildcard allow returns true, which drops the
/// whole wildcard. tinyproxy's filter is an allow list with no exception form,
/// so a narrower grant cannot be expressed — losing the wildcard is the only
/// way "deny beats allow" can be honoured without leaking the denied host.
fn deny_covers(deny: &ProxyRule, allow: &ProxyRule) -> bool {
    match (deny, allow) {
        (ProxyRule::Host(d), ProxyRule::Host(a)) => d == a,
        (ProxyRule::Host(d), ProxyRule::Subdomains(a)) => is_subdomain_of(d, a),
        (ProxyRule::Subdomains(d), ProxyRule::Host(a)) => is_subdomain_of(a, d),
        (ProxyRule::Subdomains(d), ProxyRule::Subdomains(a)) => a == d || is_subdomain_of(a, d),
        _ => false,
    }
}

/// Render a profile's rules into the file mounted at `/etc/tinyproxy/filter`.
///
/// One fnmatch pattern per line, in the order the allow list gave them, with
/// duplicates removed. An empty result is **deny-all**, which is what an empty
/// `proxy_allow`, an all-unsupported list, and a fully-denied list each
/// correctly produce.
pub fn render_proxy_filter(allow: &[String], deny: &[String]) -> String {
    let denials: Vec<ProxyRule> = deny.iter().map(|r| parse_proxy_rule(r)).collect();
    for rule in &denials {
        if let ProxyRule::Unsupported(why) = rule {
            log::warn!("sandbox egress deny rule ignored: {why}");
        }
    }
    let mut lines: Vec<String> = Vec::new();
    for rule in allow.iter().map(|r| parse_proxy_rule(r)) {
        let line = match &rule {
            ProxyRule::Host(h) => h.clone(),
            ProxyRule::Subdomains(d) => format!("*.{d}"),
            ProxyRule::Unsupported(why) => {
                log::warn!("sandbox egress allow rule ignored: {why}");
                continue;
            }
        };
        if denials.iter().any(|d| deny_covers(d, &rule)) {
            log::warn!("sandbox egress allow rule {line:?} dropped: a deny rule covers it");
            continue;
        }
        if !lines.contains(&line) {
            lines.push(line);
        }
    }
    if lines.is_empty() {
        return String::new();
    }
    format!("{}\n", lines.join("\n"))
}

/// The filter text for the profile `name`, or deny-all when it has none.
pub fn filter_for_profile(cfg: &SandboxConfig, name: Option<&str>) -> String {
    match name.and_then(|n| cfg.profile.get(n)) {
        Some(p) => render_proxy_filter(&p.proxy_allow, &p.proxy_deny),
        None => String::new(),
    }
}

```

### Task 3 — Cap CONNECT, in `containers/proxy/tinyproxy.conf`

Insert two lines directly **before** `MaxClients 20`:

```
ConnectPort 443
ConnectPort 563
```

Nothing else in the file changes. (563 is NNTPS, the second port tinyproxy's
own documentation pairs with 443; it costs nothing and matches the
convention.) The file is `include_str!`d by two tests, so this rebuilds the
test binary — expected, not a stale cache.

### Task 4 — `start_proxy` writes the rendered filter, same file as Task 2

Change the signature to take the filter text:

```rust
pub fn start_proxy(
    cfg: &SandboxConfig,
    job_id: &str,
    is_ghost: bool,
    session_id: Option<&str>,
    filter: &str,
) -> Result<(), String> {
```

Then, in the body, rename the local path binding and write the text — the
three edits are `let filter =` → `let path =`, `filter.parent()` →
`path.parent()`, and the write itself:

```rust
    let path = proxy_filter_path(job_id);
    if let Some(parent) = path.parent()
        && let Err(e) = std::fs::create_dir_all(parent)
    {
        return Err(format!("sandbox egress filter directory failed: {e}"));
    }
    if let Err(e) = std::fs::write(&path, filter.as_bytes()) {
        return Err(format!("sandbox egress filter write failed: {e}"));
    }
```

and further down, the `proxy_run_args` call takes `&path`:

```rust
        proxy_run_args(cfg, job_id, &path, is_ghost, session_id),
```

### Task 5 — The caller renders it, in `src/daemon/background/run.rs`

**Replace** these three lines — the opening of the proxy-startup block, which
occurs once:

```rust
        let (cfg_p, job_p, sid_p) = (config.sandbox.clone(), job_id.clone(), session_id.clone());
        let started = tokio::task::spawn_blocking(move || {
            crate::daemon::executor::container::start_proxy(
```

with:

```rust
        let filter = crate::daemon::executor::container::filter_for_profile(
            &config.sandbox,
            profile_name.as_deref(),
        );
        let (cfg_p, job_p, sid_p) = (config.sandbox.clone(), job_id.clone(), session_id.clone());
        let started = tokio::task::spawn_blocking(move || {
            crate::daemon::executor::container::start_proxy(
```

and add the new argument to that call, after `sid_p.as_deref(),`:

```rust
                &filter,
```

### Task 6 — Tests, appended to `container.rs`'s existing `mod tests`

Eight tests, appended at the end of the module. Every name begins
`sandbox_filter_`. Prototype verbatim, after `cargo fmt --all`:

```rust
    fn sandbox_filter_parses_the_three_supported_rule_forms() {
        assert_eq!(
            parse_proxy_rule("example.com"),
            ProxyRule::Host("example.com".to_string())
        );
        assert_eq!(
            parse_proxy_rule("  example.com  "),
            ProxyRule::Host("example.com".to_string()),
            "surrounding whitespace is trimmed, not rejected"
        );
        assert_eq!(
            parse_proxy_rule("*.example.com"),
            ProxyRule::Subdomains("example.com".to_string())
        );
        assert_eq!(
            parse_proxy_rule("example.com:443"),
            ProxyRule::Host("example.com".to_string())
        );
        assert_eq!(
            parse_proxy_rule("*.example.com:80"),
            ProxyRule::Subdomains("example.com".to_string())
        );
    }

    #[test]
    fn sandbox_filter_refuses_every_rule_it_cannot_enforce() {
        // Each of these would otherwise be approximated into a *broader*
        // grant than the operator wrote. Measured 2026-08-30: a tinyproxy
        // filter line matches the host alone, so a port cannot be enforced
        // there, and `ConnectPort 443`/`563` is what caps CONNECT.
        for bad in [
            "",
            "   ",
            "https://example.com/",
            "example.com/path",
            "example.com example.org",
            "example.com:22",
            "example.com:8443",
            "example.com:notaport",
            "*",
            "*.",
            "ex*ple.com",
            ":443",
        ] {
            assert!(
                matches!(parse_proxy_rule(bad), ProxyRule::Unsupported(_)),
                "{bad:?} must not parse into a usable rule, got {:?}",
                parse_proxy_rule(bad)
            );
        }
    }

    #[test]
    fn sandbox_filter_renders_one_pattern_per_line_in_order() {
        let out = render_proxy_filter(
            &[
                "crates.io".to_string(),
                "*.crates.io".to_string(),
                "crates.io".to_string(),
                "docs.rs:443".to_string(),
            ],
            &[],
        );
        assert_eq!(out, "crates.io\n*.crates.io\ndocs.rs\n", "{out:?}");
    }

    #[test]
    fn sandbox_filter_deny_beats_an_exactly_matching_allow() {
        let out = render_proxy_filter(
            &["a.example.com".to_string(), "b.example.com".to_string()],
            &["a.example.com".to_string()],
        );
        assert_eq!(out, "b.example.com\n", "{out:?}");
    }

    #[test]
    fn sandbox_filter_a_deny_inside_a_wildcard_drops_the_whole_wildcard() {
        // tinyproxy's filter is an allow list with no exception form, so the
        // narrower grant cannot be expressed. Dropping the wildcard is the
        // only rendering that does not leak the denied host.
        let out = render_proxy_filter(
            &["*.example.com".to_string(), "other.org".to_string()],
            &["secret.example.com".to_string()],
        );
        assert_eq!(out, "other.org\n", "{out:?}");
        assert!(
            !out.contains("example.com"),
            "the denied host must not remain reachable through the wildcard: {out:?}"
        );
        // A deny that is merely a *sibling* of the wildcard leaves it intact.
        let unrelated = render_proxy_filter(
            &["*.example.com".to_string()],
            &["secret.example.org".to_string()],
        );
        assert_eq!(unrelated, "*.example.com\n", "{unrelated:?}");
        // The apex is not inside its own wildcard — `*.d` never matches `d`.
        let apex =
            render_proxy_filter(&["*.example.com".to_string()], &["example.com".to_string()]);
        assert_eq!(apex, "*.example.com\n", "{apex:?}");
    }

    #[test]
    fn sandbox_filter_denies_everything_when_nothing_survives() {
        // An empty filter file is deny-all (measured), so each of these is a
        // profile that can reach nothing — never an open door.
        assert_eq!(render_proxy_filter(&[], &[]), "");
        assert_eq!(
            render_proxy_filter(&["example.com:22".to_string()], &[]),
            "",
            "an all-unsupported allow list must not fall back to permitting anything"
        );
        assert_eq!(
            render_proxy_filter(&["example.com".to_string()], &["example.com".to_string()]),
            ""
        );
        assert_eq!(
            render_proxy_filter(
                &["*.example.com".to_string()],
                &["*.example.com".to_string()]
            ),
            ""
        );
    }

    #[test]
    fn sandbox_filter_for_an_unknown_profile_is_deny_all() {
        let mut cfg = cfg_with_profile("proxy");
        cfg.profile
            .get_mut("researcher")
            .expect("seeded above")
            .proxy_deny = vec!["bad.example.com".to_string()];
        assert_eq!(
            filter_for_profile(&cfg, Some("researcher")),
            "example.com\n"
        );
        assert_eq!(
            filter_for_profile(&cfg, Some("analyst")),
            "",
            "a profile with no config entry reaches nothing"
        );
        assert_eq!(filter_for_profile(&cfg, None), "");
    }

    #[test]
    fn sandbox_filter_conf_caps_connect_to_tls_ports() {
        // Without these two lines tinyproxy opens CONNECT to *any* port on an
        // allowlisted host — measured 2026-08-30, it dialled example.com:22,
        // :25 and :3306. That would make the milestone's "HTTP(S) only"
        // contract false.
        let conf = include_str!("../../../containers/proxy/tinyproxy.conf");
        assert!(
            conf.lines().any(|l| l.trim() == "ConnectPort 443"),
            "{conf}"
        );
        assert!(
            conf.lines().any(|l| l.trim() == "ConnectPort 563"),
            "{conf}"
        );
        assert!(
            !conf.lines().any(|l| l.trim() == "ConnectPort 22"),
            "{conf}"
        );
    }
```

### Task 7 — Mutation pair M1: a deny inside a wildcard really drops it

Mutation edits go through your `patch` tool — **`sed -i`, `perl -i` and `>`
redirects into a source file are banned by your contract and `bash` will
refuse them. Restore with the inverse `patch`, never with `git checkout`**
(§ Gotchas 2). Append each marker and run to `/tmp/e2e-08.txt`. Run the gates
(§ End-to-end verification) only **after** all three pairs are restored.

1. **Apply.** `patch` `src/daemon/executor/container.rs`:
   - `old_str`: `        (ProxyRule::Host(d), ProxyRule::Subdomains(a)) => is_subdomain_of(d, a),`
   - `new_str`: `        (ProxyRule::Host(_), ProxyRule::Subdomains(_)) => false,`

   Then:
   ```sh
   echo "== M1 APPLIED ==" >> /tmp/e2e-08.txt
   cargo test --lib sandbox_filter 2>&1 | grep -E "FAILED|^test result:" | sed 's/; finished in .*//' >> /tmp/e2e-08.txt
   grep -c 'is_subdomain_of(d, a),' src/daemon/executor/container.rs >> /tmp/e2e-08.txt
   ```
   Measured on the prototype: **exactly 1 failed**, naming
   `sandbox_filter_a_deny_inside_a_wildcard_drops_the_whole_wildcard`, and the
   `grep -c` prints `0`. A green suite here means a denied host stays
   reachable through the wildcard — record a blocker.

2. **Restore.** The inverse `patch`, marker `== M1 RESTORED ==`, the same two
   commands. `8 passed` and the `grep -c` prints `1`.

### Task 8 — Mutation pair M2: unenforceable ports are refused, not approximated

Only after M1 is restored.

1. **Apply.** `patch` `src/daemon/executor/container.rs`, deleting the port
   gate — `old_str`:

   ```
       if let Some(port) = port
           && port != 80
           && port != 443
       {
           return ProxyRule::Unsupported(format!(
               "{text:?} names port {port}; only 80 and 443 are reachable through the proxy"
           ));
       }
   ```

   and `new_str` is the **empty string** (delete the block).

   Then, with the marker `== M2 APPLIED ==`, the same `cargo test` line and:
   ```sh
   grep -c 'only 80 and 443 are reachable through the proxy' src/daemon/executor/container.rs >> /tmp/e2e-08.txt
   ```
   Measured: **exactly 2 failed** — `sandbox_filter_refuses_every_rule_it_cannot_enforce`
   **and** `sandbox_filter_denies_everything_when_nothing_survives` — and the
   `grep -c` prints `0`. **Two, not one**: this mutation is the only one in
   the phase that trips a second test, because `example.com:22` then renders a
   line instead of nothing. If you see one failure, the patch landed
   somewhere else — record a blocker.

2. **Restore.** Re-insert the block directly before `    if host.is_empty() {`,
   marker `== M2 RESTORED ==`, the same two commands. `8 passed` and the
   `grep -c` prints `1`.

### Task 9 — Mutation pair M3: the CONNECT cap is really in the image

Only after M2 is restored. A non-Rust file; `patch` works the same way.

1. **Apply.** `patch` `containers/proxy/tinyproxy.conf`:
   - `old_str`:
     ```
     ConnectPort 443
     ConnectPort 563
     MaxClients 20
     ```
   - `new_str`: `MaxClients 20`

   Then, with the marker `== M3 APPLIED ==`, the same `cargo test` line and:
   ```sh
   grep -c '^ConnectPort 443$' containers/proxy/tinyproxy.conf >> /tmp/e2e-08.txt
   ```
   Measured: **exactly 1 failed**, naming
   `sandbox_filter_conf_caps_connect_to_tls_ports`, and the `grep -c` prints
   `0`.

2. **Restore.** The inverse `patch`, marker `== M3 RESTORED ==`, the same two
   commands. `8 passed` and the `grep -c` prints `1`.

The `grep -c` after **each** direction is not optional: a `patch` whose
`old_str` matches the wrong line fails silently, and a mutation that never
applied certifies a vacuous guard. **All three failure counts above were
measured, not estimated.**

### Task 10 — Capture the end-to-end evidence

**The § End-to-end block appends (`>> /tmp/e2e-08.txt`). If you need to run it
a second time — for any reason — `rm -f /tmp/e2e-08.txt` first and run the
whole sequence again from Task 7.** Two executions otherwise leave two copies
in the file, the paste holds one, and the self-check prints `PASTE MISMATCH`.
**Never edit `/tmp/e2e-08.txt` or the pasted block to reconcile them.** Run
`cargo fmt --all` **before** the block so `fmt_exit` is a real `0`.

Every `test result:` line is piped through `sed 's/; finished in .*//'` so
per-run timings cannot cause a spurious mismatch. Do not add the suffix back.

Run the block in § End-to-end verification **verbatim and unmodified**, then
paste the resulting `/tmp/e2e-08.txt` into a new Update Log entry headed
`### Update — <date> (end-to-end verification)`. **The entry ends with the
self-check's verdict line, `PASTE MATCH`, bare on its own line after the
fenced block.**

## Acceptance criteria

**Every count below was read off the architect's prototype of this exact
change, not derived from the spec text.**

- [ ] `grep -c 'proxy_deny'` prints **2** in `src/config/types.rs` (field +
      `Default`), **1** in `assets/etc/config.toml`, **1** in
      `src/config/mod.rs`, and **4** in `src/daemon/executor/container.rs`
      (**before: 0, 0, 0, 0**).
- [ ] Each of `enum ProxyRule`, `fn parse_proxy_rule(`, `fn is_subdomain_of(`,
      `fn deny_covers(`, `fn render_proxy_filter(` and `fn filter_for_profile(`
      appears exactly **1** time in `src/daemon/executor/container.rs`
      (**before: 0** for all six).
- [ ] `grep -c 'is_subdomain_of(d, a),' src/daemon/executor/container.rs`
      prints `1` and
      `grep -c 'only 80 and 443 are reachable through the proxy' …` prints `1`
      (**before: 0, 0**) — the two seams M1 and M2 mutate.
- [ ] `grep -c 'filter: &str,' src/daemon/executor/container.rs` prints `1`
      and `grep -c 'std::fs::write(&path, filter.as_bytes())' …` prints `1`
      (**before: 0, 0**) — `start_proxy` writes the rendered text, not `b""`.
- [ ] `grep -c '^ConnectPort' containers/proxy/tinyproxy.conf` prints `2`
      (**before: 0**).
- [ ] `grep -c 'filter_for_profile' src/daemon/background/run.rs` prints `1`
      (**before: 0**).
- [ ] `cargo test --lib sandbox_filter 2>&1 | grep -c "^test .* ok$"` prints
      `8`. A count, not an exit status.
- [ ] `cargo test --lib` reports **1507** passing and `0 failed`
      (**before: 1499**), with `4 ignored` unchanged; and **`cargo test`
      (all targets)** is green — `tests/doc_truth.rs` is the one that checks
      the seeded config (§ Gotchas 1).
- [ ] `grep -rc "allow(dead_code)" src/ | awk -F: '{s+=$2} END {print s}'`
      prints `6` (**unchanged**).
- [ ] `sed -n '1,/^#\[cfg(test)\]/p' src/daemon/executor/container.rs | grep -c '\.unwrap()\|\.expect('`
      prints `0` (**before: 0**).
- [ ] `git diff --name-only | wc -l` prints `7` — the six files this phase
      edits plus this phase doc, whose Update Log § Task 10 requires you to
      write. (The count includes this doc by construction: the evidence entry
      exists before the block that counts files runs.)
- [ ] The § End-to-end entry shows `== M1 APPLIED ==` and `== M3 APPLIED ==`
      each failing **exactly one** named test, `== M2 APPLIED ==` failing
      **exactly two**, all three `RESTORED` runs passing, with a `grep -c`
      line after each direction reading the value that task states.
- [ ] No new `#[allow(...)]` anywhere, no `unsafe`, no `TODO`.
- [ ] All four gates green: `cargo fmt --all`, `cargo build`,
      `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`.
- [ ] The § End-to-end entry contains the literal line `PASTE MATCH` (bare,
      with no surrounding backticks):
      `grep -c '^PASTE MATCH$' docs/dev/milestones/M19-sandbox-completion/phase-08-proxy-allowlist.md`
      prints `1`.

## Test plan

Eight unit tests in `container.rs`'s `mod tests`, given in full in Task 6.
No new test file. The only existing-test edits are the two struct literals
Task 1 names, which the compiler forces.

**The negative cases are the phase.**
`sandbox_filter_refuses_every_rule_it_cannot_enforce` walks twelve malformed
or unenforceable rules — URLs, paths, embedded spaces, `example.com:22`,
`example.com:8443`, a bare `*`, a mid-label `*` — because each one, if
approximated into "just use the host", is a **broader** grant than the
operator wrote; M2 proves the port gate is live.
`sandbox_filter_denies_everything_when_nothing_survives` pins that an empty,
all-unsupported, or fully-denied list renders the empty string, which is
deny-all — the direction a mistake must fall.
`sandbox_filter_a_deny_inside_a_wildcard_drops_the_whole_wildcard` pins the
precedence rule *and* its two boundaries (a sibling deny leaves the wildcard;
an apex deny does not drop `*.apex`, because `*.d` never matches `d`); M1
proves it. `sandbox_filter_conf_caps_connect_to_tls_ports` pins the two lines
that make "HTTP(S) only" true, through `include_str!` so the pin is on the
real file; M3 proves it.

`start_proxy` still spawns docker and is **not** unit-tested, matching the
rest of the module; its new parameter is exercised through
`filter_for_profile`, which is pure. **If an existing test other than the two
struct literals requires a change to pass, stop and record a blocker.**

## End-to-end verification

Run this block verbatim from the repo root, **after** Tasks 7, 8 and 9 have
appended their mutation markers to `/tmp/e2e-08.txt` and all three pairs are
restored.

```sh
{
echo "== A. named tests (expect 8 ok) =="
cargo test --lib sandbox_filter 2>&1 | grep -E "^test |^test result:" | sed 's/; finished in .*//'; echo "cargo_exit=${PIPESTATUS[0]}"
echo "== B. full suite, all targets =="
cargo test 2>&1 | grep -E "^test result:" | sed 's/; finished in .*//'; echo "cargo_exit=${PIPESTATUS[0]}"
echo "== C. gates =="
cargo fmt --all -- --check > /dev/null 2>&1; echo "fmt_exit=$?"
cargo clippy --all-targets --all-features -- -D warnings > /dev/null 2>&1; echo "clippy_exit=$?"
echo "== D. structural greps =="
C=src/daemon/executor/container.rs
for f in parse_proxy_rule is_subdomain_of deny_covers render_proxy_filter filter_for_profile; do
  echo -n "fn $f (1): "; grep -c "fn $f(" "$C"
done
echo -n "enum ProxyRule (1):             "; grep -c 'enum ProxyRule' "$C"
echo -n "M1 seam (1):                    "; grep -c 'is_subdomain_of(d, a),' "$C"
echo -n "M2 seam (1):                    "; grep -c 'only 80 and 443 are reachable through the proxy' "$C"
echo -n "start_proxy filter param (1):   "; grep -c 'filter: &str,' "$C"
echo -n "writes rendered filter (1):     "; grep -c 'std::fs::write(&path, filter.as_bytes())' "$C"
echo -n "proxy_deny container.rs (4):    "; grep -c 'proxy_deny' "$C"
echo -n "proxy_deny types.rs (2):        "; grep -c 'proxy_deny' src/config/types.rs
echo -n "proxy_deny config.toml (1):     "; grep -c 'proxy_deny' assets/etc/config.toml
echo -n "proxy_deny mod.rs (1):          "; grep -c 'proxy_deny' src/config/mod.rs
echo -n "ConnectPort conf (2):           "; grep -c '^ConnectPort' containers/proxy/tinyproxy.conf
echo -n "run.rs filter_for_profile (1):  "; grep -c 'filter_for_profile' src/daemon/background/run.rs
echo -n "allow total (6):                "; grep -rc "allow(dead_code)" src/ | awk -F: '{s+=$2} END {print s}'
echo -n "prod unwrap container.rs (0):   "; sed -n '1,/^#\[cfg(test)\]/p' "$C" | grep -c '\.unwrap()\|\.expect('
echo -n "files changed (7):              "; git diff --name-only | wc -l
} >> /tmp/e2e-08.txt 2>&1
cat /tmp/e2e-08.txt
```

Paste the whole of `/tmp/e2e-08.txt` — mutation markers included — into your
Update Log entry as a fenced block, then run the self-check and paste its
verdict line into the same entry **bare, on its own line, with no backticks**:

```sh
D=docs/dev/milestones/M19-sandbox-completion/phase-08-proxy-allowlist.md
L=$(grep -n '^### Update.*end-to-end verification' "$D" | tail -1 | cut -d: -f1)
tail -n +"$L" "$D" | awk '/^```/{c++; next} c==1{print} c==2{exit}' > /tmp/pasted-08.txt
diff /tmp/pasted-08.txt /tmp/e2e-08.txt && echo "PASTE MATCH" || echo "PASTE MISMATCH"
```

**Run the block exactly as written.** If a label in it has gone stale against
the criteria, that is a spec defect — record a blocker naming it rather than
editing the block.

## Authorizations

- Edit **only** these six files: `src/config/types.rs`, `src/config/mod.rs`,
  `assets/etc/config.toml`, `containers/proxy/tinyproxy.conf`,
  `src/daemon/executor/container.rs`, `src/daemon/background/run.rs` — plus
  this phase doc's Update Log. No other file, no other doc.
- Run `cargo fmt --all`, `cargo build`,
  `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`.
- No `#[allow(...)]` may be added or removed, and no `#[ignore]` may be added
  or removed.
- **Do not change any existing test's assertions.** The two struct literals in
  Task 1 gain a field; that is the only permitted test edit.
- **Do not run `docker`, `podman`, or any container command** — including
  `daemoneye sandbox build` — and do not start, stop or query a system
  service. Every proxy behaviour this phase depends on was measured by the
  architect (§ Live measurements) and is re-verified at milestone close.
- Mutation edits go through `patch`. **Never `git checkout` a file to restore
  it** — it discards this round's own uncommitted work (§ Gotchas 2).
- **Append to the Update Log; never edit or delete an existing entry.** When
  flipping this doc's `Status:` line, change **only** that line — the line
  above it is `**Milestone:** M19 — Sandbox Completion` and must survive (a
  mis-anchored status patch ate it in phase-03; see `bugs/bug-phase-03-1.md`).
  After the flip, `grep -c '^\*\*Status:\*\*' <this doc>` must print `1` and
  `grep -c '^\*\*Milestone:\*\*' <this doc>` must print `1`.
- **Never edit `/tmp/e2e-08.txt` or the pasted evidence block after capture,
  for any reason** (Task 10). On a `PASTE MISMATCH`, delete the artifact and
  re-run the sequence; if a mismatch survives a clean re-run, record a
  blocker. This is `bugs/bug-phase-04-1.md`.
- **If you cannot finish honestly — an acceptance criterion is unsatisfiable, a
  mutation leaves the suite green or fails a different number of tests than the
  spec states, *or* a gate is red for a reason this phase did not cause —
  record a blocker Update Log entry naming the exact criterion, and stop.
  Reporting the blocker *is* the successful outcome.**
- **Record what you decide, not what you wish had been decided.** Every claim
  in your completion summary must be one the reviewer can re-run as a command
  from this doc. **Do not describe a criterion as met without reading its
  output, and if a pasted number disagrees with the value the criterion
  states, say so in your summary rather than reporting overall conformance** —
  phase-07's evidence contained exactly such a mismatch and its summary said
  "nothing deviated".

## Out of scope

- **The `events.jsonl` audit record** — destination host, matched rule,
  decision, `proxy_type`, repeat count. **Phase 13.** Do not write any event
  here.
- **Sentinel credential injection** — `[sandbox.profile.<name>.credentials]`,
  the `de-cred-<rand>` sentinel and the proxy-side header rewrite.
  **Phase 14.**
- **Config validation for the new rule forms** — `SandboxConfig::validate`
  keeps its existing two warnings and gains none. `render_proxy_filter`
  already logs a warning per ignored rule at the moment it matters, which is
  job start rather than daemon start.
- **Raw TCP / SSH egress** — deferred past M19. Task 3's `ConnectPort` is what
  makes the HTTP(S)-only contract true today; widening it is that later
  phase's decision, not a regression to fix here.
- **Re-reading the filter without restarting the proxy.** Each job gets its
  own proxy started after its filter is written (phase-07), so a reload path
  has no caller in M19.
- **`respawn.rs` / foreground / remote execution** — unchanged, as in
  phase-07.
- `CLAUDE.md`, `README.md`, the design doc — the phase-10 doc sweep.

## Update Log

<!-- entries appended below this line -->
