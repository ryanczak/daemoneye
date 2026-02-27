use std::process::Command;
use std::sync::OnceLock;

#[derive(Debug, Clone)]
pub struct SystemContext {
    pub os_info: String,
    pub uptime: String,
    pub load_avg: String,
    pub memory: String,
    pub running_processes: String,
    pub shell_env: String,
    pub command_history: String,
}

static SYS_CONTEXT: OnceLock<SystemContext> = OnceLock::new();

pub fn get_or_init_sys_context() -> &'static SystemContext {
    SYS_CONTEXT.get_or_init(|| SystemContext {
        os_info: run_cmd("uname", &["-a"]).unwrap_or_default(),
        uptime: run_cmd("uptime", &["-p"]).unwrap_or_default(),
        load_avg: run_cmd("cat", &["/proc/loadavg"]).unwrap_or_default(),
        memory: run_cmd("free", &["-h"]).unwrap_or_default(),
        running_processes: run_cmd("ps", &["aux", "--sort=-%cpu"])
            .unwrap_or_default()
            .lines()
            .take(20)
            .collect::<Vec<_>>()
            .join(
                "
",
            ),
        shell_env: curated_env(),
        command_history: get_history(),
    })
}

fn curated_env() -> String {
    const SAFE_VARS: &[&str] = &[
        "SHELL", "USER", "LOGNAME", "HOME", "PATH", "PWD",
        "TERM", "LANG", "LC_ALL", "EDITOR", "VISUAL",
    ];
    SAFE_VARS
        .iter()
        .filter_map(|k| std::env::var(k).ok().map(|v| format!("{}={}", k, v)))
        .collect::<Vec<_>>()
        .join("\n")
}

fn run_cmd(cmd: &str, args: &[&str]) -> Option<String> {
    Command::new(cmd)
        .args(args)
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
}

fn get_history() -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    let bash = std::fs::read_to_string(format!("{}/.bash_history", home)).unwrap_or_default();
    if !bash.is_empty() {
        return bash
            .lines()
            .rev()
            .take(50)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join(
                "
",
            );
    }
    let zsh = std::fs::read_to_string(format!("{}/.zsh_history", home)).unwrap_or_default();
    if !zsh.is_empty() {
        return zsh
            .lines()
            .rev()
            .take(50)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join(
                "
",
            );
    }
    String::new()
}

impl SystemContext {
    pub fn format_for_ai(&self) -> String {
        format!(
            "OS Info:
{}

Uptime:
{}

Load Average:
{}

Memory:
{}

Top Processes:
{}

Shell Env:
{}

Recent History:
{}",
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
