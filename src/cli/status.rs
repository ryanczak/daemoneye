use anyhow::Result;
use std::collections::VecDeque;
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

                    let col_width = 49;

                    let title_left = format!("{}Process Information{}", blood_red, reset);
                    let title_right = format!("{}Session Information{}", blood_red, reset);

                    let left_pad = col_width - 19;
                    let left_pad_str = " ".repeat(left_pad);

                    println!(
                        "{}{}{deep_yellow}│{reset} {}",
                        title_left, left_pad_str, title_right
                    );
                    let sep_line = format!("{:─<width$}", "", width = col_width);
                    println!(
                        "{deep_yellow}{}┼──────────────────────────────────{reset}",
                        sep_line
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

                    let mut rows = Vec::new();
                    rows.push((
                        "PID:".to_string(),
                        pid.to_string(),
                        "Active sessions:".to_string(),
                        active_sessions.to_string(),
                    ));
                    rows.push((
                        "Uptime:".to_string(),
                        uptime_str,
                        "Active model:".to_string(),
                        format!("{}/{}", provider, model),
                    ));
                    rows.push((
                        "Socket:".to_string(),
                        socket_path,
                        "Token budget:".to_string(),
                        format!(
                            "{} / {} ({}%)",
                            active_prompt_tokens, context_window_tokens, tokens_pct
                        ),
                    ));

                    rows.push((
                        "─".to_string(),
                        "".to_string(),
                        "".to_string(),
                        "".to_string(),
                    ));

                    rows.push((
                        "Webhooks rec'd:".to_string(),
                        webhooks_received.to_string(),
                        "Circuit:".to_string(),
                        circuit_state.clone(),
                    ));
                    rows.push((
                        "Webhook URL:".to_string(),
                        webhook_url,
                        "".to_string(),
                        "".to_string(),
                    ));

                    rows.push((
                        "─".to_string(),
                        "".to_string(),
                        "".to_string(),
                        "".to_string(),
                    ));

                    rows.push((
                        "Commands (fg):".to_string(),
                        commands_fg_str,
                        "Memories:".to_string(),
                        format!(
                            "{} created, {} recalled, {} deleted",
                            memories_created, memories_recalled, memories_deleted
                        ),
                    ));

                    let mut mem_cats: Vec<_> = memory_breakdown.into_iter().collect();
                    mem_cats.sort_by_key(|k| k.0.clone());

                    let mut right_queue = VecDeque::new();
                    for (cat, count) in mem_cats {
                        right_queue.push_back((
                            "".to_string(),
                            format!("  - {:<12} {}", format!("{}:", cat), count),
                        ));
                    }

                    let (rk, rv) = right_queue
                        .pop_front()
                        .unwrap_or(("".to_string(), "".to_string()));
                    rows.push(("Commands (bg):".to_string(), commands_bg_str, rk, rv));

                    rows.push((
                        "─".to_string(),
                        "".to_string(),
                        "".to_string(),
                        "".to_string(),
                    ));

                    let (rk, rv) = right_queue
                        .pop_front()
                        .unwrap_or(("".to_string(), "".to_string()));
                    rows.push((
                        "Runbooks:".to_string(),
                        format!(
                            "{} created, {} executed, {} deleted",
                            runbooks_created, runbooks_executed, runbooks_deleted
                        ),
                        rk,
                        rv,
                    ));

                    let (rk, rv) = right_queue
                        .pop_front()
                        .unwrap_or(("".to_string(), "".to_string()));
                    rows.push((
                        "Scripts:".to_string(),
                        format!("{} created, {} executed", scripts_created, scripts_executed),
                        rk,
                        rv,
                    ));

                    let (rk, rv) = right_queue
                        .pop_front()
                        .unwrap_or(("".to_string(), "".to_string()));
                    rows.push((
                        "Schedules:".to_string(),
                        format!(
                            "{} created, {} executed, {} deleted",
                            schedules_created, schedules_executed, schedules_deleted
                        ),
                        rk,
                        rv,
                    ));

                    while let Some((rk, rv)) = right_queue.pop_front() {
                        rows.push(("".to_string(), "".to_string(), rk, rv));
                    }

                    for (lk, lv, rk, rv) in rows {
                        if lk == "─" {
                            println!(
                                "{deep_yellow}{:─<col_width$}┼──────────────────────────────────{reset}",
                                "",
                                col_width = col_width
                            );
                            continue;
                        }

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
                    let bottom_sep = format!("{:─<width$}", "", width = col_width + 35);
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
