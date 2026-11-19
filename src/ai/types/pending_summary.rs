use super::PendingCall;

impl PendingCall {
    /// Returns a short human-readable argument summary for the `ToolStarted` event.
    pub fn summary(&self) -> String {
        if let Some(s) = self.shell_summary() {
            return s;
        }
        match self {
            PendingCall::CancelSchedule { job_id, .. }
            | PendingCall::DeleteSchedule { job_id, .. } => job_id.clone(),
            PendingCall::ReadScript { script_name, .. }
            | PendingCall::DeleteScript { script_name, .. } => script_name.clone(),
            PendingCall::ReadFile {
                path,
                offset,
                limit,
                pattern,
                ..
            } => {
                let mut s = path.clone();
                match (offset, limit) {
                    (Some(o), Some(l)) if *o > 0 || *l > 0 => {
                        s.push_str(&format!(" lines={}-{}", o, o + l));
                    }
                    (Some(o), None) if *o > 0 => {
                        s.push_str(&format!(" offset={o}"));
                    }
                    _ => {}
                }
                if let Some(pat) = pattern {
                    s.push_str(&format!(" grep=\"{pat}\""));
                }
                s
            }
            PendingCall::ReadRunbook { name, .. } => name.clone(),
            PendingCall::AddMemory { key, category, .. }
            | PendingCall::UpdateMemory { key, category, .. }
            | PendingCall::DeleteMemory { key, category, .. }
            | PendingCall::ReadMemory { key, category, .. } => {
                format!("{category}/{key}")
            }
            PendingCall::ListMemories { category, .. } => {
                category.as_deref().unwrap_or("all").to_string()
            }
            PendingCall::SearchRepository { query, .. } => {
                let q = crate::util::truncate_chars(query, 40);
                format!("\"{q}\"")
            }
            PendingCall::SpawnGhost { runbook, .. } => runbook.clone(),
            PendingCall::CreateAgent { name, .. }
            | PendingCall::ReadAgent { name, .. }
            | PendingCall::DeleteAgent { name, .. } => name.clone(),
            PendingCall::ListAgents { .. } => String::new(),
            PendingCall::AwaitAgentResult { job_id, .. } => job_id.clone(),
            PendingCall::LoadTools { groups, .. } => {
                format!("load_tools: {}", groups.join(", "))
            }
            PendingCall::RunLogTail { run_id, limit, .. } => match limit {
                Some(n) => format!("tail {run_id} limit={n}"),
                None => format!("tail {run_id} (default limit)"),
            },
            PendingCall::RunLogSearch { run_id, query, .. } => {
                format!("search {run_id} \"{query}\"")
            }
            PendingCall::GetTurn { run_id, turn, .. } => {
                format!("turn {turn} of {run_id}")
            }
            PendingCall::InvalidArgs { name, .. } => format!("invalid arguments for {name}"),
            _ => String::new(),
        }
    }

    fn shell_summary(&self) -> Option<String> {
        let s = match self {
            // A foreground run may name the shell it targets and the host it
            // runs on; both are what the user watching the line needs to know
            // — a panel that says only "run a command" for a remote command is
            // the wrong summary.
            PendingCall::Foreground {
                cmd, shell, host, ..
            } => {
                let q = crate::util::truncate_chars(cmd, 40);
                match (shell, host) {
                    (Some(s), Some(h)) => format!("\"{q}\" in {s} on {h}"),
                    (Some(s), None) => format!("\"{q}\" in {s}"),
                    (None, Some(h)) => format!("\"{q}\" on {h}"),
                    (None, None) => format!("\"{q}\""),
                }
            }
            PendingCall::WatchShell {
                shell,
                timeout_secs,
                pattern,
                ..
            } => {
                if let Some(pat) = pattern {
                    format!("{shell} pattern=\"{pat}\"")
                } else {
                    format!("{shell} {timeout_secs}s")
                }
            }
            PendingCall::GetShellContext { scope, .. } | PendingCall::ListShells { scope, .. } => {
                scope.clone().unwrap_or_default()
            }
            // A close shows which shell; an open shows which host, when one
            // was named — the ordinary case is local, i.e. nothing to show.
            PendingCall::OpenShell {
                host,
                rows: Some(r),
                cols: Some(c),
                ..
            } => format!("{host:?} rows={r} cols={c}"),
            PendingCall::OpenShell { host, .. } => host.clone().unwrap_or_default(),
            PendingCall::CloseShell { shell, .. } | PendingCall::SendInput { shell, .. } => {
                shell.clone()
            }
            PendingCall::ReadShell {
                shell,
                lines,
                since_command,
                filter,
                ..
            } => {
                let mut s = shell.clone();
                if let Some(n) = lines {
                    s.push_str(&format!(" lines={n}"));
                }
                if let Some(i) = since_command {
                    s.push_str(&format!(" since_command={i}"));
                }
                if let Some(f) = filter {
                    s.push_str(&format!(" filter=\"{f}\""));
                }
                s
            }
            PendingCall::ReadShellScreen {
                shell,
                lines,
                scrollback,
                pattern,
                wait_for,
                settle_ms,
                ..
            } => {
                let mut s = shell.clone();
                if let Some(n) = lines {
                    s.push_str(&format!(" lines={n}"));
                }
                if let Some(n) = scrollback {
                    s.push_str(&format!(" scrollback={n}"));
                }
                if let Some(p) = pattern {
                    s.push_str(&format!(" pattern=\"{p}\""));
                }
                if let Some(p) = wait_for {
                    s.push_str(&format!(" wait_for=\"{p}\""));
                }
                if let Some(n) = settle_ms {
                    s.push_str(&format!(" settle_ms={n}"));
                }
                s
            }
            PendingCall::FindInShells { pattern, scope, .. } => match scope {
                Some(s) => format!("/{pattern}/ scope={s}"),
                None => format!("/{pattern}/"),
            },
            PendingCall::ListShellLogs { since, limit, .. } => match (since, limit) {
                (Some(s), Some(l)) => format!("since {s} limit={l}"),
                (Some(s), None) => format!("since {s}"),
                (None, Some(l)) => format!("limit={l}"),
                (None, None) => String::new(),
            },
            PendingCall::ReadShellLog {
                log,
                command_index,
                lines,
                filter,
                ..
            } => {
                let mut s = log.clone();
                if let Some(i) = command_index {
                    s.push_str(&format!(" command_index={i}"));
                }
                if let Some(n) = lines {
                    s.push_str(&format!(" lines={n}"));
                }
                if let Some(f) = filter {
                    s.push_str(&format!(" filter=\"{f}\""));
                }
                s
            }
            _ => return None,
        };
        Some(s)
    }
}
