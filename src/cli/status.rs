use anyhow::Result;
use tokio::io::BufReader;

use crate::cli::commands::{connect, recv, send_request};
use crate::ipc::{Request, Response};

pub async fn run_status() -> Result<()> {
    match connect().await {
        Err(_) => {
            println!("Daemon is not running.");
            std::process::exit(1);
        }
        Ok(stream) => {
            let (rx, mut tx) = stream.into_split();
            let mut rx = BufReader::new(rx);
            send_request(&mut tx, Request::Status).await?;
            match recv(&mut rx).await {
                Ok(Response::DaemonStatus {
                    uptime_secs,
                    pid,
                    active_sessions,
                    total_turns,
                    provider,
                    model,
                    available_models: _,
                    socket_path,
                    schedule_count,
                    commands_fg_succeeded,
                    commands_fg_failed,
                    commands_fg_approved,
                    commands_fg_denied,
                    commands_bg_succeeded,
                    commands_bg_failed,
                    commands_bg_approved,
                    commands_bg_denied,
                    commands_sched_succeeded,
                    commands_sched_failed,
                    ghosts_launched,
                    ghosts_active,
                    ghosts_completed,
                    ghosts_failed,
                    webhooks_received,
                    webhooks_rejected,
                    webhook_url,
                    runbook_count,
                    runbooks_created,
                    runbooks_executed,
                    runbooks_deleted,
                    script_count,
                    scripts_created,
                    scripts_executed,
                    scripts_deleted,
                    memories_created,
                    memories_recalled,
                    memories_deleted,
                    schedules_created,
                    schedules_executed,
                    schedules_deleted,
                    active_prompt_tokens,
                    context_window_tokens,
                    recent_commands,
                    memory_breakdown,
                    redaction_counts,
                    compactions,
                    compaction_ratio,
                    scripts_approved,
                    scripts_denied,
                    runbooks_approved,
                    runbooks_denied,
                    file_edits_approved,
                    file_edits_denied,
                    limits,
                }) => {
                    let hours = uptime_secs / 3600;
                    let mins = (uptime_secs % 3600) / 60;
                    let secs = uptime_secs % 60;
                    let uptime_str = if hours > 0 {
                        format!("{}h {}m {}s", hours, mins, secs)
                    } else if mins > 0 {
                        format!("{}m {}s", mins, secs)
                    } else {
                        format!("{}s", secs)
                    };

                    let blood_red = "\x1b[1m\x1b[38;2;180;0;0m";
                    let deep_yellow = "\x1b[38;2;220;160;0m";
                    let reset = "\x1b[0m";
                    let bold_white = "\x1b[1m\x1b[37m";

                    let commands_fg_str = format!(
                        "{} approved, {} denied ({} ok, {} fail)",
                        commands_fg_approved,
                        commands_fg_denied,
                        commands_fg_succeeded,
                        commands_fg_failed
                    );

                    let commands_bg_str = format!(
                        "{} approved, {} denied ({} ok, {} fail)",
                        commands_bg_approved,
                        commands_bg_denied,
                        commands_bg_succeeded,
                        commands_bg_failed
                    );

                    let commands_sched_total = commands_sched_succeeded + commands_sched_failed;
                    let commands_sched_str = format!(
                        "{} ({} ok, {} fail)",
                        commands_sched_total, commands_sched_succeeded, commands_sched_failed
                    );

                    let tokens_pct = if context_window_tokens > 0 {
                        ((active_prompt_tokens as f64 / context_window_tokens as f64) * 100.0)
                            as u32
                    } else {
                        0
                    };

                    let home_dir = std::env::var("HOME").unwrap_or_default();
                    let display_socket_path =
                        if !home_dir.is_empty() && socket_path.starts_with(&home_dir) {
                            socket_path.replacen(&home_dir, "~", 1)
                        } else {
                            socket_path.clone()
                        };

                    let mut left_items = Vec::new();
                    left_items.push(("PID:".to_string(), pid.to_string()));
                    left_items.push(("Uptime:".to_string(), uptime_str));
                    left_items.push(("Socket:".to_string(), display_socket_path));
                    left_items.push(("─".to_string(), "".to_string()));
                    left_items.push(("§".to_string(), "Webhook".to_string()));
                    left_items.push(("URL:".to_string(), webhook_url));
                    left_items.push(("Received:".to_string(), webhooks_received.to_string()));
                    left_items.push(("Rejected:".to_string(), webhooks_rejected.to_string()));
                    left_items.push(("─".to_string(), "".to_string()));
                    left_items.push(("§".to_string(), "Commands".to_string()));
                    left_items.push(("Foreground:".to_string(), commands_fg_str));
                    left_items.push(("Background:".to_string(), commands_bg_str));
                    left_items.push(("Scheduled:".to_string(), commands_sched_str));
                    left_items.push(("─".to_string(), "".to_string()));
                    left_items.push(("§".to_string(), "Tooling".to_string()));
                    left_items.push((
                        "Runbooks:".to_string(),
                        format!("{} existing", runbook_count),
                    ));
                    left_items.push((
                        "".to_string(),
                        format!(
                            "  {} created, {} executed, {} deleted",
                            runbooks_created, runbooks_executed, runbooks_deleted
                        ),
                    ));
                    left_items.push((
                        "".to_string(),
                        format!(
                            "  {} approved, {} denied (writes)",
                            runbooks_approved, runbooks_denied
                        ),
                    ));
                    left_items.push(("Scripts:".to_string(), format!("{} existing", script_count)));
                    left_items.push((
                        "".to_string(),
                        format!(
                            "  {} created, {} executed, {} deleted",
                            scripts_created, scripts_executed, scripts_deleted
                        ),
                    ));
                    left_items.push((
                        "".to_string(),
                        format!(
                            "  {} approved, {} denied (writes)",
                            scripts_approved, scripts_denied
                        ),
                    ));
                    left_items.push((
                        "File edits:".to_string(),
                        format!(
                            "{} approved, {} denied",
                            file_edits_approved, file_edits_denied
                        ),
                    ));
                    left_items.push((
                        "Schedules:".to_string(),
                        format!("{} active", schedule_count),
                    ));
                    left_items.push((
                        "".to_string(),
                        format!(
                            "  {} created, {} executed, {} deleted",
                            schedules_created, schedules_executed, schedules_deleted
                        ),
                    ));

                    let mut right_items = Vec::new();
                    right_items.push(("Active sessions:".to_string(), active_sessions.to_string()));
                    right_items.push((
                        "Active model:".to_string(),
                        format!("{}/{}", provider, model),
                    ));
                    right_items.push(("Turn count:".to_string(), total_turns.to_string()));
                    right_items.push((
                        "Token budget:".to_string(),
                        format!(
                            "{} / {} ({}%)",
                            active_prompt_tokens, context_window_tokens, tokens_pct
                        ),
                    ));
                    right_items.push((
                        "Compactions:".to_string(),
                        if compactions == 0 {
                            "0".to_string()
                        } else {
                            format!("{} ({:.1}:1)", compactions, compaction_ratio)
                        },
                    ));
                    right_items.push(("─".to_string(), "".to_string()));
                    right_items.push(("§".to_string(), "Memories".to_string()));
                    const KNOWN_CATS: &[&str] = &["knowledge", "session", "incident"];
                    for cat in KNOWN_CATS {
                        let count = memory_breakdown.get(*cat).copied().unwrap_or(0);
                        let label = format!("{}{}:", cat[..1].to_uppercase(), &cat[1..]);
                        right_items.push((label, count.to_string()));
                    }
                    right_items.push((
                        "".to_string(),
                        format!(
                            "  {} created, {} recalled, {} deleted",
                            memories_created, memories_recalled, memories_deleted
                        ),
                    ));

                    let mut mem_cats: Vec<_> = memory_breakdown
                        .into_iter()
                        .filter(|(cat, _)| !KNOWN_CATS.contains(&cat.as_str()))
                        .collect();
                    mem_cats.sort_by_key(|k| k.0.clone());
                    for (cat, count) in mem_cats {
                        right_items.push((format!("  - {}:", cat), count.to_string()));
                    }

                    right_items.push(("─".to_string(), "".to_string()));
                    right_items.push(("§".to_string(), "Limits".to_string()));
                    let fmt_cap_u32 = |v: u32| if v == 0 { "unlimited".to_string() } else { v.to_string() };
                    let fmt_cap_usize = |v: usize| if v == 0 { "unlimited".to_string() } else { v.to_string() };
                    right_items.push(("Per-tool batch:".to_string(), fmt_cap_u32(limits.per_tool_batch)));
                    right_items.push(("Total/turn:".to_string(), fmt_cap_u32(limits.total_tool_calls_per_turn)));
                    right_items.push(("Result chars:".to_string(), fmt_cap_usize(limits.tool_result_chars)));
                    right_items.push(("Max history:".to_string(), fmt_cap_usize(limits.max_history)));
                    right_items.push(("Max turns:".to_string(), fmt_cap_usize(limits.max_turns)));
                    right_items.push(("Session tools:".to_string(), fmt_cap_usize(limits.max_tool_calls_per_session)));
                    if !limits.per_tool_overrides.is_empty() {
                        let overrides_str = limits
                            .per_tool_overrides
                            .iter()
                            .map(|(tool, cap)| format!("{}={}", tool, fmt_cap_u32(*cap)))
                            .collect::<Vec<_>>()
                            .join(", ");
                        right_items.push(("".to_string(), format!("  overrides: {}", overrides_str)));
                    }

                    // Use a special marker to indicate this separator needs a top joint (┬)
                    // for the side-by-side divider.
                    right_items.push(("┬─".to_string(), "".to_string()));

                    // We render Redactions and Ghosts side-by-side using a combined list.
                    // The '§' header is handled specially to show both titles.
                    right_items.push((
                        "§".to_string(),
                        format!(
                            "{:<23}{deep_yellow}│{reset}{blood_red} Ghost Shells",
                            "Redactions",
                            deep_yellow = deep_yellow,
                            reset = reset,
                            blood_red = blood_red
                        ),
                    ));

                    let mut redact_sorted: Vec<_> = redaction_counts.into_iter().collect();
                    redact_sorted.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));

                    let ghost_metrics = [
                        (format!("{:<10}", " Active:   "), ghosts_active.to_string()),
                        (
                            format!("{:<10}", " Launched: "),
                            ghosts_launched.to_string(),
                        ),
                        (
                            format!("{:<10}", " Completed:"),
                            ghosts_completed.to_string(),
                        ),
                        (format!("{:<10}", " Failed:   "), ghosts_failed.to_string()),
                    ];

                    for (i, (rtype, count)) in redact_sorted.into_iter().enumerate() {
                        let left_col = format!(" {:<18} {}", rtype + ":", count);
                        if let Some((gk, gv)) = ghost_metrics.get(i) {
                            right_items.push((
                                "".to_string(),
                                format!(
                                    "{:<23}{deep_yellow}│{reset} {} {}",
                                    left_col,
                                    gk,
                                    gv,
                                    deep_yellow = deep_yellow,
                                    reset = reset
                                ),
                            ));
                        } else {
                            right_items.push((
                                "".to_string(),
                                format!(
                                    "{:<23}{deep_yellow}│{reset}",
                                    left_col,
                                    deep_yellow = deep_yellow,
                                    reset = reset
                                ),
                            ));
                        }
                    }

                    let term_width: usize = {
                        fn tiocgwinsz(fd: libc::c_int) -> Option<usize> {
                            let mut ws = libc::winsize {
                                ws_row: 0,
                                ws_col: 0,
                                ws_xpixel: 0,
                                ws_ypixel: 0,
                            };
                            let ret = unsafe { libc::ioctl(fd, libc::TIOCGWINSZ, &mut ws) };
                            if ret == 0 && ws.ws_col > 0 {
                                Some(ws.ws_col as usize)
                            } else {
                                None
                            }
                        }
                        tiocgwinsz(libc::STDOUT_FILENO)
                            .or_else(|| tiocgwinsz(libc::STDERR_FILENO))
                            .or_else(|| tiocgwinsz(libc::STDIN_FILENO))
                            .or_else(|| {
                                use std::fs::OpenOptions;
                                use std::os::unix::io::AsRawFd;
                                OpenOptions::new()
                                    .read(true)
                                    .write(true)
                                    .open("/dev/tty")
                                    .ok()
                                    .and_then(|f| tiocgwinsz(f.as_raw_fd()))
                            })
                            .or_else(|| std::env::var("COLUMNS").ok().and_then(|v| v.parse().ok()))
                            .unwrap_or(80)
                    };

                    let col_width = {
                        let raw = left_items
                            .iter()
                            .map(|(k, v)| {
                                if k == "─" {
                                    0
                                } else if k == "§" || k.is_empty() {
                                    v.chars().count()
                                } else {
                                    19 + v.chars().count()
                                }
                            })
                            .max()
                            .unwrap_or(40)
                            + 2;
                        // Cap at ~55% of terminal width so long Socket/URL values
                        // trigger the existing left-truncation logic instead of
                        // expanding the column past the screen edge.
                        raw.min((term_width * 55 / 100).max(40))
                    };
                    // All dividers render as: (col_width+1) dashes + join-char + right_width dashes
                    // = col_width + right_width + 2 chars total.  Set right_width so that equals term_width.
                    let right_width = term_width.saturating_sub(col_width + 2).max(20);

                    let title_left = format!("{}Process Information{}", blood_red, reset);
                    let title_right = format!("{}Session Information{}", blood_red, reset);
                    let left_pad_str = " ".repeat(col_width.saturating_sub(19));
                    println!(
                        "{}{} {deep_yellow}│{reset} {}",
                        title_left, left_pad_str, title_right
                    );

                    let rows_count = left_items.len().max(right_items.len());
                    for i in 0..rows_count {
                        let (lk, lv) = left_items
                            .get(i)
                            .cloned()
                            .unwrap_or(("".to_string(), "".to_string()));
                        let (rk, rv) = right_items
                            .get(i)
                            .cloned()
                            .unwrap_or(("".to_string(), "".to_string()));

                        let l_color = bold_white;
                        let r_color = bold_white;

                        // Blood-red section header sentinel
                        if lk == "§" || rk == "§" {
                            let l_str = if lk == "§" {
                                let vis_len = lv.chars().count();
                                let pad = col_width.saturating_sub(vis_len);
                                format!("{}{}{reset}{}", blood_red, lv, " ".repeat(pad))
                            } else {
                                // normal left side
                                let lv_trunc = if !lk.is_empty() {
                                    let lv_chars: Vec<char> = lv.chars().collect();
                                    if lv_chars.len() > col_width - 19 {
                                        let max_len = col_width - 19 - 3;
                                        let trunc: String =
                                            lv_chars[lv_chars.len() - max_len..].iter().collect();
                                        format!("...{}", trunc)
                                    } else {
                                        lv.to_string()
                                    }
                                } else {
                                    lv.to_string()
                                };
                                if lk.is_empty() {
                                    let vis_len = lv_trunc.chars().count();
                                    let pad = col_width.saturating_sub(vis_len);
                                    format!("{}{}{reset}{}", l_color, lv_trunc, " ".repeat(pad))
                                } else {
                                    let f = format!(" {:<18}{}{}{reset}", lk, l_color, lv_trunc);
                                    let vis_len = 19 + lv_trunc.chars().count();
                                    let pad = col_width.saturating_sub(vis_len);
                                    format!("{}{}", f, " ".repeat(pad))
                                }
                            };
                            let r_str = if rk == "§" {
                                format!("{}{}{reset}", blood_red, rv)
                            } else if rk.is_empty() {
                                format!("{}{}{reset}", r_color, rv)
                            } else {
                                format!(" {:<18}{}{}{reset}", rk, r_color, rv)
                            };
                            if rk == "─" {
                                println!(
                                    "{} {deep_yellow}├{:─<right_width$}{reset}",
                                    l_str,
                                    "",
                                    right_width = right_width
                                );
                            } else if rk == "┬─" {
                                let mut r_line = String::with_capacity(right_width * 3);
                                for j in 0..right_width {
                                    if j == 24 {
                                        r_line.push('┬');
                                    } else {
                                        r_line.push('─');
                                    }
                                }
                                println!("{} {deep_yellow}├{}{reset}", l_str, r_line);
                            } else if rv.is_empty() && rk != "§" {
                                println!("{} {deep_yellow}│{reset}", l_str);
                            } else {
                                println!("{} {deep_yellow}│{reset} {}", l_str, r_str);
                            }
                            continue;
                        }

                        if lk == "─" && (rk == "─" || rk == "┬─") {
                            let mut r_line = String::with_capacity(right_width * 3);
                            for i in 0..right_width {
                                if rk == "┬─" && i == 24 {
                                    r_line.push('┬');
                                } else {
                                    r_line.push('─');
                                }
                            }
                            println!(
                                "{deep_yellow}{:─<col_width$}┼{}{reset}",
                                "",
                                r_line,
                                col_width = col_width + 1
                            );
                            continue;
                        } else if lk == "─" {
                            let r_str = if rk.is_empty() {
                                format!("{}{}{reset}", r_color, rv)
                            } else {
                                format!(" {:<18}{}{}{reset}", rk, r_color, rv)
                            };
                            println!(
                                "{deep_yellow}{:─<col_width$}┤{reset} {}",
                                "",
                                r_str,
                                col_width = col_width + 1
                            );
                            continue;
                        } else if rk == "─" || rk == "┬─" {
                            let lv_trunc = if !lk.is_empty() {
                                let lv_chars: Vec<char> = lv.chars().collect();
                                if lv_chars.len() > col_width - 19 {
                                    let max_len = col_width - 19 - 3;
                                    let trunc: String =
                                        lv_chars[lv_chars.len() - max_len..].iter().collect();
                                    format!("...{}", trunc)
                                } else {
                                    lv.to_string()
                                }
                            } else {
                                lv.to_string()
                            };

                            let l_str = if lk.is_empty() {
                                format!("{:<col_width$}", "", col_width = col_width)
                            } else {
                                let f = format!(" {:<18}{}{}{reset}", lk, l_color, lv_trunc);
                                let vis_len = 19 + lv_trunc.chars().count();
                                let pad = col_width.saturating_sub(vis_len);
                                format!("{}{}", f, " ".repeat(pad))
                            };

                            let mut r_line = String::with_capacity(right_width * 3);
                            for i in 0..right_width {
                                if rk == "┬─" && i == 24 {
                                    r_line.push('┬');
                                } else {
                                    r_line.push('─');
                                }
                            }

                            println!("{} {deep_yellow}├{}{reset}", l_str, r_line);
                            continue;
                        }

                        let lv_trunc = if !lk.is_empty() {
                            let lv_chars: Vec<char> = lv.chars().collect();
                            if lv_chars.len() > col_width - 19 {
                                let max_len = col_width - 19 - 3;
                                let trunc: String =
                                    lv_chars[lv_chars.len() - max_len..].iter().collect();
                                format!("...{}", trunc)
                            } else {
                                lv.to_string()
                            }
                        } else {
                            lv.to_string()
                        };

                        let l_str = if lk.is_empty() {
                            if lv_trunc.is_empty() {
                                format!("{:<col_width$}", "", col_width = col_width)
                            } else {
                                let vis_len = lv_trunc.chars().count();
                                let pad = col_width.saturating_sub(vis_len);
                                format!("{}{}{reset}{}", l_color, lv_trunc, " ".repeat(pad))
                            }
                        } else {
                            let f = format!(" {:<18}{}{}{reset}", lk, l_color, lv_trunc);
                            let vis_len = 19 + lv_trunc.chars().count();
                            let pad = col_width.saturating_sub(vis_len);
                            format!("{}{}", f, " ".repeat(pad))
                        };

                        // Truncate right-side values so they don't overflow right_width.
                        // Key takes 19 chars (1 space + 18 padded label), leaving the rest for value.
                        let rv_max = right_width.saturating_sub(20);
                        let rv_trunc =
                            if !rk.is_empty() && rv.chars().count() > rv_max && rv_max > 3 {
                                let rv_chars: Vec<char> = rv.chars().collect();
                                let trunc: String =
                                    rv_chars[rv_chars.len() - (rv_max - 3)..].iter().collect();
                                format!("...{}", trunc)
                            } else {
                                rv.clone()
                            };

                        if rk.is_empty() && rv.is_empty() {
                            println!("{} {deep_yellow}│{reset}", l_str);
                        } else if rk.is_empty() {
                            let r_str = format!("{}{}{reset}", r_color, rv_trunc);
                            println!("{} {deep_yellow}│{reset} {}", l_str, r_str);
                        } else {
                            let r_str = format!(" {:<18}{}{}{reset}", rk, r_color, rv_trunc);
                            println!("{} {deep_yellow}│{reset} {}", l_str, r_str);
                        }
                    }

                    let mut r_bottom = String::with_capacity(right_width * 3);
                    for i in 0..right_width {
                        if i == 24 {
                            r_bottom.push('┴');
                        } else {
                            r_bottom.push('─');
                        }
                    }
                    let bottom_sep = format!("{:─<left$}┴{}", "", r_bottom, left = col_width + 1);
                    println!("{deep_yellow}{}{reset}", bottom_sep);
                    println!("{}Recent Commands{}", blood_red, reset);
                    if recent_commands.is_empty() {
                        println!("  (none)");
                    } else {
                        for c in recent_commands.iter() {
                            let approval_color = match c.approval.as_str() {
                                "approved" => "\x1b[32m",
                                "denied" => "\x1b[31m",
                                _ => bold_white,
                            };
                            let exit_color = match c.status.as_str() {
                                "succeeded" => "\x1b[32m",
                                "pending" => "\x1b[33m",
                                s if s.starts_with("failed") => "\x1b[31m",
                                _ => bold_white,
                            };

                            let approval_fmt =
                                format!("{}{:<10}{}", approval_color, c.approval, reset);
                            let exit_fmt = format!("{}{:<14}{}", exit_color, c.status, reset);
                            let mode_padded = format!("{:<19}", format!("[{}]", c.mode));

                            let mut cmd_disp = c.cmd.clone();
                            if cmd_disp.len() > 55 {
                                cmd_disp.truncate(52);
                                cmd_disp.push_str("...");
                            }

                            println!(
                                "  {} {} {} {} {}{}{}",
                                c.timestamp,
                                mode_padded,
                                approval_fmt,
                                exit_fmt,
                                bold_white,
                                cmd_disp,
                                reset
                            );
                        }
                    }
                }
                _ => {
                    println!("Daemon did not return status.");
                    std::process::exit(1);
                }
            }
        }
    }
    Ok(())
}
