//! `daemoneye setup` — initialize the `~/.daemoneye/` directory layout, copy the
//! binary into `bin/`, write the systemd user service file, and print the
//! tmux bind-key / shell-integration snippets.

use anyhow::Result;
use std::path::{Path, PathBuf};

fn print_directory_layout(dir: &Path) {
    println!("Initialised ~/.daemoneye/ layout:");
    println!(
        "  {}/etc/config.toml       ← edit this to configure the daemon",
        dir.display()
    );
    println!(
        "  {}/etc/prompts/           ← system prompt files (.toml)",
        dir.display()
    );
    println!(
        "  {}/var/run/               ← socket, schedules, pane prefs",
        dir.display()
    );
    println!(
        "  {}/var/log/               ← daemon.log and per-pane capture logs",
        dir.display()
    );
    println!(
        "  {}/bin/                   ← place symlinks/wrappers here",
        dir.display()
    );
    println!(
        "  {}/scripts/               ← automation scripts",
        dir.display()
    );
    println!(
        "  {}/runbooks/              ← procedure runbooks",
        dir.display()
    );
    println!(
        "  {}/memory/                ← persistent AI memory",
        dir.display()
    );
    println!(
        "  {}/agents/               ← named agent configs",
        dir.display()
    );
    println!();
}

fn report_seeded_memories_and_agents(dir: &Path, overwrite_memory: bool) {
    let memory_dir = dir.join("memory");
    let seeded_knowledge = [
        "webhook-setup",
        "runbook-format",
        "runbook-ghost-template",
        "ghost-shell-guide",
        "scheduling-guide",
        "scripts-and-sudoers",
        "agent-runtime-layout",
        "config-reference",
    ];
    let seeded_session = ["pane-referencing-convention", "unicode-decoration-pref"];
    if overwrite_memory {
        println!("Overwriting built-in memories:");
        match crate::config::overwrite_knowledge_memories() {
            Ok(()) => {
                for key in &seeded_knowledge {
                    println!("  knowledge/{}  ✓ (overwritten)", key);
                }
                for key in &seeded_session {
                    println!("  session/{}  ✓ (overwritten)", key);
                }
            }
            Err(e) => eprintln!("Warning: could not overwrite memories: {}", e),
        }
    } else {
        println!("Seeded memories (written once, preserved on upgrade):");
        for key in &seeded_knowledge {
            let exists = memory_dir
                .join("knowledge")
                .join(format!("{}.md", key))
                .exists();
            println!(
                "  knowledge/{}  {}",
                key,
                if exists { "✓" } else { "(missing)" }
            );
        }
        for key in &seeded_session {
            let exists = memory_dir
                .join("session")
                .join(format!("{}.md", key))
                .exists();
            println!(
                "  session/{}  {}",
                key,
                if exists { "✓" } else { "(missing)" }
            );
        }
    }
    println!();

    // Show seeded agent status.
    let seeded_agents = [
        (
            "architect",
            "opus",
            "Anthropic Opus — design reviews and planning",
        ),
        (
            "researcher",
            "gemini-pro",
            "Gemini Pro — deep investigation and research",
        ),
        (
            "sysadmin",
            "lmstudio",
            "LM Studio (Qwen3-27B) — local infrastructure tasks",
        ),
    ];
    println!("Seeded agents (written once, preserved on upgrade):");
    for (name, model_key, description) in &seeded_agents {
        let exists = crate::agents::config_path(name).exists();
        println!(
            "  agents/{}  {}  [model: {}]",
            name,
            if exists { "✓" } else { "(missing)" },
            model_key,
        );
        if !exists {
            println!("    {}", description);
        }
    }
    println!();
    println!("  To activate the example agents, uncomment the matching [models.*] stanzas");
    println!(
        "  in {}/etc/config.toml and fill in your API keys.",
        dir.display()
    );
    println!();
}

fn write_systemd_service_and_print_hints(bin_dest: &Path) {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    let systemd_dir = PathBuf::from(&home).join(".config/systemd/user");
    let service_path = systemd_dir.join("daemoneye.service");

    let service_content = "\
[Unit]
Description=DaemonEye Tmux Daemon
After=network.target

[Service]
Type=simple
# --console: don't fork; write logs to stdout so systemd/journald captures them.
ExecStart=%h/.daemoneye/bin/daemoneye daemon --console
ExecStop=%h/.daemoneye/bin/daemoneye stop
Restart=on-failure
RestartSec=5
Environment=\"PATH=%h/.daemoneye/bin:/usr/local/bin:/usr/bin:/bin\"

[Install]
WantedBy=default.target
";

    match std::fs::create_dir_all(&systemd_dir)
        .and_then(|_| std::fs::write(&service_path, service_content))
    {
        Ok(()) => {
            println!("Wrote {}", service_path.display());
            println!();
            println!("# Enable and start the daemon:");
            println!("systemctl --user daemon-reload");
            println!("systemctl --user enable --now daemoneye");
            println!();
            println!("# Check status and view logs:");
            println!("systemctl --user status daemoneye");
            println!("daemoneye logs");
        }
        Err(e) => {
            eprintln!("Warning: could not write service file: {}", e);
            eprintln!("You can install it manually:");
            eprintln!("  mkdir -p ~/.config/systemd/user");
            eprintln!("  cp daemoneye.service ~/.config/systemd/user/");
        }
    }

    let split_flag = "-v";
    let daemon_bin = bin_dest.to_string_lossy().into_owned();

    println!();
    println!("# Add this to your ~/.tmux.conf:");
    println!(
        "bind-key T split-window {} '{} chat'",
        split_flag, daemon_bin
    );
    println!();
    println!("# Then reload tmux config:");
    println!("tmux source-file ~/.tmux.conf");
    println!();
    println!("# If you already have a bind-key that uses the bare name 'daemoneye',");
    println!("# replace it with the full path above — the tmux session may not");
    println!("# inherit ~/.cargo/bin in its PATH.");

    println!();
    println!("# ── Server / systemd use ────────────────────────────────────────────────────");
    println!("# When running as a systemd user service, add to ~/.daemoneye/etc/config.toml:");
    println!("#");
    println!("#   [daemon]");
    println!("#   tmux_session = \"daemoneye\"   # session the daemon creates at startup");
    println!("#");
    println!("# The daemon will create the session automatically and `daemoneye chat`");
    println!("# will attach to it when run from outside tmux.");
}

/// Run `daemoneye setup`.
///
/// - `overwrite_bin`    — copy the current executable to `~/.daemoneye/bin/daemoneye`
///   even if a copy already exists there.
/// - `overwrite_memory` — overwrite the built-in knowledge and session memory files with the
///   versions bundled in this binary.
/// - `overwrite_prompt` — overwrite `~/.daemoneye/etc/prompts/sre.toml` with the
///   version bundled in this binary (implied by `--overwrite-all`).
pub fn run_setup(
    overwrite_bin: bool,
    overwrite_memory: bool,
    overwrite_prompt: bool,
) -> Result<()> {
    crate::config::Config::ensure_dirs()
        .map_err(|e| anyhow::anyhow!("Failed to initialise config directory: {}", e))?;

    let dir = crate::config::config_dir();
    print_directory_layout(&dir);
    report_seeded_memories_and_agents(&dir, overwrite_memory);

    let bin_dest = crate::config::bin_dir().join("daemoneye");
    let current_exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("daemoneye"));
    let bin_exists = bin_dest.exists();
    if !bin_exists || overwrite_bin {
        match std::fs::copy(&current_exe, &bin_dest) {
            Ok(_) => {
                if bin_exists {
                    println!("Updated binary → {}", bin_dest.display());
                } else {
                    println!("Copied binary → {}", bin_dest.display());
                }
            }
            Err(e) => eprintln!(
                "Warning: could not copy binary to {}: {}",
                bin_dest.display(),
                e
            ),
        }
    } else {
        println!(
            "Binary already installed at {} (use --overwrite-bin to update)",
            bin_dest.display()
        );
    }
    println!();

    if overwrite_prompt {
        match crate::config::overwrite_sre_prompt() {
            Ok(()) => println!(
                "Refreshed built-in SRE prompt → {}/etc/prompts/sre.toml",
                dir.display()
            ),
            Err(e) => eprintln!("Warning: could not overwrite SRE prompt: {}", e),
        }
        println!();
    }

    write_systemd_service_and_print_hints(&bin_dest);
    Ok(())
}
