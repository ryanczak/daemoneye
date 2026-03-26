use anyhow::Result;

/// List all available prompts from ~/.daemoneye/prompts/.
pub fn run_prompts() -> Result<()> {
    use crate::config::{load_named_prompt, prompts_dir};

    let dir = prompts_dir();
    let mut entries: Vec<(String, String)> = Vec::new();

    if dir.is_dir() {
        let mut paths: Vec<_> = std::fs::read_dir(&dir)?
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().is_some_and(|x| x == "toml"))
            .collect();
        paths.sort_by_key(|e| e.file_name());

        for entry in paths {
            let name = entry
                .path()
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            let def = load_named_prompt(&name);
            let preview: String = def.system.chars().take(60).collect();
            entries.push((name, preview));
        }
    }

    if entries.is_empty() {
        println!("No prompts found in {}", dir.display());
        println!("Create a prompt file: {}/my-prompt.toml", dir.display());
        return Ok(());
    }

    let name_width = entries
        .iter()
        .map(|(n, _)| n.len())
        .max()
        .unwrap_or(4)
        .max(4);
    println!("\x1b[1mAvailable prompts\x1b[0m  ({})", dir.display());
    println!();
    for (name, desc) in &entries {
        println!(
            "  \x1b[1m\x1b[96m{:<width$}\x1b[0m  {}",
            name,
            desc,
            width = name_width
        );
    }
    println!();
    println!(
        "  Use \x1b[1m/prompt <name>\x1b[0m in chat to switch, or set \x1b[1mprompt = \"<name>\"\x1b[0m in config.toml."
    );
    Ok(())
}

/// List scripts in ~/.daemoneye/scripts/ (read directly, no daemon needed).
pub fn run_scripts() -> Result<()> {
    let scripts = crate::scripts::list_scripts()?;
    if scripts.is_empty() {
        let dir = crate::scripts::scripts_dir();
        println!("No scripts found in {}", dir.display());
        println!("Ask the AI to write a script, or place one there manually.");
        return Ok(());
    }
    let name_w = scripts
        .iter()
        .map(|s| s.name.len())
        .max()
        .unwrap_or(4)
        .max(4);
    println!(
        "\x1b[1mScripts\x1b[0m  ({})",
        crate::scripts::scripts_dir().display()
    );
    println!();
    for s in &scripts {
        println!(
            "  \x1b[1m\x1b[96m{:<width$}\x1b[0m  {} bytes",
            s.name,
            s.size,
            width = name_w
        );
    }
    println!();
    Ok(())
}

/// List scheduled jobs (reads schedules.json directly, no daemon needed).
pub fn run_sched_list() -> Result<()> {
    let path = crate::config::Config::schedules_path();
    let store = crate::scheduler::ScheduleStore::load_or_create(path)?;
    let jobs = store.list();
    if jobs.is_empty() {
        println!("No scheduled jobs.");
        return Ok(());
    }
    let name_w = jobs.iter().map(|j| j.name.len()).max().unwrap_or(4).max(4);
    println!("\x1b[1mScheduled Jobs\x1b[0m");
    println!();
    println!(
        "  {:<8}  {:<name_w$}  {:<16}  {:<12}  Next Run",
        "ID",
        "Name",
        "Schedule",
        "Status",
        name_w = name_w
    );
    println!(
        "  {}  {}  {}  {}  {}",
        "─".repeat(8),
        "─".repeat(name_w),
        "─".repeat(16),
        "─".repeat(12),
        "─".repeat(24)
    );
    for job in &jobs {
        let id_short = &job.id[..job.id.len().min(8)];
        let next = job
            .kind
            .next_run()
            .map(|t| t.format("%Y-%m-%d %H:%M").to_string())
            .unwrap_or_else(|| "—".to_string());
        println!(
            "  \x1b[96m{:<8}\x1b[0m  {:<name_w$}  {:<16}  {:<12}  {}",
            id_short,
            job.name,
            job.kind.describe(),
            job.status.describe(),
            next,
            name_w = name_w
        );
    }
    println!();
    Ok(())
}

/// Cancel a scheduled job by UUID prefix (reads/writes schedules.json directly).
pub fn run_sched_cancel(id: String) -> Result<()> {
    let path = crate::config::Config::schedules_path();
    let store = crate::scheduler::ScheduleStore::load_or_create(path)?;
    // Support prefix matching
    let jobs = store.list();
    let matched: Vec<&crate::scheduler::ScheduledJob> =
        jobs.iter().filter(|j| j.id.starts_with(&id)).collect();
    match matched.len() {
        0 => {
            eprintln!("No job found with ID starting with '{}'", id);
            std::process::exit(1);
        }
        1 => {
            let full_id = matched[0].id.clone();
            store.cancel(&full_id)?;
            println!("Cancelled job {} ({})", full_id, matched[0].name);
        }
        _ => {
            eprintln!(
                "Ambiguous ID prefix '{}' — matches {} jobs. Use more characters.",
                id,
                matched.len()
            );
            std::process::exit(1);
        }
    }
    Ok(())
}

/// Permanently delete a scheduled job by UUID prefix (reads/writes schedules.json directly).
pub fn run_sched_delete(id: String) -> Result<()> {
    let path = crate::config::Config::schedules_path();
    let store = crate::scheduler::ScheduleStore::load_or_create(path)?;
    // Support prefix matching
    let jobs = store.list();
    let matched: Vec<&crate::scheduler::ScheduledJob> =
        jobs.iter().filter(|j| j.id.starts_with(&id)).collect();
    match matched.len() {
        0 => {
            eprintln!("No job found with ID starting with '{}'", id);
            std::process::exit(1);
        }
        1 => {
            let full_id = matched[0].id.clone();
            store.delete(&full_id)?;
            println!("Permanently deleted job {} ({})", full_id, matched[0].name);
        }
        _ => {
            eprintln!(
                "Ambiguous ID prefix '{}' — matches {} jobs. Use more characters.",
                id,
                matched.len()
            );
            std::process::exit(1);
        }
    }
    Ok(())
}

/// List leftover de-* tmux windows (from failed scheduled jobs).
pub fn run_sched_windows() -> Result<()> {
    // Use tmux list-windows to find de-* windows
    let output = std::process::Command::new("tmux")
        .args(["list-windows", "-a", "-F", "#{session_name}:#{window_name}"])
        .output();
    match output {
        Ok(out) => {
            let text = String::from_utf8_lossy(&out.stdout);
            let de_windows: Vec<&str> = text
                .lines()
                .filter(|l| {
                    let name = l.split_once(':').map(|x| x.1).unwrap_or("");
                    name.starts_with(crate::daemon::DAEMON_WINDOW_PREFIX)
                })
                .collect();
            if de_windows.is_empty() {
                println!("No leftover de-* tmux windows found.");
            } else {
                println!("\x1b[1mLeftover scheduled job windows:\x1b[0m");
                println!();
                for w in &de_windows {
                    println!("  \x1b[96m{}\x1b[0m", w);
                }
                println!();
                println!("Kill a window:  tmux kill-window -t <session>:<window>");
            }
        }
        Err(e) => {
            eprintln!("Failed to list tmux windows: {}", e);
        }
    }
    Ok(())
}
