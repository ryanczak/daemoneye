//! Repo-hygiene gate: fail when a doc reintroduces a claim about the memory
//! index that stopped being true when FTS5 search landed.

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
