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
];

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
