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
                    commands_succeeded,
                    commands_failed,
                    webhooks_received,
                    runbooks_created,
                    runbooks_executed,
                    scripts_created,
                    scripts_executed,
                    memories_created,
                    memories_recalled,
                    schedules_created,
                    schedules_executed,
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

                    let col_width = 44;

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

                    let commands_total = commands_succeeded + commands_failed;
                    let commands_str = format!(
                        "{} ({} ok, {} fail)",
                        commands_total, commands_succeeded, commands_failed
                    );

                    let tokens_pct = if context_window_tokens > 0 {
                        ((active_prompt_tokens as f64 / context_window_tokens as f64) * 100.0)
                            as u32
                    } else {
                        0
                    };

                    let left_keys = vec![
                        "PID:",
                        "Uptime:",
                        "Socket:",
                        "Webhooks rec'd:",
                        "Commands exec'd:",
                        "Runbooks:",
                        "Scripts:",
                        "Schedules:",
                    ];
                    let left_vals = vec![
                        pid.to_string(),
                        uptime_str,
                        socket_path,
                        webhooks_received.to_string(),
                        commands_str,
                        format!(
                            "{} created, {} executed",
                            runbooks_created, runbooks_executed
                        ),
                        format!("{} created, {} executed", scripts_created, scripts_executed),
                        format!(
                            "{} created, {} executed",
                            schedules_created, schedules_executed
                        ),
                    ];

                    let mut right_keys = vec![
                        "Active sessions:",
                        "Active model:",
                        "Token budget:",
                        "Circuit:",
                    ];
                    let mut right_vals = vec![
                        active_sessions.to_string(),
                        format!("{}/{}", provider, model),
                        format!(
                            "{} / {} ({}%)",
                            active_prompt_tokens, context_window_tokens, tokens_pct
                        ),
                        circuit_state.clone(),
                    ];

                    right_keys.push("");
                    right_vals.push("".to_string());
                    right_keys.push("Memories:");
                    right_vals.push(format!(
                        "{} created, {} recalled",
                        memories_created, memories_recalled
                    ));

                    for (cat, count) in &memory_breakdown {
                        right_keys.push("");
                        let cat_fmt = format!("  - {}:", cat);
                        right_vals.push(format!("{:<15} {}", cat_fmt, count));
                    }

                    let rows = left_keys.len().max(right_keys.len());

                    for i in 0..rows {
                        let lk = left_keys.get(i).cloned().unwrap_or("");
                        let lv = left_vals.get(i).cloned().unwrap_or_default();
                        let rk = right_keys.get(i).cloned().unwrap_or("");
                        let rv = right_vals.get(i).cloned().unwrap_or_default();

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
                            format!("{:<col_width$}", "")
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
                            let exit_str = match c.exit_code {
                                Some(0) => format!("\x1b[32m[0]\x1b[0m"),
                                Some(code) => format!("\x1b[31m[{}]\x1b[0m", code),
                                None => format!("\x1b[33m[?]\x1b[0m"),
                            };
                            let time_str = match c.runtime_ms {
                                Some(ms) => format!("{}ms", ms),
                                None => "-".to_string(),
                            };

                            let mut cmd_disp = c.cmd.clone();
                            if cmd_disp.len() > 60 {
                                cmd_disp.truncate(57);
                                cmd_disp.push_str("...");
                            }

                            println!(
                                "  {} {:>5} {:>7}  {}{}{}",
                                c.timestamp, exit_str, time_str, bold_white, cmd_disp, reset
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
