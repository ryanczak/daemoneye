use anyhow::{Context, Result, bail};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
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

    // Best-effort index the script. Tags come from inline header parsing.
    let tags = read_script_tags(name);
    let tags_str = tags.join(",");
    if let Err(e) = crate::memory::index::index_artifact("script", name, &tags_str, content) {
        log::warn!("script index update failed for '{}': {e:#}", name);
    }

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

    // Best-effort remove from index.
    if let Err(e) = crate::memory::index::remove_artifact("script", name) {
        log::warn!("script index removal failed for '{}': {e:#}", name);
    }

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
    if name == "." || name == ".." {
        bail!("Invalid script name: '{}'", name);
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-')
    {
        bail!("Invalid script name: '{}'", name);
    }
    Ok(())
}

/// Generate the content for a sudoers drop-in file that grants the current user
/// NOPASSWD access to the given script.
///
/// This is a pure function and does not touch the filesystem — useful for testing.
pub fn sudoers_rule(user: &str, script_path: &str) -> String {
    format!(
        "{} ALL=(ALL) NOPASSWD: {}\n",
        user,
        sudoers_escape_path(script_path)
    )
}

/// Escape sudoers-special characters in a pathname.
///
/// Per sudoers(5), these characters terminate words or inject directives and must
/// be backslash-escaped: `\`, space, tab, `@`, `!`, `=`, `:`, `,`, `(`, `)`.
/// Backslash is escaped first so the other escapes are not re-escaped.
fn sudoers_escape_path(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for c in path.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            ' ' | '\t' | '@' | '!' | '=' | ':' | ',' | '(' | ')' => {
                out.push('\\');
                out.push(c);
            }
            _ => out.push(c),
        }
    }
    out
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

    // Write the rule to a private temp file (M1): placed inside ~/.daemoneye
    // (already 0700) with O_EXCL + mode 0600, so another local user cannot
    // race us by pre-creating a symlink or swapping content at a predictable
    // /tmp path between write and install. sudo install reads it from here.
    let tmp_path = crate::config::var_run_dir().join(format!("sudoers-{}.tmp", std::process::id()));
    let write_result = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&tmp_path)
        .and_then(|mut f| f.write_all(rule.as_bytes()))
        .with_context(|| format!("writing temp sudoers file '{}'", tmp_path.display()));
    if let Err(e) = write_result {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(e);
    }

    // Install the temp file with correct permissions.  Cleanup of the temp
    // file happens regardless of install success/failure.
    let tmp_str = tmp_path.to_str().context("rendering temp sudoers path")?;
    let install_result = std::process::Command::new("sudo")
        .args(["install", "-m", "0440", tmp_str, &sudoers_file])
        .status();
    let _ = std::fs::remove_file(&tmp_path);
    let install_status = install_result.context("running 'sudo install'")?;
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

/// Hex-encode a string (no external crate required).
fn to_hex(s: &str) -> String {
    s.bytes().map(|b| format!("{:02x}", b)).collect()
}

/// Build a self-contained remote shell fragment that materializes `name` (with
/// the given `content`) into `~/.daemoneye/scripts/<name>` on the remote host,
/// `chmod 700`, atomically (temp file + rename) and idempotently (overwrites any
/// existing copy). Content is hex-encoded so no byte of the script reaches the
/// remote shell unquoted. The fragment exits non-zero on any failure, so it is
/// safe to `&&`-join before the script invocation.
///
/// `name` is assumed already validated to `[A-Za-z0-9._-]` (see
/// `validate_script_name`), so it is safe to interpolate unquoted into the path.
pub fn remote_materialize_cmd(name: &str, content: &str) -> String {
    let hex = to_hex(content);
    format!(
        "mkdir -p ~/.daemoneye/scripts && \\\n\
         if command -v python3 >/dev/null 2>&1; then \\\n\
           python3 -c \"import sys;sys.stdout.buffer.write(bytes.fromhex('{}'))\"; \\\n\
         else \\\n\
           perl -e 'print pack(\"H*\",\"{}\")'; \\\n\
         fi > ~/.daemoneye/scripts/{}.de_tmp && \\\n\
         chmod 700 ~/.daemoneye/scripts/{}.de_tmp && \\\n\
         mv -f ~/.daemoneye/scripts/{}.de_tmp ~/.daemoneye/scripts/{}",
        hex, hex, name, name, name, name,
    )
}

/// Derive the interpreter command name from a script's shebang line.
///
/// Returns a name guaranteed to match `[A-Za-z0-9._-]+` (safe to interpolate
/// unquoted into the remote command). Falls back to `"bash"` when there is no
/// shebang or the derived name is not a clean interpreter token.
fn shebang_interpreter(content: &str) -> String {
    let first_line = content.lines().next().unwrap_or("");
    let Some(rest) = first_line.strip_prefix("#!") else {
        return "bash".to_string();
    };
    let tokens: Vec<&str> = rest.split_whitespace().collect();
    let tokens = if tokens.is_empty() {
        return "bash".to_string();
    } else {
        tokens
    };

    let interp_token = if tokens[0].ends_with("env") && tokens.len() >= 2 {
        tokens[1]
    } else {
        tokens[0]
    };

    // Extract basename (strip any directory prefix)
    let basename = std::path::Path::new(interp_token)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(interp_token)
        .to_string();

    // Validate against safe charset — reject anything that could inject shell metachars
    if basename
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-')
    {
        basename
    } else {
        "bash".to_string()
    }
}

/// Parse a command line for a daemon-host script invocation suitable for streaming to
/// a remote pane. Strips one optional leading `sudo ` token, then inspects the first
/// whitespace-delimited token of the remainder:
///
/// - returns `None` if there is no first token, or the token is an **absolute** path
///   (starts with `/`);
/// - otherwise takes the token's **basename**; returns `None` if that basename is not a
///   valid script name (`validate_script_name` — the `[A-Za-z0-9._-]` allowlist);
/// - on success returns `Some((basename, args_tail))` where `args_tail` is everything
///   after the first token, verbatim, with its single leading space preserved (empty if
///   there were no args).
///
/// Pure parser — does NOT touch the filesystem. The caller confirms the script exists on
/// the daemon host via `read_script`; a parse hit whose script does not exist is a normal
/// remote command, not an error.
pub fn parse_script_invocation(cmd: &str) -> Option<(String, String)> {
    let cmd = cmd.strip_prefix("sudo ").unwrap_or(cmd).trim_start();
    let mut parts = cmd.splitn(2, char::is_whitespace);
    let first = parts.next()?;
    if first.is_empty() {
        return None;
    }
    if first.starts_with('/') {
        return None;
    }
    let args_tail = parts.next().map(|s| format!(" {}", s)).unwrap_or_default();
    let basename = std::path::Path::new(first).file_name()?.to_str()?;
    if validate_script_name(basename).is_err() {
        return None;
    }
    Some((basename.to_string(), args_tail))
}

/// Build a remote shell fragment that runs `content` (a daemon-host script) on the
/// remote host **without writing it to the remote filesystem**: the hex-encoded
/// content is decoded on the remote and piped straight into the shebang-derived
/// interpreter's stdin via `/dev/stdin`. `args` is the verbatim argument tail from
/// the original invocation (already shell text, e.g. " --flag arg", or empty); it is
/// appended after the interpreter so the script's positional parameters are set.
///
/// Content is hex-encoded, so no byte of the script reaches the remote shell unquoted.
pub fn remote_stream_cmd(content: &str, args: &str) -> String {
    let hex = to_hex(content);
    let interp = shebang_interpreter(content);
    format!(
        "{{ if command -v python3 >/dev/null 2>&1; then \\\n\
          python3 -c \"import sys;sys.stdout.buffer.write(bytes.fromhex('{}'))\"; \\\n\
        else \\\n\
          perl -e 'print pack(\"H*\",\"{}\")'; \\\n\
        fi; }} | {} /dev/stdin{}",
        hex, hex, interp, args,
    )
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

    fn with_home<F: FnOnce()>(tmp: &std::path::Path, f: F) {
        let _guard = crate::test_home_guard();
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

    #[test]
    fn validate_rejects_metacharacters() {
        assert!(validate_script_name("foo bar").is_err());
        assert!(validate_script_name("foo;rm -rf").is_err());
        assert!(validate_script_name("foo|bar").is_err());
        assert!(validate_script_name("foo&bar").is_err());
        assert!(validate_script_name("foo$x").is_err());
        assert!(validate_script_name("foo>out").is_err());
        assert!(validate_script_name("a`b").is_err());
        assert!(validate_script_name("foo\nbar").is_err());
        assert!(validate_script_name("foo\0").is_err());
        assert!(validate_script_name("foo'bar").is_err());
        assert!(validate_script_name("foo(bar)").is_err());
    }

    #[test]
    fn validate_accepts_allowlisted() {
        assert!(validate_script_name("check-disk.sh").is_ok());
        assert!(validate_script_name("my_script").is_ok());
        assert!(validate_script_name("a.b.c").is_ok());
        assert!(validate_script_name("Backup-01").is_ok());
        assert!(validate_script_name("x").is_ok());
    }

    #[test]
    fn sudoers_rule_escapes_special_chars() {
        // Space and comma in path must be backslash-escaped.
        let rule = sudoers_rule("alice", "/home/od d/scripts/a,b.sh");
        assert!(rule.contains("\\ "), "space should be escaped");
        assert!(rule.contains("\\,"), "comma should be escaped");
        assert!(!rule.contains("od d"), "raw space must not appear");
        assert!(!rule.contains("a,b"), "raw comma must not appear");
        assert!(rule.ends_with('\n'));

        // Literal backslash must be doubled.
        let rule = sudoers_rule("alice", "/home/al\\ice/scripts/x.sh");
        assert!(rule.contains("\\\\"), "backslash should be doubled");
    }

    #[test]
    fn sudoers_rule_passthrough_when_safe() {
        let path = "/home/alice/.daemoneye/scripts/check-disk.sh";
        let rule = sudoers_rule("alice", path);
        assert_eq!(
            rule,
            "alice ALL=(ALL) NOPASSWD: /home/alice/.daemoneye/scripts/check-disk.sh\n"
        );
    }

    #[test]
    fn remote_materialize_cmd_contains_hex_not_raw() {
        let content = "echo secret-token\n";
        let output = remote_materialize_cmd("foo.sh", content);
        let expected_hex = to_hex(content);
        assert!(
            output.contains(&expected_hex),
            "output should contain the hex encoding, got: {}",
            output
        );
        assert!(
            !output.contains("echo secret-token"),
            "output must NOT contain the raw content verbatim, got: {}",
            output
        );
    }

    #[test]
    fn remote_materialize_cmd_has_mkdir_chmod_atomic_mv() {
        let output = remote_materialize_cmd("foo.sh", "echo hi");
        assert!(output.contains("mkdir -p ~/.daemoneye/scripts"));
        assert!(output.contains("chmod 700"));
        assert!(output.contains(".de_tmp"));
        assert!(
            output.contains("mv -f ~/.daemoneye/scripts/foo.sh.de_tmp ~/.daemoneye/scripts/foo.sh")
        );
    }

    #[test]
    fn remote_materialize_cmd_has_python_and_perl_branches() {
        let output = remote_materialize_cmd("foo.sh", "echo hi");
        assert!(output.contains("python3"));
        assert!(output.contains("perl"));
    }

    #[test]
    fn remote_materialize_cmd_metachars_stay_hex() {
        let content = "x'; rm -rf / #\n";
        let output = remote_materialize_cmd("foo.sh", content);
        let expected_hex = to_hex(content);
        assert!(
            output.contains(&expected_hex),
            "output should contain the hex encoding, got: {}",
            output
        );
        // The raw content substring must NOT appear outside the hex blob.
        assert!(
            !output.contains("x'; rm -rf / #"),
            "raw metacharacters must not appear in output, got: {}",
            output
        );
    }

    #[test]
    fn remote_stream_cmd_pipes_hex_no_disk() {
        let content = "#!/bin/bash\necho hi\n";
        let output = remote_stream_cmd(content, "");
        let expected_hex = to_hex(content);
        assert!(
            output.contains(&expected_hex),
            "output should contain the hex encoding, got: {}",
            output
        );
        assert!(
            !output.contains("echo hi"),
            "output must NOT contain the raw content, got: {}",
            output
        );
        assert!(
            output.contains("bash /dev/stdin"),
            "output should pipe into bash /dev/stdin, got: {}",
            output
        );
        // Negative property: no remote disk write
        assert!(
            !output.contains("mkdir"),
            "output must NOT contain mkdir, got: {}",
            output
        );
        assert!(
            !output.contains(".de_tmp"),
            "output must NOT contain .de_tmp, got: {}",
            output
        );
        assert!(
            !output.contains(" mv "),
            "output must NOT contain mv, got: {}",
            output
        );
        // No redirection of decoded bytes to a file path
        assert!(
            !output.contains("> ~/") && !output.contains("> /tmp/"),
            "output must NOT redirect decoded bytes to a file path, got: {}",
            output
        );
    }

    #[test]
    fn remote_stream_cmd_passes_args() {
        let content = "#!/bin/bash\necho hi\n";
        let output = remote_stream_cmd(content, " --flag arg");
        assert!(
            output.ends_with("bash /dev/stdin --flag arg"),
            "output should end with interpreter + args, got: {}",
            output
        );
    }

    #[test]
    fn remote_stream_cmd_python_and_perl_branches() {
        let output = remote_stream_cmd("#!/bin/bash\necho hi\n", "");
        assert!(output.contains("python3"));
        assert!(output.contains("perl"));
    }

    #[test]
    fn remote_stream_cmd_honors_shebang() {
        let content = "#!/usr/bin/env python3\nprint(1)\n";
        let output = remote_stream_cmd(content, "");
        assert!(
            output.contains("python3 /dev/stdin"),
            "output should use python3 interpreter, got: {}",
            output
        );
    }

    #[test]
    fn shebang_interpreter_cases() {
        assert_eq!(shebang_interpreter("#!/bin/bash\necho hi"), "bash");
        assert_eq!(
            shebang_interpreter("#!/usr/bin/env python3\nprint(1)"),
            "python3"
        );
        assert_eq!(shebang_interpreter("#!/usr/bin/perl -w\nprint 1"), "perl");
        assert_eq!(shebang_interpreter("echo hi"), "bash");
        // Injection case: semicolon in basename fails charset gate
        assert_eq!(shebang_interpreter("#!/bin/sh; rm -rf /\necho hi"), "bash");
    }

    #[test]
    fn parse_script_invocation_bare_name() {
        let result = parse_script_invocation("foo.sh");
        assert_eq!(result, Some(("foo.sh".into(), "".into())));
    }

    #[test]
    fn parse_script_invocation_with_args() {
        let result = parse_script_invocation("foo.sh --flag arg");
        assert_eq!(result, Some(("foo.sh".into(), " --flag arg".into())));
    }

    #[test]
    fn parse_script_invocation_strips_sudo() {
        let result = parse_script_invocation("sudo foo.sh --flag");
        assert_eq!(result, Some(("foo.sh".into(), " --flag".into())));
    }

    #[test]
    fn parse_script_invocation_relative() {
        let result = parse_script_invocation("./foo.sh");
        assert_eq!(result, Some(("foo.sh".into(), "".into())));
    }

    #[test]
    fn parse_script_invocation_none_for_absolute() {
        let result = parse_script_invocation("/usr/bin/foo.sh");
        assert_eq!(result, None);
    }

    #[test]
    fn parse_script_invocation_none_for_empty() {
        let result = parse_script_invocation("");
        assert_eq!(result, None);
    }

    #[test]
    fn parse_script_invocation_rejects_metachar_name() {
        let result = parse_script_invocation("foo;rm -rf /");
        assert_eq!(result, None);
    }
}
