//! Repo-hygiene gates over the project's prose documentation. Three kinds:
//!
//! - **Retired claims** — strings that stopped being true and must not come
//!   back (originally the memory-index claims that FTS5 search invalidated).
//! - **Required claims** — facts that must stay documented, checked against the
//!   *durable* part of each doc so a mention surviving only in a transient
//!   milestone section does not count.
//! - **Structural** — `CLAUDE.md`'s AI-tools table is cross-referenced against
//!   the real `TOOLS` table, so it cannot silently fall behind the code.

use std::path::Path;

/// (doc path relative to the repo root, forbidden substring, why it is wrong)
const RETIRED_CLAIMS: &[(&str, &str, &str)] = &[
    (
        "docs/architecture.md",
        "grep fallback",
        "there is no grep fallback for recall; src/search.rs backs search_repository",
    ),
    (
        "docs/architecture.md",
        "currently a **stub**",
        "src/memory/index.rs is a real FTS5 index",
    ),
    (
        "CLAUDE.md",
        "grep scan in `src/search.rs`. Un-stubbing",
        "the index is no longer a stub",
    ),
    (
        "CLAUDE.md",
        "returns an empty `Vec`",
        "fts5_search returns BM25-ranked hits",
    ),
    (
        "README.md",
        "grep fallback",
        "there is no grep fallback for recall; fts5_search returns BM25-ranked hits",
    ),
    (
        "README.md",
        "migrate.rs",
        "src/memory/migrate.rs does not exist; the module is index.rs / review.rs / tags.rs",
    ),
];

/// (doc path, required substring, why it must be documented)
///
/// Checked against the **durable** part of each doc: for `docs/architecture.md`
/// everything before the milestone roadmap, because that section is rewritten
/// every milestone and a claim living only there disappears on the next close.
const REQUIRED_CLAIMS: &[(&str, &str, &str)] = &[
    (
        "CLAUDE.md",
        "daemoneye reindex",
        "the operator entry point to reconcile_index() must stay documented",
    ),
    (
        "docs/architecture.md",
        "daemoneye reindex",
        "the operator entry point to reconcile_index() must stay documented",
    ),
    (
        "README.md",
        "daemoneye reindex",
        "the operator entry point to reconcile_index() must stay documented",
    ),
    (
        "README.md",
        "daemoneye audit-prompts",
        "audit-prompts is a shipped subcommand and went undocumented for four milestones",
    ),
];

/// The heading that opens the AI-tools table in `CLAUDE.md`.
const TOOLS_HEADING: &str = "### Current AI tools";

/// The heading that begins the transient part of `docs/architecture.md`.
const ROADMAP_HEADING: &str = "## 5. Milestone roadmap";

fn durable_part(doc: &str, text: &str) -> String {
    if doc == "docs/architecture.md" {
        match text.find(ROADMAP_HEADING) {
            Some(i) => text[..i].to_string(),
            None => panic!("{doc} no longer contains {ROADMAP_HEADING:?}"),
        }
    } else {
        text.to_string()
    }
}

#[test]
fn docs_document_the_reindex_command() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut missing = Vec::new();
    for (doc, phrase, why) in REQUIRED_CLAIMS {
        let path = root.join(doc);
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
        if !durable_part(doc, &text).contains(phrase) {
            missing.push(format!("{doc}: missing {phrase:?} — {why}"));
        }
    }
    assert!(
        missing.is_empty(),
        "docs no longer document these:\n{}",
        missing.join("\n")
    );
}

#[test]
fn docs_do_not_carry_retired_index_claims() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut findings = Vec::new();
    for (doc, phrase, why) in RETIRED_CLAIMS {
        let path = root.join(doc);
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
        if text.contains(phrase) {
            findings.push(format!("{doc}: retired claim {phrase:?} — {why}"));
        }
    }
    assert!(
        findings.is_empty(),
        "retired index claims are back in the docs:\n{}",
        findings.join("\n")
    );
}

// ---------------------------------------------------------------------------
// CLAUDE.md § "Current AI tools" must mirror the real `TOOLS` table
// ---------------------------------------------------------------------------
//
// This drifted twice: the table sat nine tools behind the code, and it grouped
// `write_script / read_script / …` into one row when the write side is core and
// the read side is deferred — a shape that cannot express the truth.
//
// The check links against `daemoneye::ai::tools::TOOLS` rather than parsing
// `defs.rs`, so the code side of the comparison cannot itself go stale.

/// The section of `CLAUDE.md` between the tools heading and the next `## `.
fn tools_section(text: &str) -> &str {
    let start = text
        .find(TOOLS_HEADING)
        .unwrap_or_else(|| panic!("CLAUDE.md no longer contains {TOOLS_HEADING:?}"));
    let rest = &text[start + TOOLS_HEADING.len()..];
    match rest.find("\n## ") {
        Some(i) => &rest[..i],
        None => rest,
    }
}

/// `(tool name, Loaded cell)` for every table row. The Loaded cell is unwrapped
/// from any `**bold**` emphasis so `**agents**` and `core` compare alike.
fn documented_tools(section: &str) -> Vec<(String, String)> {
    section
        .lines()
        .filter(|l| l.starts_with("| `"))
        .filter_map(|l| {
            let cells: Vec<&str> = l.split('|').collect();
            // ["", " `name` ", " loaded ", " description ", ""]
            if cells.len() < 4 {
                return None;
            }
            let name = cells[1].trim().trim_matches('`').to_string();
            let loaded = cells[2].trim().trim_matches('*').trim().to_string();
            Some((name, loaded))
        })
        .collect()
}

#[test]
fn claude_md_tools_table_matches_the_code() {
    use daemoneye::ai::tools::TOOLS;

    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let text = std::fs::read_to_string(root.join("CLAUDE.md")).expect("reading CLAUDE.md");
    let section = tools_section(&text);
    let documented = documented_tools(section);

    let mut problems = Vec::new();

    // Duplicate rows would let a wrong row hide behind a right one.
    let mut seen = std::collections::BTreeSet::new();
    for (name, _) in &documented {
        if !seen.insert(name.clone()) {
            problems.push(format!("{name}: listed more than once"));
        }
    }

    let documented: std::collections::BTreeMap<_, _> = documented.into_iter().collect();
    let actual: std::collections::BTreeMap<String, String> = TOOLS
        .iter()
        .map(|t| {
            (
                t.name.to_string(),
                t.deferred_group.unwrap_or("core").to_string(),
            )
        })
        .collect();

    for (name, group) in &actual {
        match documented.get(name) {
            None => problems.push(format!(
                "{name}: in TOOLS but missing from the CLAUDE.md table (Loaded = {group})"
            )),
            Some(doc_group) if doc_group != group => problems.push(format!(
                "{name}: table says Loaded = {doc_group:?}, code says {group:?}"
            )),
            Some(_) => {}
        }
    }
    for name in documented.keys() {
        if !actual.contains_key(name) {
            problems.push(format!(
                "{name}: in the CLAUDE.md table but not in TOOLS — renamed or removed?"
            ));
        }
    }

    assert!(
        problems.is_empty(),
        "CLAUDE.md § \"Current AI tools\" is out of sync with src/ai/tools/defs.rs:\n{}\n\
         \nThe table must list every tool with a Loaded value matching \
         ToolDef.deferred_group (`core` for None, else the group name).",
        problems.join("\n")
    );
}

#[test]
fn claude_md_tools_table_counts_are_accurate() {
    use daemoneye::ai::tools::TOOLS;

    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let text = std::fs::read_to_string(root.join("CLAUDE.md")).expect("reading CLAUDE.md");
    let section = tools_section(&text);

    let total = TOOLS.len();
    let core = TOOLS.iter().filter(|t| t.deferred_group.is_none()).count();
    let deferred = total - core;

    // The prose above the table states these three numbers. A stale count is
    // the exact defect that put "six built-in knowledge memory files" in the
    // README while seven were seeded.
    let expected = format!("**{total} tools: {core} core + {deferred} deferred.**");
    assert!(
        section.contains(&expected),
        "CLAUDE.md § \"Current AI tools\" must state the real counts.\n\
         expected to find: {expected}\n\
         TOOLS currently holds {total} tools ({core} core, {deferred} deferred)."
    );
}
