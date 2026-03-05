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
}
