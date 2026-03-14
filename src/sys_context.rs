use crate::util::UnpoisonExt;
use std::process::Command;
use std::sync::RwLock;

/// A snapshot of the host system's runtime state, collected once at daemon
/// startup and optionally refreshed on `/refresh`.  Injected into the AI's
/// first-turn prompt so it has immediate situational awareness.
#[derive(Debug, Clone)]
pub struct SystemContext {
    /// Output of `uname -a` (kernel version, architecture, hostname).
    pub os_info: String,
    /// Human-readable uptime from `uptime -p`.
    pub uptime: String,
    /// Raw `/proc/loadavg` content (1-, 5-, 15-minute averages + process counts).
    pub load_avg: String,
    /// Output of `free -h` (RAM and swap usage).
    pub memory: String,
    /// Top 20 processes by CPU from `ps aux --sort=-%cpu`.
    pub running_processes: String,
    /// Curated subset of environment variables (see `SAFE_VARS`).
    pub shell_env: String,
    /// Last 50 lines of the user's shell history file (bash or zsh).
    pub command_history: String,
}

static SYS_CONTEXT: RwLock<Option<SystemContext>> = RwLock::new(None);

/// Return the cached system context, collecting it on first call.
pub fn get_or_init_sys_context() -> SystemContext {
    {
        let lock = SYS_CONTEXT.read().unwrap_or_log();
        if let Some(ref ctx) = *lock {
            return ctx.clone();
        }
    }
    refresh_sys_context()
}

/// Re-collect all system context fields and update the cache.
/// Call this when the user runs `/refresh` in the chat interface.
pub fn refresh_sys_context() -> SystemContext {
    let ctx = collect();
    *SYS_CONTEXT.write().unwrap_or_log() = Some(ctx.clone());
    ctx
}

/// Run all system collectors and assemble a fresh [`SystemContext`].
fn collect() -> SystemContext {
    SystemContext {
        os_info: run_cmd("uname", &["-a"]).unwrap_or_default(),
        uptime: run_cmd("uptime", &["-p"]).unwrap_or_default(),
        load_avg: compact_load_avg(),
        memory: compact_memory(),
        running_processes: run_cmd("ps", &["-eo", "pid,%cpu,%mem,comm", "--sort=-%cpu"])
            .unwrap_or_default()
            .lines()
            .take(13) // header + 12 processes
            .collect::<Vec<_>>()
            .join("\n"),
        shell_env: curated_env(),
        command_history: get_history(),
    }
}

/// Return just the three load averages from /proc/loadavg, dropping the
/// process-count and last-PID fields which are not useful to the AI.
fn compact_load_avg() -> String {
    let raw = std::fs::read_to_string("/proc/loadavg").unwrap_or_default();
    raw.split_whitespace()
        .take(3)
        .collect::<Vec<_>>()
        .join(" ")
}

/// Return the Mem: and Swap: lines from `free -h`, dropping the header row.
fn compact_memory() -> String {
    run_cmd("free", &["-h"])
        .unwrap_or_default()
        .lines()
        .filter(|l| l.starts_with("Mem:") || l.starts_with("Swap:"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Collect a safe, curated subset of environment variables.
/// PATH is truncated to avoid bloating the prompt with long toolchain entries.
fn curated_env() -> String {
    const SAFE_VARS: &[&str] = &[
        "SHELL", "USER", "LOGNAME", "HOME", "PWD", "TERM", "LANG", "LC_ALL", "EDITOR", "VISUAL",
    ];
    let mut lines: Vec<String> = SAFE_VARS
        .iter()
        .filter_map(|k| std::env::var(k).ok().map(|v| format!("{}={}", k, v)))
        .collect();
    // Include PATH but truncate it — the full value can be hundreds of tokens.
    if let Ok(path) = std::env::var("PATH") {
        let truncated = if path.len() > 120 {
            format!("{}…", &path[..120])
        } else {
            path
        };
        lines.push(format!("PATH={}", truncated));
    }
    lines.join("\n")
}

/// Run a subprocess and return its stdout as a `String`, or `None` on failure.
fn run_cmd(cmd: &str, args: &[&str]) -> Option<String> {
    Command::new(cmd)
        .args(args)
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
}

/// Read the last 20 commands from the user's shell history file.
/// Tries `~/.bash_history` first, then `~/.zsh_history`.
/// Returns an empty string if neither file is readable.
fn get_history() -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    last_n_lines_from_file(&format!("{}/.bash_history", home), 20)
        .or_else(|| last_n_lines_from_file(&format!("{}/.zsh_history", home), 20))
        .unwrap_or_default()
}

/// Read at most `n` lines from the tail of a text file.
/// Returns `None` if the file is empty or unreadable.
fn last_n_lines_from_file(path: &str, n: usize) -> Option<String> {
    let content = std::fs::read_to_string(path).unwrap_or_default();
    if content.is_empty() {
        return None;
    }
    let lines: Vec<&str> = content.lines().rev().take(n).collect();
    Some(lines.into_iter().rev().collect::<Vec<_>>().join("\n"))
}

impl SystemContext {
    /// Format all collected fields as a single block of text suitable for
    /// injection into an AI system prompt.
    pub fn format_for_ai(&self) -> String {
        format!(
            "OS Info:\n{}\n\nUptime:\n{}\n\nLoad Average:\n{}\n\nMemory:\n{}\n\nTop Processes:\n{}\n\nShell Env:\n{}\n\nRecent History:\n{}",
            self.os_info.trim(),
            self.uptime.trim(),
            self.load_avg.trim(),
            self.memory.trim(),
            self.running_processes.trim(),
            self.shell_env.trim(),
            self.command_history.trim()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    // ── last_n_lines_from_file ────────────────────────────────────────────────

    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    fn write_to_tmp(content: &str) -> std::path::PathBuf {
        // Unique path per call: pid + monotonic counter avoids inter-test races.
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = std::path::PathBuf::from(format!(
            "/tmp/daemoneye_test_history_{}_{}.txt",
            std::process::id(),
            n,
        ));
        let mut f = std::fs::File::create(&path).unwrap();
        write!(f, "{}", content).unwrap();
        path
    }

    #[test]
    fn last_n_returns_none_for_missing_file() {
        assert!(last_n_lines_from_file("/tmp/__does_not_exist_daemoneye__", 50).is_none());
    }

    #[test]
    fn last_n_returns_none_for_empty_file() {
        let path = write_to_tmp("");
        let result = last_n_lines_from_file(path.to_str().unwrap(), 50);
        let _ = std::fs::remove_file(&path);
        assert!(result.is_none());
    }

    #[test]
    fn last_n_returns_all_when_under_limit() {
        let path = write_to_tmp("a\nb\nc");
        let result = last_n_lines_from_file(path.to_str().unwrap(), 50).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(result, "a\nb\nc");
    }

    #[test]
    fn last_n_returns_tail_when_over_limit() {
        let lines: Vec<String> = (0..30).map(|i| format!("cmd{}", i)).collect();
        let path = write_to_tmp(&lines.join("\n"));
        let result = last_n_lines_from_file(path.to_str().unwrap(), 20).unwrap();
        let _ = std::fs::remove_file(&path);

        let returned: Vec<&str> = result.lines().collect();
        assert_eq!(returned.len(), 20);
        assert_eq!(returned[0], "cmd10");
        assert_eq!(returned[19], "cmd29");
    }

    // ── curated_env ───────────────────────────────────────────────────────────

    #[test]
    fn curated_env_excludes_arbitrary_vars() {
        // curated_env filters to SAFE_VARS only; an invented name is never included.
        let env = curated_env();
        // MY_SECRET_TOKEN is not in SAFE_VARS so it must not appear regardless of env.
        assert!(!env.contains("MY_SECRET_TOKEN"));
    }

    #[test]
    fn curated_env_key_value_format() {
        // Every line (if present) must be of the form KEY=VALUE.
        let env = curated_env();
        for line in env.lines() {
            assert!(line.contains('='), "malformed env line: {line}");
        }
    }

    // ── format_for_ai ─────────────────────────────────────────────────────────

    #[test]
    fn format_for_ai_contains_all_section_headers() {
        let ctx = SystemContext {
            os_info: "Linux".to_string(),
            uptime: "up 1 hour".to_string(),
            load_avg: "0.1 0.2 0.3".to_string(),
            memory: "8G total".to_string(),
            running_processes: "PID USER ...".to_string(),
            shell_env: "SHELL=/bin/bash".to_string(),
            command_history: "ls -la".to_string(),
        };
        let text = ctx.format_for_ai();
        assert!(text.contains("OS Info:"));
        assert!(text.contains("Uptime:"));
        assert!(text.contains("Load Average:"));
        assert!(text.contains("Memory:"));
        assert!(text.contains("Top Processes:"));
        assert!(text.contains("Shell Env:"));
        assert!(text.contains("Recent History:"));
    }

    #[test]
    fn format_for_ai_trims_whitespace_from_fields() {
        let ctx = SystemContext {
            os_info: "  Linux  ".to_string(),
            uptime: "\nup 1 hour\n".to_string(),
            load_avg: String::new(),
            memory: String::new(),
            running_processes: String::new(),
            shell_env: String::new(),
            command_history: String::new(),
        };
        let text = ctx.format_for_ai();
        assert!(text.contains("Linux"), "should trim OS info");
        assert!(
            !text.contains("  Linux  "),
            "should not have surrounding spaces"
        );
    }

    // ── compact_load_avg ──────────────────────────────────────────────────────

    #[test]
    fn compact_load_avg_strips_process_fields() {
        // Simulate /proc/loadavg content: "0.15 0.12 0.18 1/3546 706811"
        // compact_load_avg reads the real file, so just verify the format contract
        // by calling the function and checking it contains exactly 3 space-separated
        // numeric tokens (or is empty if the file is missing in CI).
        let result = compact_load_avg();
        if result.is_empty() { return; } // /proc/loadavg absent (unusual)
        let parts: Vec<&str> = result.split_whitespace().collect();
        assert_eq!(parts.len(), 3, "should have exactly 3 load average values, got: {result}");
        for p in &parts {
            p.parse::<f64>().expect("each part should be a float");
        }
    }

    // ── compact_memory ────────────────────────────────────────────────────────

    #[test]
    fn compact_memory_excludes_header_row() {
        let result = compact_memory();
        if result.is_empty() { return; } // `free` not available in CI
        for line in result.lines() {
            assert!(
                line.starts_with("Mem:") || line.starts_with("Swap:"),
                "unexpected line in compact_memory output: {line}"
            );
        }
    }

    // ── curated_env PATH truncation ───────────────────────────────────────────

    #[test]
    fn curated_env_path_is_truncated() {
        let env = curated_env();
        let path_line = env.lines().find(|l| l.starts_with("PATH="));
        if let Some(line) = path_line {
            // Value portion is line minus "PATH=" prefix (5 chars).
            // With truncation marker the value is at most 121 chars (120 + "…").
            assert!(
                line.len() <= 128, // "PATH=" (5) + 120 ASCII chars + "…" (3 UTF-8 bytes)
                "PATH line too long ({} chars): {}", line.len(), &line[..line.len().min(80)]
            );
        }
    }
}
