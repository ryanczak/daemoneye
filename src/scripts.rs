use anyhow::{Context, Result, bail};
use std::path::PathBuf;

/// Metadata about a script in `~/.daemoneye/scripts/`.
#[derive(Debug, Clone)]
pub struct ScriptInfo {
    pub name: String,
    pub size: u64,
}

/// Return the scripts directory: `~/.daemoneye/scripts/`.
pub fn scripts_dir() -> PathBuf {
    crate::config::config_dir().join("scripts")
}

/// Ensure the scripts directory exists.
pub fn ensure_scripts_dir() -> Result<()> {
    let dir = scripts_dir();
    std::fs::create_dir_all(&dir).with_context(|| format!("creating scripts dir {}", dir.display()))
}

/// List all files in `~/.daemoneye/scripts/`, sorted by name.
pub fn list_scripts() -> Result<Vec<ScriptInfo>> {
    let dir = scripts_dir();
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut entries = std::fs::read_dir(&dir)?
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let path = e.path();
            if !path.is_file() {
                return None;
            }
            let name = path.file_name()?.to_string_lossy().to_string();
            let size = e.metadata().ok()?.len();
            Some(ScriptInfo { name, size })
        })
        .collect::<Vec<_>>();
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(entries)
}

/// Write (create or overwrite) a script file and set its permissions to 0o700.
pub fn write_script(name: &str, content: &str) -> Result<()> {
    validate_script_name(name)?;
    ensure_scripts_dir()?;
    let path = scripts_dir().join(name);
    std::fs::write(&path, content).with_context(|| format!("writing script {}", path.display()))?;
    // chmod 700: owner can read/write/execute, no group/other permissions
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))
        .with_context(|| format!("chmod 700 {}", path.display()))?;
    crate::daemon::stats::inc_scripts_created();
    Ok(())
}

/// Return the full path of a named script, erroring if it does not exist.
pub fn resolve_script(name: &str) -> Result<PathBuf> {
    validate_script_name(name)?;
    let path = scripts_dir().join(name);
    if !path.exists() {
        bail!("Script '{}' not found in {}", name, scripts_dir().display());
    }
    Ok(path)
}

/// Delete a named script.
pub fn delete_script(name: &str) -> Result<()> {
    let path = resolve_script(name)?;
    std::fs::remove_file(&path).with_context(|| format!("deleting script {}", path.display()))?;
    crate::daemon::stats::inc_scripts_deleted();
    Ok(())
}

/// Read the content of a named script.
pub fn read_script(name: &str) -> Result<String> {
    let path = resolve_script(name)?;
    std::fs::read_to_string(&path).with_context(|| format!("reading script {}", path.display()))
}

/// List all scripts with their inline-header tags.
///
/// Tags are read from the `# --- daemoneye ---` comment header embedded in
/// each script file.  Scripts without a header return an empty tag list.
pub fn list_scripts_with_tags() -> Result<Vec<(ScriptInfo, Vec<String>)>> {
    Ok(list_scripts()?
        .into_iter()
        .map(|s| {
            let tags = read_script_tags(&s.name);
            (s, tags)
        })
        .collect())
}

/// Read the tags from a script's inline comment header (first 4 KiB only).
fn read_script_tags(name: &str) -> Vec<String> {
    let path = scripts_dir().join(name);
    let Ok(content) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    // Only scan the first 4 KiB — headers always appear near the top.
    let sample = if content.len() > 4096 {
        &content[..4096]
    } else {
        &content
    };
    let (header, _) = crate::header::parse_comment_header(sample);
    header.tags
}

/// Reject names containing path separators or other unsafe characters.
fn validate_script_name(name: &str) -> Result<()> {
    if name.is_empty() {
        bail!("Script name cannot be empty");
    }
    if name.contains('/') || name.contains('\0') || name == "." || name == ".." {
        bail!("Invalid script name: '{}'", name);
    }
    Ok(())
}

/// Generate the content for a sudoers drop-in file that grants the current user
/// NOPASSWD access to the given script.
///
/// This is a pure function and does not touch the filesystem — useful for testing.
pub fn sudoers_rule(user: &str, script_path: &str) -> String {
    format!("{} ALL=(ALL) NOPASSWD: {}\n", user, script_path)
}

/// Install a NOPASSWD sudoers rule for a named script in `~/.daemoneye/scripts/`.
///
/// Steps:
/// 1. Validates the script name and checks the script exists.
/// 2. Resolves the absolute script path.
/// 3. Determines the current username via `$USER` or `id -un`.
/// 4. Writes the rule to a temp file, then installs it to
///    `/etc/sudoers.d/daemoneye-<sanitised-name>` using
///    `sudo install -m 0440`.
/// 5. Validates the installed file with `sudo visudo -c -f <file>`;
///    removes it on validation failure.
pub fn install_sudoers(script_name: &str) -> Result<()> {
    validate_script_name(script_name)?;

    let script_path = scripts_dir().join(script_name);
    if !script_path.exists() {
        bail!(
            "Script '{}' not found in ~/.daemoneye/scripts/",
            script_name
        );
    }
    let abs_path = script_path
        .canonicalize()
        .with_context(|| format!("resolving absolute path for '{}'", script_name))?;
    let abs_path_str = abs_path.to_string_lossy().to_string();

    // Determine the current user.
    let user = std::env::var("USER")
        .or_else(|_| {
            let out = std::process::Command::new("id")
                .arg("-un")
                .output()
                .context("running 'id -un'")?;
            Ok::<String, anyhow::Error>(String::from_utf8_lossy(&out.stdout).trim().to_string())
        })
        .context("determining current username")?;
    if user.is_empty() {
        bail!("Could not determine current username");
    }

    // Sanitise the script name for use as a filename component.
    let safe_name: String = script_name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let sudoers_file = format!("/etc/sudoers.d/daemoneye-{}", safe_name);

    let rule = sudoers_rule(&user, &abs_path_str);

    // Write rule to a temp file.
    let tmp_path = format!("/tmp/daemoneye-sudoers-{}", std::process::id());
    std::fs::write(&tmp_path, &rule)
        .with_context(|| format!("writing temp sudoers file '{}'", tmp_path))?;

    // Install the temp file with correct permissions.
    let install_status = std::process::Command::new("sudo")
        .args(["install", "-m", "0440", &tmp_path, &sudoers_file])
        .status()
        .context("running 'sudo install'")?;
    let _ = std::fs::remove_file(&tmp_path);
    if !install_status.success() {
        bail!("sudo install failed with status {}", install_status);
    }

    // Validate the installed file.
    let visudo_status = std::process::Command::new("sudo")
        .args(["visudo", "-c", "-f", &sudoers_file])
        .status()
        .context("running 'sudo visudo -c'")?;
    if !visudo_status.success() {
        // Remove the invalid file before bailing.
        let _ = std::process::Command::new("sudo")
            .args(["rm", "-f", &sudoers_file])
            .status();
        bail!(
            "visudo validation failed for '{}'. The file has been removed.",
            sudoers_file
        );
    }

    println!(
        "Installed sudoers rule: {}\nRule: {}",
        sudoers_file,
        rule.trim()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::UnpoisonExt;

    #[test]
    fn validate_rejects_path_traversal() {
        assert!(validate_script_name("../etc/passwd").is_err());
        assert!(validate_script_name("sub/dir").is_err());
        assert!(validate_script_name("").is_err());
    }

    #[test]
    fn validate_accepts_normal_names() {
        assert!(validate_script_name("check-disk.sh").is_ok());
        assert!(validate_script_name("my_script").is_ok());
    }

    fn with_home<F: FnOnce()>(tmp: &std::path::Path, f: F) {
        let _guard = crate::TEST_HOME_LOCK.lock().unwrap_or_log();
        let old_home = std::env::var("HOME").ok();
        unsafe {
            std::env::set_var("HOME", tmp);
        }
        f();
        match old_home {
            Some(v) => unsafe {
                std::env::set_var("HOME", v);
            },
            None => unsafe {
                std::env::remove_var("HOME");
            },
        }
    }

    #[test]
    fn script_inline_header_roundtrip() {
        let tmp = std::env::temp_dir().join(format!("de_sc_hdr_test_{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        with_home(&tmp, || {
            let content = "#!/bin/bash\n\
                           # --- daemoneye ---\n\
                           # tags: [disk, cleanup]\n\
                           # --- /daemoneye ---\n\
                           echo hi\n";
            write_script("my-script.sh", content).unwrap();
            let tags = read_script_tags("my-script.sh");
            assert!(tags.contains(&"disk".to_string()));
            assert!(tags.contains(&"cleanup".to_string()));
        });
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn script_without_header_has_no_tags() {
        let tmp = std::env::temp_dir().join(format!("de_sc_notags_test_{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        with_home(&tmp, || {
            write_script("plain.sh", "#!/bin/bash\necho hi").unwrap();
            let tags = read_script_tags("plain.sh");
            assert!(tags.is_empty());
        });
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn list_scripts_with_tags_reads_inline_header() {
        let tmp = std::env::temp_dir().join(format!("de_sc_tags_test_{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        with_home(&tmp, || {
            let tagged = "#!/bin/bash\n\
                          # --- daemoneye ---\n\
                          # tags: [certs]\n\
                          # --- /daemoneye ---\n\
                          echo done\n";
            write_script("tagged.sh", tagged).unwrap();
            write_script("plain.sh", "#!/bin/bash\necho hi").unwrap();
            let all = list_scripts_with_tags().unwrap();
            let tagged_entry = all.iter().find(|(s, _)| s.name == "tagged.sh").unwrap();
            assert_eq!(tagged_entry.1, vec!["certs"]);
            let plain_entry = all.iter().find(|(s, _)| s.name == "plain.sh").unwrap();
            assert!(plain_entry.1.is_empty());
        });
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn sudoers_rule_content() {
        let rule = sudoers_rule("alice", "/home/alice/.daemoneye/scripts/check-disk.sh");
        assert_eq!(
            rule,
            "alice ALL=(ALL) NOPASSWD: /home/alice/.daemoneye/scripts/check-disk.sh\n"
        );
    }

    #[test]
    fn sudoers_rule_special_chars_in_path() {
        // Paths with hyphens and underscores should pass through unchanged.
        let rule = sudoers_rule("bob", "/opt/scripts/rotate_certs.sh");
        assert!(rule.starts_with("bob ALL=(ALL) NOPASSWD: /opt/scripts/rotate_certs.sh"));
    }
}
