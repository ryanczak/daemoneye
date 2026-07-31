use std::io::{self, Write};
use std::path::PathBuf;
use std::process;

use crate::config::path_audit::{PathClassification, classify_text};
use crate::config::{config_dir, prompts_dir};

/// Result of auditing a single asset.
struct AssetResult {
    name: String,
    path: PathBuf,
    entries: Vec<(String, PathClassification)>,
    missing: bool,
    read_error: Option<String>,
}

/// Collect the installed assets to audit.
fn collect_assets() -> Vec<AssetResult> {
    let mut assets = Vec::new();

    // 1. SRE prompt
    let prompt_path = prompts_dir().join("sre.toml");
    assets.push(audit_file("SRE prompt", prompt_path));

    // 2. Knowledge memories
    let knowledge_dir = config_dir().join("memory").join("knowledge");
    if !knowledge_dir.is_dir() {
        assets.push(AssetResult {
            name: "Knowledge memories".to_string(),
            path: knowledge_dir.clone(),
            entries: Vec::new(),
            missing: true,
            read_error: Some(format!(
                "Knowledge directory does not exist: {}",
                knowledge_dir.display()
            )),
        });
    } else {
        let mut entries: Vec<_> = match std::fs::read_dir(&knowledge_dir) {
            Ok(rd) => rd
                .filter_map(|e| e.ok())
                .filter(|e| e.path().extension().is_some_and(|ext| ext == "md"))
                .collect(),
            Err(e) => {
                assets.push(AssetResult {
                    name: "Knowledge memories".to_string(),
                    path: knowledge_dir.clone(),
                    entries: Vec::new(),
                    missing: false,
                    read_error: Some(format!("Cannot read knowledge directory: {e}")),
                });
                return assets;
            }
        };
        entries.sort_by_key(|e| e.file_name());

        for entry in entries {
            let path = entry.path();
            let name = path
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| path.display().to_string());
            assets.push(audit_file(&name, path));
        }
    }

    assets
}

/// Audit a single file: read it, classify path literals.
fn audit_file(name: &str, path: PathBuf) -> AssetResult {
    if !path.exists() {
        return AssetResult {
            name: name.to_string(),
            path,
            entries: Vec::new(),
            missing: true,
            read_error: None,
        };
    }

    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(e) => {
            return AssetResult {
                name: name.to_string(),
                path,
                entries: Vec::new(),
                missing: false,
                read_error: Some(format!("Cannot read file: {e}")),
            };
        }
    };

    let entries = classify_text(&content);
    AssetResult {
        name: name.to_string(),
        path,
        entries,
        missing: false,
        read_error: None,
    }
}

/// Print the full audit report. Returns the number of non-current findings.
fn print_report(assets: &[AssetResult]) -> usize {
    let mut total_current = 0usize;
    let mut total_superseded = 0usize;
    let mut total_unknown = 0usize;
    let mut any_error = false;

    for asset in assets {
        println!("\x1b[1m{}\x1b[0m", asset.name);
        println!("  {}", asset.path.display());

        if asset.missing {
            if let Some(ref err) = asset.read_error {
                println!("  \x1b[31m✗ {}\x1b[0m", err);
            } else {
                println!("  \x1b[31m✗ Not installed\x1b[0m");
            }
            println!();
            any_error = true;
            continue;
        }

        if let Some(ref err) = asset.read_error {
            println!("  \x1b[31m✗ {}\x1b[0m", err);
            println!();
            any_error = true;
            continue;
        }

        if asset.entries.is_empty() {
            println!("  (no path literals found)");
            println!();
            continue;
        }

        for (literal, classification) in &asset.entries {
            match classification {
                PathClassification::Current => {
                    println!("  \x1b[32m✓\x1b[0m `{}` — current", literal);
                    total_current += 1;
                }
                PathClassification::Superseded { reason } => {
                    println!("  \x1b[33m⚠\x1b[0m `{}` — superseded: {}", literal, reason);
                    total_superseded += 1;
                }
                PathClassification::Unknown => {
                    println!("  \x1b[31m✗\x1b[0m `{}` — unknown", literal);
                    total_unknown += 1;
                }
            }
        }
        println!();
    }

    let total_literals = total_current + total_superseded + total_unknown;
    println!(
        "\x1b[1mSummary\x1b[0m: {} literals checked — {} current, {} superseded, {} unknown",
        total_literals, total_current, total_superseded, total_unknown
    );

    if any_error {
        println!("\n\x1b[31mSome assets could not be read.\x1b[0m");
    }

    total_superseded + total_unknown
}

/// CLI entry point — audit installed prompt and knowledge memory files.
pub fn run_audit_prompts() {
    let assets = collect_assets();
    let non_current = print_report(&assets);

    // Ensure output is flushed before exiting
    let _ = io::stdout().flush();

    if non_current > 0 {
        eprintln!(
            "\n\x1b[31mAudit failed: {} path issue(s) found.\x1b[0m",
            non_current
        );
        let _ = io::stderr().flush();
        process::exit(1);
    }

    // Check for missing/unreadable assets
    let has_errors = assets.iter().any(|a| a.missing || a.read_error.is_some());
    if has_errors {
        eprintln!("\n\x1b[31mAudit failed: some assets could not be read.\x1b[0m");
        let _ = io::stderr().flush();
        process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::config_dir;
    use crate::test_home_guard;
    use std::collections::HashMap;
    use std::fs;

    fn setup_test_home() -> (tempfile::TempDir, crate::TestHomeGuard) {
        let _lock = test_home_guard();
        let tmp = tempfile::tempdir().unwrap();
        unsafe {
            std::env::set_var("HOME", tmp.path());
        }
        (tmp, _lock)
    }

    #[test]
    fn audit_prompts_exits_zero_on_clean_tree() {
        let (_tmp, _lock) = setup_test_home();
        // Seed a clean tree
        crate::config::Config::ensure_dirs().unwrap();

        // Run the audit — it should exit 0 (no panic, no non-zero exit)
        // We can't easily capture process::exit in a unit test, so we test
        // the inner logic directly.
        let assets = collect_assets();
        let non_current = print_report(&assets);

        // All seeded assets should be current
        assert_eq!(non_current, 0, "expected all paths current");

        // Verify prompt was found
        let prompt = assets.iter().find(|a| a.name == "SRE prompt");
        assert!(prompt.is_some(), "SRE prompt should exist after seeding");
        assert!(!prompt.unwrap().missing);
        drop(_lock);
    }

    #[test]
    fn audit_prompts_reports_superseded_path() {
        let (_tmp, _lock) = setup_test_home();
        crate::config::Config::ensure_dirs().unwrap();

        // Inject a superseded path into the installed prompt
        let prompt_path = prompts_dir().join("sre.toml");
        let original = fs::read_to_string(&prompt_path).unwrap();
        let modified = format!(
            "{}\n# test injection: see `var/log/events.jsonl` for events\n",
            original
        );
        fs::write(&prompt_path, &modified).unwrap();

        let assets = collect_assets();
        let non_current = print_report(&assets);

        // Should find the superseded path
        assert!(non_current > 0, "expected at least one superseded path");

        // Restore
        fs::write(&prompt_path, &original).unwrap();
        drop(_lock);
    }

    #[test]
    fn audit_prompts_no_write_property() {
        let (_tmp, _lock) = setup_test_home();
        crate::config::Config::ensure_dirs().unwrap();

        // Snapshot the tree: paths + mtimes
        let snapshot = snapshot_tree(&config_dir());

        // Run the audit
        let assets = collect_assets();
        let _ = print_report(&assets);

        // Snapshot again
        let after = snapshot_tree(&config_dir());

        // Nothing should have changed
        assert_eq!(snapshot, after, "audit-prompts must not modify any files");
        drop(_lock);
    }

    #[test]
    fn audit_prompts_missing_prompt_no_panic() {
        let (_tmp, _lock) = setup_test_home();
        // Create the config dir but do NOT seed — no prompt file
        let dir = config_dir();
        fs::create_dir_all(&dir).unwrap();

        // Should not panic
        let assets = collect_assets();
        let _ = print_report(&assets);

        // Prompt should be reported as missing
        let prompt = assets.iter().find(|a| a.name == "SRE prompt");
        assert!(prompt.is_some(), "SRE prompt entry should exist");
        assert!(prompt.unwrap().missing, "SRE prompt should be missing");
        drop(_lock);
    }

    /// Snapshot a directory tree: collect all file paths and their mtimes.
    fn snapshot_tree(root: &std::path::Path) -> HashMap<PathBuf, u64> {
        let mut map = HashMap::new();
        fn walk(dir: &std::path::Path, map: &mut HashMap<PathBuf, u64>) {
            let Ok(entries) = std::fs::read_dir(dir) else {
                return;
            };
            for entry in entries.filter_map(|e| e.ok()) {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, map);
                } else if let Ok(meta) = entry.metadata()
                    && let Ok(m) = meta.modified()
                {
                    map.insert(
                        path,
                        m.duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_secs())
                            .unwrap_or(0),
                    );
                }
            }
        }
        walk(root, &mut map);
        map
    }
}
