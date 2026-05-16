use std::collections::HashMap;

/// Tags inferred from the current session context for dynamic memory retrieval.
#[derive(Debug, Clone)]
pub struct SessionTags {
    pub cwd_tags: Vec<String>,
    pub cmd_tags: Vec<String>,
    pub host_tags: Vec<String>,
    pub runbook: Option<String>,
    pub saved_name: Option<String>,
    pub recent_keywords: Vec<String>,
}

impl SessionTags {
    /// Return all tags flattened into a single vector.
    pub fn all_tags(&self) -> Vec<String> {
        let mut tags = Vec::new();
        tags.extend(self.cwd_tags.iter().cloned());
        tags.extend(self.cmd_tags.iter().cloned());
        tags.extend(self.host_tags.iter().cloned());
        if let Some(ref rb) = self.runbook {
            tags.push(rb.clone());
        }
        if let Some(ref sn) = self.saved_name {
            tags.push(sn.clone());
        }
        tags.extend(self.recent_keywords.iter().cloned());
        tags
    }
}

/// Hand-curated mapping from directory component names to tags.
fn cwd_rules() -> HashMap<&'static str, Vec<&'static str>> {
    use std::collections::HashMap;
    let mut m = HashMap::new();
    m.insert("postgres", vec!["postgres"]);
    m.insert("postgresql", vec!["postgres"]);
    m.insert("pgsql", vec!["postgres"]);
    m.insert("nginx", vec!["nginx", "http"]);
    m.insert("apache", vec!["apache", "http"]);
    m.insert("httpd", vec!["apache", "http"]);
    m.insert("redis", vec!["redis"]);
    m.insert("mongodb", vec!["mongo"]);
    m.insert("mongo", vec!["mongo"]);
    m.insert("mysql", vec!["mysql"]);
    m.insert("mariadb", vec!["mysql"]);
    m.insert("elasticsearch", vec!["elastic", "search"]);
    m.insert("kafka", vec!["kafka", "mq"]);
    m.insert("rabbitmq", vec!["rabbitmq", "mq"]);
    m.insert("docker", vec!["docker", "container"]);
    m.insert("kubernetes", vec!["k8s", "container"]);
    m.insert("k8s", vec!["k8s", "container"]);
    m.insert("var/log", vec!["logs"]);
    m.insert("log", vec!["logs"]);
    m.insert("etc", vec!["config"]);
    m.insert("tmp", vec!["temp"]);
    m.insert("home", vec!["user", "home"]);
    m.insert("ssh", vec!["ssh", "remote"]);
    m.insert("cert", vec!["ssl", "cert"]);
    m.insert("ssl", vec!["ssl", "cert"]);
    m
}

/// Hand-curated mapping from command names to tags.
fn cmd_rules() -> HashMap<&'static str, Vec<&'static str>> {
    use std::collections::HashMap;
    let mut m = HashMap::new();
    m.insert("psql", vec!["postgres"]);
    m.insert("pg_dump", vec!["postgres"]);
    m.insert("redis-cli", vec!["redis"]);
    m.insert("mongosh", vec!["mongo"]);
    m.insert("mongo", vec!["mongo"]);
    m.insert("mysql", vec!["mysql"]);
    m.insert("mariadb", vec!["mysql"]);
    m.insert("nginx", vec!["nginx", "reload"]);
    m.insert("apache2ctl", vec!["apache"]);
    m.insert("systemctl", vec!["systemd", "service"]);
    m.insert("journalctl", vec!["systemd", "logs"]);
    m.insert("docker", vec!["docker", "container"]);
    m.insert("kubectl", vec!["k8s", "container"]);
    m.insert("helm", vec!["k8s", "container"]);
    m.insert("ssh", vec!["ssh", "remote"]);
    m.insert("scp", vec!["ssh", "remote"]);
    m.insert("rsync", vec!["sync", "remote"]);
    m.insert("curl", vec!["http", "api"]);
    m.insert("wget", vec!["http", "download"]);
    m.insert("git", vec!["git", "vcs"]);
    m.insert("cargo", vec!["rust", "build"]);
    m.insert("make", vec!["build"]);
    m.insert("gcc", vec!["build", "compile"]);
    m.insert("npm", vec!["node", "js"]);
    m.insert("python", vec!["python"]);
    m.insert("python3", vec!["python"]);
    m
}

/// Infer tags from a working directory path.
/// Uses path-component extraction and hand-curated rules.
pub fn infer_tags_from_cwd(cwd: &str) -> Vec<String> {
    let rules = cwd_rules();
    let mut tags = Vec::new();

    // Split path into components
    let components: Vec<&str> = cwd.split('/').filter(|s| !s.is_empty()).collect();

    // Check last 2 path components against rules
    let start_idx = if components.len() <= 2 { 0 } else { components.len() - 2 };
    for comp in components.iter().skip(start_idx) {
        if let Some(tag_list) = rules.get(*comp) {
            for t in tag_list {
                if !tags.contains(&(*t).to_string()) {
                    tags.push((*t).to_string());
                }
            }
        }
    }

    // If no rules matched, use last 2 components as tags
    if tags.is_empty() && components.len() >= 2 {
        for comp in components.iter().rev().take(2).rev() {
            if !comp.is_empty() {
                tags.push(comp.to_string());
            }
        }
    } else if tags.is_empty() && components.len() == 1 {
        tags.push(components[0].to_string());
    }

    tags
}

/// Infer tags from a command string.
/// Looks up the command in the rules table; falls back to basename.
pub fn infer_tags_from_cmd(cmd: &str) -> Vec<String> {
    let rules = cmd_rules();
    let cmd = cmd.trim();
    if cmd.is_empty() {
        return Vec::new();
    }

    // Extract the command name (first word, stripped of path)
    let first_word = cmd.split_whitespace().next().unwrap_or(cmd);
    let basename = first_word.split('/').next_back().unwrap_or(first_word);

    if let Some(tag_list) = rules.get(basename) {
        tag_list.iter().map(|s| (*s).to_string()).collect()
    } else {
        // Fall back to basename as tag
        vec![basename.to_string()]
    }
}

/// Extract keywords from recent user turns.
/// Tokenizes messages, keeps terms >= 4 chars, returns top 10 by frequency.
pub fn extract_recent_keywords(turn_history: &[String]) -> Vec<String> {
    let mut freq: HashMap<String, usize> = HashMap::new();

    // Take last 3 turns
    let recent: Vec<&String> = turn_history.iter().rev().take(3).collect();

    for msg in recent {
        for token in msg.split_whitespace() {
            let cleaned: String = token.chars().filter(|c| c.is_alphanumeric()).collect();
            if cleaned.len() >= 4 {
                let lower = cleaned.to_lowercase();
                *freq.entry(lower).or_insert(0) += 1;
            }
        }
    }

    // Sort by frequency, then alphabetically for stability
    let mut terms: Vec<(String, usize)> = freq.into_iter().collect();
    terms.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

    terms.into_iter().take(10).map(|(k, _)| k).collect()
}

/// Infer session tags from pane state and cache.
pub fn infer_session_tags(
    client_pane: Option<&str>,
    cache: &crate::tmux::cache::SessionCache,
    recent_turns: &[String],
) -> SessionTags {
    use crate::util::UnpoisonExt;

    let (cwd, cmd) = if let Some(pane_id) = client_pane {
        let panes = cache.panes.read().unwrap_or_log();
        if let Some(pane) = panes.get(pane_id) {
            (pane.current_path.clone(), pane.current_cmd.clone())
        } else {
            (String::new(), String::new())
        }
    } else {
        (String::new(), String::new())
    };

    let cwd_tags = infer_tags_from_cwd(&cwd);
    let cmd_tags = infer_tags_from_cmd(&cmd);

    // Host tags from sys context
    let host_tags = {
        let mut tags = Vec::new();
        if let Ok(hostname) = std::env::var("HOSTNAME") {
            tags.push(hostname);
        }
        let sc = crate::sys_context::get_or_init_sys_context();
        // Extract OS family from uname output
        if !sc.os_info.is_empty() {
            let os_word = sc.os_info.split_whitespace().last().map(|s| s.to_lowercase());
            if let Some(w) = os_word {
                if w.contains("linux") {
                    tags.push("linux".to_string());
                } else if w.contains("darwin") {
                    tags.push("macos".to_string());
                }
            }
        }
        tags
    };

    SessionTags {
        cwd_tags,
        cmd_tags,
        host_tags,
        runbook: None,
        saved_name: None,
        recent_keywords: extract_recent_keywords(recent_turns),
    }
}
