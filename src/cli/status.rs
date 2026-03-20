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
                    schedule_count: _,
                    circuit_state,
                    commands_fg_succeeded,
                    commands_fg_failed,
                    commands_bg_succeeded,
                    commands_bg_failed,
                    webhooks_received,
                    webhook_url,
                    runbooks_created,
                    runbooks_executed,
                    runbooks_deleted,
                    scripts_created,
                    scripts_executed,
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

                    let col_width = 62;
                    let right_width = 44;

                    let title_left = format!("{}Process Information{}", blood_red, reset);
                    let title_right = format!("{}Session Information{}", blood_red, reset);

                    let left_pad = col_width - 19;
                    let left_pad_str = " ".repeat(left_pad);

                    println!(
                        "{}{} {deep_yellow}│{reset} {}",
                        title_left, left_pad_str, title_right
                    );
                    println!(
                        "{deep_yellow}{:─<col_width$}┼{:─<right_width$}{reset}",
                        "",
                        "",
                        col_width = col_width + 1,
                        right_width = right_width
                    );

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
                    left_items.push(("Webhook URL:".to_string(), webhook_url));
                    left_items.push(("Webhooks rec'd:".to_string(), webhooks_received.to_string()));
                    left_items.push(("─".to_string(), "".to_string()));
                    left_items.push(("Commands (fg):".to_string(), commands_fg_str));
                    left_items.push(("Commands (bg):".to_string(), commands_bg_str));
                    left_items.push(("─".to_string(), "".to_string()));
                    left_items.push((
                        "Runbooks:".to_string(),
                        format!(
                            "{} created, {} executed, {} deleted",
                            runbooks_created, runbooks_executed, runbooks_deleted
                        ),
                    ));
                    left_items.push((
                        "Scripts:".to_string(),
                        format!("{} created, {} executed", scripts_created, scripts_executed),
                    ));
                    left_items.push((
                        "Schedules:".to_string(),
                        format!(
                            "{} created, {} executed, {} deleted",
                            schedules_created, schedules_executed, schedules_deleted
                        ),
                    ));

                    let mut right_items = Vec::new();
                    right_items.push(("Active sessions:".to_string(), active_sessions.to_string()));
                    right_items.push((
                        "Active model:".to_string(),
                        format!("{}/{}", provider, model),
                    ));
                    right_items.push(("Circuit:".to_string(), circuit_state.clone()));
                    right_items.push((
                        "Token budget:".to_string(),
                        format!(
                            "{} / {} ({}%)",
                            active_prompt_tokens, context_window_tokens, tokens_pct
                        ),
                    ));
                    right_items.push(("─".to_string(), "".to_string()));
                    right_items.push((
                        "Memories:".to_string(),
                        format!(
                            "{} created, {} recalled, {} deleted",
                            memories_created, memories_recalled, memories_deleted
                        ),
                    ));

                    let mut mem_cats: Vec<_> = memory_breakdown.into_iter().collect();
                    mem_cats.sort_by_key(|k| k.0.clone());
                    for (cat, count) in mem_cats {
                        right_items.push((
                            "".to_string(),
                            format!("  - {:<12} {}", format!("{}:", cat), count),
                        ));
                    }

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
                            match rv.as_str() {
                                "closed" => "\x1b[32m",
                                "open" => "\x1b[31m",
                                _ => "\x1b[33m",
                            }
                        } else {
                            bold_white
                        };

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
                                format!("{:<17} {}{}{reset}", rk, r_color, rv)
                            };
                            println!(
                                "{deep_yellow}{:─<col_width$}┼{reset} {}",
                                "",
                                r_str,
                                col_width = col_width + 1
                            );
                            continue;
                        } else if rk == "─" {
                            let lv_chars: Vec<char> = lv.chars().collect();
                            let lv_trunc = if lv_chars.len() > col_width - 18 {
                                let max_len = col_width - 18 - 3;
                                let trunc: String =
                                    lv_chars[lv_chars.len() - max_len..].iter().collect();
                                format!("...{}", trunc)
                            } else {
                                lv.to_string()
                            };

                            let l_str = if lk.is_empty() {
                                format!("{:<col_width$}", "", col_width = col_width)
                            } else {
                                let f = format!("{:<17} {}{}{reset}", lk, l_color, lv_trunc);
                                let vis_len = 18 + lv_trunc.chars().count();
                                let pad = if vis_len < col_width {
                                    col_width - vis_len
                                } else {
                                    0
                                };
                                format!("{}{}", f, " ".repeat(pad))
                            };
                            println!(
                                "{} {deep_yellow}│{:─<right_width$}{reset}",
                                l_str,
                                "",
                                right_width = right_width
                            );
                            continue;
                        }

                        let lv_chars: Vec<char> = lv.chars().collect();
                        let lv_trunc = if lv_chars.len() > col_width - 18 {
                            let max_len = col_width - 18 - 3;
                            let trunc: String =
                                lv_chars[lv_chars.len() - max_len..].iter().collect();
                            format!("...{}", trunc)
                        } else {
                            lv.to_string()
                        };

                        let l_str = if lk.is_empty() {
                            format!("{:<col_width$}", "", col_width = col_width)
                        } else {
                            let f = format!("{:<17} {}{}{reset}", lk, l_color, lv_trunc);
                            let vis_len = 18 + lv_trunc.chars().count();
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
                            let r_str = format!("{:<17} {}{}{reset}", rk, r_color, rv);
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
                            let status_color = match c.status.as_str() {
                                "succeeded" => "\x1b[32m",
                                "approved" => "\x1b[33m",
                                s if s.starts_with("failed") => "\x1b[31m",
                                _ => bold_white,
                            };

                            let status_padded = format!("{:<14}", c.status);
                            let status_fmt = format!("{}{}{}", status_color, status_padded, reset);
                            let mode_padded = format!("{:<18}", format!("[{}]", c.mode));

                            let mut cmd_disp = c.cmd.clone();
                            if cmd_disp.len() > 60 {
                                cmd_disp.truncate(57);
                                cmd_disp.push_str("...");
                            }

                            println!(
                                "  {} {} {} {}{}{}",
                                c.timestamp, mode_padded, status_fmt, bold_white, cmd_disp, reset
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
