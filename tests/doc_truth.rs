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

// ---------------------------------------------------------------------------
// README.md § "AI tools" must mirror the real `TOOLS` table too
// ---------------------------------------------------------------------------
//
// `CLAUDE.md`'s table has been gated since M11; the README's has not, and it
// drifted three tools and a whole milestone behind (33/24 while the code held
// 36/27) before anyone noticed. Same comparison, different table shape: the
// README splits Core and Deferred into two tables, puts the deferred tools
// inside a group row, and marks approval-gated tools with a `⚠`.

/// The heading that opens the AI-tools tables in `README.md`.
const README_TOOLS_HEADING: &str = "## AI tools";

/// The section of `README.md` between the AI-tools heading and the next `## `.
fn readme_tools_section(text: &str) -> &str {
    let start = text
        .find(README_TOOLS_HEADING)
        .unwrap_or_else(|| panic!("README.md no longer contains {README_TOOLS_HEADING:?}"));
    let rest = &text[start + README_TOOLS_HEADING.len()..];
    match rest.find("\n## ") {
        Some(i) => &rest[..i],
        None => rest,
    }
}

/// Every `` `backticked` `` identifier in a string, in order.
fn backticked(s: &str) -> Vec<String> {
    s.split('`')
        .skip(1)
        .step_by(2)
        .map(|t| t.trim().to_string())
        .collect()
}

/// `(tool name, group)` for every tool the README documents. `group` is `core`
/// for rows in the Core table, else the deferred group's name — the same shape
/// `documented_tools` returns for `CLAUDE.md`, so the two can be compared to
/// `TOOLS` the same way.
fn readme_documented_tools(section: &str) -> Vec<(String, String)> {
    let (core_part, deferred_part) = match section.find("### Deferred") {
        Some(i) => (&section[..i], &section[i..]),
        None => panic!("README.md § \"AI tools\" no longer has a \"### Deferred\" table"),
    };

    let mut out: Vec<(String, String)> = core_part
        .lines()
        .filter(|l| l.starts_with("| `"))
        .filter_map(|l| Some((backticked(l).first()?.clone(), "core".to_string())))
        .collect();

    // Deferred rows are `| `group` | `tool` ⚠, `tool`, … |` — the first
    // backticked token is the group, the rest are its tools.
    for line in deferred_part.lines().filter(|l| l.starts_with("| `")) {
        let names = backticked(line);
        let Some((group, tools)) = names.split_first() else {
            continue;
        };
        for tool in tools {
            out.push((tool.clone(), group.clone()));
        }
    }
    out
}

#[test]
fn readme_tools_tables_match_the_code() {
    use daemoneye::ai::tools::TOOLS;

    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let text = std::fs::read_to_string(root.join("README.md")).expect("reading README.md");
    let documented: std::collections::BTreeMap<String, String> =
        readme_documented_tools(readme_tools_section(&text))
            .into_iter()
            .collect();

    let actual: std::collections::BTreeMap<String, String> = TOOLS
        .iter()
        .map(|t| {
            (
                t.name.to_string(),
                t.deferred_group.unwrap_or("core").to_string(),
            )
        })
        .collect();

    let mut problems = Vec::new();
    for (name, group) in &actual {
        match documented.get(name) {
            None => problems.push(format!(
                "{name}: in TOOLS but missing from the README tables (belongs in {group})"
            )),
            Some(doc_group) if doc_group != group => problems.push(format!(
                "{name}: README lists it under {doc_group}, TOOLS says {group}"
            )),
            Some(_) => {}
        }
    }
    for name in documented.keys() {
        if !actual.contains_key(name) {
            problems.push(format!(
                "{name}: in the README tables but not in TOOLS — renamed or removed?"
            ));
        }
    }

    assert!(
        problems.is_empty(),
        "README.md § \"AI tools\" no longer mirrors `TOOLS`:\n{}\n\n\
         Core-table rows are core; a tool listed in a Deferred group row must \
         match that tool's ToolDef.deferred_group.",
        problems.join("\n")
    );
}

#[test]
fn readme_tools_counts_are_accurate() {
    use daemoneye::ai::tools::TOOLS;

    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let text = std::fs::read_to_string(root.join("README.md")).expect("reading README.md");
    let section = readme_tools_section(&text);

    let total = TOOLS.len();
    let core = TOOLS.iter().filter(|t| t.deferred_group.is_none()).count();
    let deferred = total - core;

    // The README states the three numbers in prose rather than in one bolded
    // run, so each is checked separately — a partially-updated sentence is the
    // realistic drift, not a wholly-rewritten one.
    for expected in [
        format!("**{total} tools**"),
        format!("The {core} **core** tools"),
        format!("the {deferred} **deferred** tools"),
    ] {
        assert!(
            section.contains(&expected),
            "README.md § \"AI tools\" must state the real counts.\n\
             expected to find: {expected}\n\
             TOOLS currently holds {total} tools ({core} core, {deferred} deferred)."
        );
    }
}

// ---------------------------------------------------------------------------
// README.md's `⚠` markers must match the tools that actually prompt
// ---------------------------------------------------------------------------
//
// The marker means "requires explicit user approval before it executes", and
// nothing checked it. Building this gate found two defects: `schedule_command`
// prompts and was unmarked, and `daemon::stream`'s `APPROVAL_GATED` — the
// obvious oracle — turned out to be a *budget-exemption* list that disagrees
// with the prompting set in both directions. The oracle is therefore
// `APPROVAL_GATED_TOOLS`, derived by reading the executor arms.

/// Tool names carrying a `⚠` marker in README.md's AI-tools tables.
fn readme_approval_marked(section: &str) -> std::collections::BTreeSet<String> {
    let deferred_at = section.find("### Deferred").unwrap_or(section.len());
    let mut marked = std::collections::BTreeSet::new();

    for (offset, line) in section
        .match_indices('\n')
        .map(|(i, _)| i + 1)
        .filter_map(|i| section[i..].lines().next().map(|l| (i, l)))
    {
        if !line.starts_with("| `") {
            continue;
        }
        if offset < deferred_at {
            // Core row: `| `name` **⚠** | description |` — the marker, when
            // present, sits in the first cell beside the name.
            let Some(first_cell) = line.split('|').nth(1) else {
                continue;
            };
            if first_cell.contains('⚠')
                && let Some(name) = backticked(first_cell).first()
            {
                marked.insert(name.clone());
            }
        } else {
            // Deferred row: `| `group` | `a` **⚠**, `b` |` — each tool carries
            // its own marker, so split the cell on commas.
            let Some(tools_cell) = line.split('|').nth(2) else {
                continue;
            };
            for entry in tools_cell.split(',') {
                if entry.contains('⚠')
                    && let Some(name) = backticked(entry).first()
                {
                    marked.insert(name.clone());
                }
            }
        }
    }
    marked
}

#[test]
fn readme_approval_markers_match_the_gated_tools() {
    use daemoneye::ai::tools::APPROVAL_GATED_TOOLS;

    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let text = std::fs::read_to_string(root.join("README.md")).expect("reading README.md");
    let marked = readme_approval_marked(readme_tools_section(&text));
    let expected: std::collections::BTreeSet<String> = APPROVAL_GATED_TOOLS
        .iter()
        .map(|s| (*s).to_string())
        .collect();

    let missing: Vec<&String> = expected.difference(&marked).collect();
    let extra: Vec<&String> = marked.difference(&expected).collect();

    assert!(
        missing.is_empty() && extra.is_empty(),
        "README.md's ⚠ markers no longer match `APPROVAL_GATED_TOOLS`.\n\
         gated but unmarked in the README: {missing:?}\n\
         marked in the README but not gated: {extra:?}\n\n\
         The marker means the tool prompts the user before it executes. If a \
         tool's gating really changed, update APPROVAL_GATED_TOOLS in \
         src/ai/tools/defs.rs — after checking its executor arm actually sends \
         a Response::*Prompt and waits."
    );
}

#[test]
fn approval_gated_tools_all_exist() {
    use daemoneye::ai::tools::{APPROVAL_GATED_TOOLS, TOOLS};

    let known: std::collections::BTreeSet<&str> = TOOLS.iter().map(|t| t.name).collect();
    let unknown: Vec<&&str> = APPROVAL_GATED_TOOLS
        .iter()
        .filter(|n| !known.contains(*n))
        .collect();

    assert!(
        unknown.is_empty(),
        "APPROVAL_GATED_TOOLS names tools that are not in TOOLS: {unknown:?} — \
         renamed or removed?"
    );
}
