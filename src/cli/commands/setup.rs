//! `daemoneye setup` — initialize the `~/.daemoneye/` directory layout, copy the
//! binary into `bin/`, write the systemd user service file, and print the
//! tmux bind-key / shell-integration snippets.

use anyhow::Result;
use std::path::PathBuf;

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
    // Ensure the full ~/.daemoneye/ directory tree and default files are in place.
    // (Also called at the top of main(), but being explicit here makes setup self-contained.)
    crate::config::Config::ensure_dirs()
        .map_err(|e| anyhow::anyhow!("Failed to initialise config directory: {}", e))?;

    let dir = crate::config::config_dir();
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
        "  {}/var/log/               ← daemon.log and pipe-pane capture logs",
        dir.display()
    );
    println!(
        "  {}/bin/                   ← place symlinks/wrappers here",
        dir.display()
    );
    println!(
        "  {}/lib/                   ← shared SDK modules (de_sdk, Python helpers)",
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
    println!();
    let memory_dir = dir.join("memory");
    let seeded_knowledge = [
        "webhook-setup",
        "runbook-format",
        "runbook-ghost-template",
        "ghost-shell-guide",
        "scheduling-guide",
        "scripts-and-sudoers",
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

    // Copy the running binary into ~/.daemoneye/bin/daemoneye.
    // On first run (no binary present) always copy; on upgrade require --overwrite-bin.
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

    // Overwrite the built-in SRE prompt when --overwrite-all is in effect.
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

    // Write the systemd user service file using the bin/ path.
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

    // Use the ~/.daemoneye/bin/ copy so the bind-key is stable across cargo reinstalls
    // and works even when ~/.cargo/bin is not in the PATH inherited by tmux.
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
    println!("# To enable accurate exit-code tracking for foreground commands,");
    println!("# add the appropriate snippet to your shell config:");
    println!();
    println!("# bash (~/.bashrc):");
    println!(
        "_de_exit_trap() {{ tmux set-environment \"DE_EXIT_${{TMUX_PANE#%}}\" \"$?\" 2>/dev/null; }}"
    );
    println!("PROMPT_COMMAND=\"_de_exit_trap${{PROMPT_COMMAND:+; $PROMPT_COMMAND}}\"");
    println!();
    println!("# zsh (~/.zshrc):");
    println!(
        "_de_precmd() {{ tmux set-environment \"DE_EXIT_${{TMUX_PANE#%}}\" \"$?\" 2>/dev/null; }}"
    );
    println!("precmd_functions+=(_de_precmd)");

    println!();
    println!("# ── Server / systemd use ────────────────────────────────────────────────────");
    println!("# When running as a systemd user service, add to ~/.daemoneye/etc/config.toml:");
    println!("#");
    println!("#   [daemon]");
    println!("#   tmux_session = \"daemoneye\"   # session the daemon creates at startup");
    println!("#");
    println!("# The daemon will create the session automatically and `daemoneye chat`");
    println!("# will attach to it when run from outside tmux.");

    Ok(())
}
