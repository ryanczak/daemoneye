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
            if name.ends_with(".meta.toml") {
                return None;
            }
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

/// Delete a named script and its sidecar `.meta.toml` if present.
pub fn delete_script(name: &str) -> Result<()> {
    let path = resolve_script(name)?;
    std::fs::remove_file(&path).with_context(|| format!("deleting script {}", path.display()))?;
    let meta = meta_path(name);
    if meta.exists() {
        let _ = std::fs::remove_file(&meta);
    }
    crate::daemon::stats::inc_scripts_deleted();
    Ok(())
}

/// Read the content of a named script.
pub fn read_script(name: &str) -> Result<String> {
    let path = resolve_script(name)?;
    std::fs::read_to_string(&path).with_context(|| format!("reading script {}", path.display()))
}

/// Optional sidecar metadata for a script, stored in `<name>.meta.toml`.
pub struct ScriptMeta {
    pub tags: Vec<String>,
}

fn meta_path(name: &str) -> std::path::PathBuf {
    scripts_dir().join(format!("{}.meta.toml", name))
}

/// Read the sidecar metadata for a script, if it exists.
pub fn read_script_meta(name: &str) -> Option<ScriptMeta> {
    let path = meta_path(name);
    let content = std::fs::read_to_string(&path).ok()?;
    Some(ScriptMeta {
        tags: parse_meta_tags(&content),
    })
}

/// List all scripts with their optional sidecar tags.
pub fn list_scripts_with_tags() -> Result<Vec<(ScriptInfo, Vec<String>)>> {
    Ok(list_scripts()?
        .into_iter()
        .map(|s| {
            let tags = read_script_meta(&s.name)
                .map(|m| m.tags)
                .unwrap_or_default();
            (s, tags)
        })
        .collect())
}

fn parse_meta_tags(content: &str) -> Vec<String> {
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("tags") && trimmed.contains('=')
            && let Some(rest) = trimmed.split_once('=').map(|(_, v)| v.trim())
                && let Some(inner) = rest.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
                    return inner
                        .split(',')
                        .map(|s| s.trim().trim_matches('"').trim_matches('\'').to_string())
                        .filter(|s| !s.is_empty())
                        .collect();
                }
    }
    Vec::new()
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
        bail!("Script '{}' not found in ~/.daemoneye/scripts/", script_name);
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
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' || c == '.' { c } else { '_' })
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
    fn script_meta_roundtrip() {
        let tmp = std::env::temp_dir().join(format!("de_sc_meta_test_{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        with_home(&tmp, || {
            write_script("my-script.sh", "#!/bin/bash\necho hi").unwrap();
            std::fs::write(
                meta_path("my-script.sh"),
                "tags = [\"disk\", \"cleanup\"]\n",
            )
            .unwrap();
            let meta = read_script_meta("my-script.sh").expect("meta not found");
            assert!(meta.tags.contains(&"disk".to_string()));
            assert!(meta.tags.contains(&"cleanup".to_string()));
        });
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn script_without_meta_has_no_tags() {
        let tmp = std::env::temp_dir().join(format!("de_sc_notags_test_{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        with_home(&tmp, || {
            write_script("plain.sh", "#!/bin/bash\necho hi").unwrap();
            let meta = read_script_meta("plain.sh");
            assert!(meta.is_none() || meta.unwrap().tags.is_empty());
        });
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn list_scripts_with_tags_includes_meta() {
        let tmp = std::env::temp_dir().join(format!("de_sc_tags_test_{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        with_home(&tmp, || {
            write_script("tagged.sh", "#!/bin/bash\necho hi").unwrap();
            std::fs::write(meta_path("tagged.sh"), "tags = [\"certs\"]\n").unwrap();
            write_script("plain.sh", "#!/bin/bash\necho hi").unwrap();
            let all = list_scripts_with_tags().unwrap();
            let tagged = all.iter().find(|(s, _)| s.name == "tagged.sh").unwrap();
            assert_eq!(tagged.1, vec!["certs"]);
            let plain = all.iter().find(|(s, _)| s.name == "plain.sh").unwrap();
            assert!(plain.1.is_empty());
            // .meta.toml should not appear in the list
            assert!(!all.iter().any(|(s, _)| s.name.ends_with(".meta.toml")));
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
