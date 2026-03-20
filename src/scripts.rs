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

/// Write sidecar metadata for a script to `<name>.meta.toml`.
pub fn write_script_meta(name: &str, meta: &ScriptMeta) -> Result<()> {
    validate_script_name(name)?;
    let quoted: Vec<String> = meta.tags.iter()
        .map(|t| format!("\"{}\"", t))
        .collect();
    let content = format!("tags = [{}]\n", quoted.join(", "));
    std::fs::write(meta_path(name), content)
        .with_context(|| format!("writing script meta for {}", name))
}

/// Read the sidecar metadata for a script, if it exists.
pub fn read_script_meta(name: &str) -> Option<ScriptMeta> {
    let path = meta_path(name);
    let content = std::fs::read_to_string(&path).ok()?;
    Some(ScriptMeta { tags: parse_meta_tags(&content) })
}


/// List all scripts with their optional sidecar tags.
pub fn list_scripts_with_tags() -> Result<Vec<(ScriptInfo, Vec<String>)>> {
    Ok(list_scripts()?
        .into_iter()
        .map(|s| {
            let tags = read_script_meta(&s.name).map(|m| m.tags).unwrap_or_default();
            (s, tags)
        })
        .collect())
}

fn parse_meta_tags(content: &str) -> Vec<String> {
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("tags") && trimmed.contains('=') {
            if let Some(rest) = trimmed.split_once('=').map(|(_, v)| v.trim()) {
                if let Some(inner) = rest.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
                    return inner
                        .split(',')
                        .map(|s| s.trim().trim_matches('"').trim_matches('\'').to_string())
                        .filter(|s| !s.is_empty())
                        .collect();
                }
            }
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
        unsafe { std::env::set_var("HOME", tmp); }
        f();
        match old_home {
            Some(v) => unsafe { std::env::set_var("HOME", v); },
            None => unsafe { std::env::remove_var("HOME"); },
        }
    }

    #[test]
    fn script_meta_roundtrip() {
        let tmp = std::env::temp_dir().join(format!("de_sc_meta_test_{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        with_home(&tmp, || {
            write_script("my-script.sh", "#!/bin/bash\necho hi").unwrap();
            write_script_meta("my-script.sh", &ScriptMeta {
                tags: vec!["disk".to_string(), "cleanup".to_string()],
            }).unwrap();
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
            write_script_meta("tagged.sh", &ScriptMeta {
                tags: vec!["certs".to_string()],
            }).unwrap();
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
}
