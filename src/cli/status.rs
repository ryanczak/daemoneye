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
                    provider,
                    model,
                    socket_path,
                    schedule_count,
                    circuit_state,
                    circuit_failures,
                    commands_fg_succeeded,
                    commands_fg_failed,
                    commands_bg_succeeded,
                    commands_bg_failed,
                    commands_sched_succeeded,
                    commands_sched_failed,
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

                    let commands_fg_total = commands_fg_succeeded + commands_fg_failed;
                    let commands_fg_str = format!(
                        "{} ({} ok, {} fail)",
                        commands_fg_total, commands_fg_succeeded, commands_fg_failed
                    );

                    let commands_bg_total = commands_bg_succeeded + commands_bg_failed;
                    let commands_bg_str = format!(
                        "{} ({} ok, {} fail)",
                        commands_bg_total, commands_bg_succeeded, commands_bg_failed
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
                    left_items.push(("Runbooks:".to_string(), format!("{} existing", runbook_count)));
                    left_items.push((
                        "".to_string(),
                        format!(
                            "  {} created, {} executed, {} deleted",
                            runbooks_created, runbooks_executed, runbooks_deleted
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
                    left_items.push(("Schedules:".to_string(), format!("{} active", schedule_count)));
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
                    right_items.push((
                        "Circuit:".to_string(),
                        format!("{} ({} failures)", circuit_state, circuit_failures),
                    ));
                    right_items.push((
                        "Token budget:".to_string(),
                        format!(
                            "{} / {} ({}%)",
                            active_prompt_tokens, context_window_tokens, tokens_pct
                        ),
                    ));
                    right_items.push(("─".to_string(), "".to_string()));
                    right_items.push(("§".to_string(), "Memories".to_string()));
                    let knowledge_count = memory_breakdown.get("knowledge").copied().unwrap_or(0);
                    let session_count = memory_breakdown.get("session").copied().unwrap_or(0);
                    right_items.push(("Knowledge:".to_string(), knowledge_count.to_string()));
                    right_items.push(("Session:".to_string(), session_count.to_string()));
                    right_items.push((
                        "".to_string(),
                        format!(
                            "  {} created, {} recalled, {} deleted",
                            memories_created, memories_recalled, memories_deleted
                        ),
                    ));

                    let mut mem_cats: Vec<_> = memory_breakdown
                        .into_iter()
                        .filter(|(cat, _)| cat != "knowledge" && cat != "session")
                        .collect();
                    mem_cats.sort_by_key(|k| k.0.clone());
                    for (cat, count) in mem_cats {
                        right_items.push((
                            format!("  - {}:", cat),
                            count.to_string(),
                        ));
                    }

                    right_items.push(("─".to_string(), "".to_string()));
                    right_items.push(("§".to_string(), "Redactions".to_string()));
                    let mut redact_sorted: Vec<_> = redaction_counts.into_iter().collect();
                    redact_sorted.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
                    for (rtype, count) in redact_sorted {
                        right_items.push((format!("{}:", rtype), count.to_string()));
                    }

                    let col_width = left_items
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
                    let right_width = 44;

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
                        let r_color = if rk == "Circuit:" {
                            match circuit_state.as_str() {
                                "closed" => "\x1b[32m",
                                "open" => "\x1b[31m",
                                _ => "\x1b[33m",
                            }
                        } else {
                            bold_white
                        };

                        // Blood-red section header sentinel
                        if lk == "§" || rk == "§" {
                            let l_str = if lk == "§" {
                                let vis_len = lv.chars().count();
                                let pad = if vis_len < col_width { col_width - vis_len } else { 0 };
                                format!("{}{}{reset}{}", blood_red, lv, " ".repeat(pad))
                            } else {
                                // normal left side
                                let lv_chars: Vec<char> = lv.chars().collect();
                                let lv_trunc = if lv_chars.len() > col_width - 19 {
                                    let max_len = col_width - 19 - 3;
                                    let trunc: String = lv_chars[lv_chars.len() - max_len..].iter().collect();
                                    format!("...{}", trunc)
                                } else { lv.to_string() };
                                if lk.is_empty() {
                                    let vis_len = lv_trunc.chars().count();
                                    let pad = if vis_len < col_width { col_width - vis_len } else { 0 };
                                    format!("{}{}{reset}{}", l_color, lv_trunc, " ".repeat(pad))
                                } else {
                                    let f = format!(" {:<18}{}{}{reset}", lk, l_color, lv_trunc);
                                    let vis_len = 19 + lv_trunc.chars().count();
                                    let pad = if vis_len < col_width { col_width - vis_len } else { 0 };
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
                                    l_str, "", right_width = right_width
                                );
                            } else if rv.is_empty() && rk != "§" {
                                println!("{} {deep_yellow}│{reset}", l_str);
                            } else {
                                println!("{} {deep_yellow}│{reset} {}", l_str, r_str);
                            }
                            continue;
                        }

                        if lk == "─" && rk == "─" {
                            println!(
                                "{deep_yellow}{:─<col_width$}┼{:─<right_width$}{reset}",
                                "",
                                "",
                                col_width = col_width + 1,
                                right_width = right_width
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
                        } else if rk == "─" {
                            let lv_chars: Vec<char> = lv.chars().collect();
                            let lv_trunc = if lv_chars.len() > col_width - 19 {
                                let max_len = col_width - 19 - 3;
                                let trunc: String =
                                    lv_chars[lv_chars.len() - max_len..].iter().collect();
                                format!("...{}", trunc)
                            } else {
                                lv.to_string()
                            };

                            let l_str = if lk.is_empty() {
                                format!("{:<col_width$}", "", col_width = col_width)
                            } else {
                                let f = format!(" {:<18}{}{}{reset}", lk, l_color, lv_trunc);
                                let vis_len = 19 + lv_trunc.chars().count();
                                let pad = if vis_len < col_width {
                                    col_width - vis_len
                                } else {
                                    0
                                };
                                format!("{}{}", f, " ".repeat(pad))
                            };
                            println!(
                                "{} {deep_yellow}├{:─<right_width$}{reset}",
                                l_str,
                                "",
                                right_width = right_width
                            );
                            continue;
                        }

                        let lv_chars: Vec<char> = lv.chars().collect();
                        let lv_trunc = if lv_chars.len() > col_width - 19 {
                            let max_len = col_width - 19 - 3;
                            let trunc: String =
                                lv_chars[lv_chars.len() - max_len..].iter().collect();
                            format!("...{}", trunc)
                        } else {
                            lv.to_string()
                        };

                        let l_str = if lk.is_empty() {
                            if lv_trunc.is_empty() {
                                format!("{:<col_width$}", "", col_width = col_width)
                            } else {
                                let vis_len = lv_trunc.chars().count();
                                let pad = if vis_len < col_width { col_width - vis_len } else { 0 };
                                format!("{}{}{reset}{}", l_color, lv_trunc, " ".repeat(pad))
                            }
                        } else {
                            let f = format!(" {:<18}{}{}{reset}", lk, l_color, lv_trunc);
                            let vis_len = 19 + lv_trunc.chars().count();
                            let pad = if vis_len < col_width {
                                col_width - vis_len
                            } else {
                                0
                            };
                            format!("{}{}", f, " ".repeat(pad))
                        };

                        if rk.is_empty() && rv.is_empty() {
                            println!("{} {deep_yellow}│{reset}", l_str);
                        } else if rk.is_empty() {
                            let r_str = format!("{}{}{reset}", r_color, rv);
                            println!("{} {deep_yellow}│{reset} {}", l_str, r_str);
                        } else {
                            let r_str = format!(" {:<18}{}{}{reset}", rk, r_color, rv);
                            println!("{} {deep_yellow}│{reset} {}", l_str, r_str);
                        }
                    }

                    println!();
                    println!("{}Recent Commands{}", blood_red, reset);
                    let bottom_sep =
                        format!("{:─<width$}", "", width = col_width + right_width + 3);
                    println!("{deep_yellow}{}{reset}", bottom_sep);
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

                            let approval_fmt = format!(
                                "{}{:<10}{}",
                                approval_color, c.approval, reset
                            );
                            let exit_fmt = format!(
                                "{}{:<14}{}",
                                exit_color, c.status, reset
                            );
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
