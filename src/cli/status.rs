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
                    commands_executed,
                    webhooks_received,
                    runbooks_count,
                    scripts_count,
                    memory_items_count,
                    recent_commands,
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

                    let title_left = format!("{}Daemon Stats{}", blood_red, reset);
                    let title_right = format!("{}Ecosystem Stats{}", blood_red, reset);
                    // The left title contains ANSI escapes (15 invisible chars + 12 visible = 27 chars)
                    // We want 44 visible characters on the left. So we need 32 spaces.
                    let left_pad = col_width - 12; // 12 is length of "Daemon Stats"
                    let left_pad_str = " ".repeat(left_pad);

                    println!(
                        "{}{}{deep_yellow}|{reset} {}",
                        title_left, left_pad_str, title_right
                    );
                    let sep_line = format!("{:-<width$}", "", width = col_width);
                    println!(
                        "{deep_yellow}{}-+----------------------------------{reset}",
                        sep_line
                    );

                    let left_keys = [
                        "PID:",
                        "Uptime:",
                        "Socket:",
                        "Provider:",
                        "Active sessions:",
                        "Commands exec'd:",
                        "Webhooks rec'd:",
                        "Circuit:",
                    ];
                    let left_vals = [
                        pid.to_string(),
                        uptime_str,
                        socket_path,
                        format!("{}/{}", provider, model),
                        active_sessions.to_string(),
                        commands_executed.to_string(),
                        webhooks_received.to_string(),
                        circuit_state.clone(),
                    ];

                    let right_keys = [
                        "Runbooks:",
                        "Scripts:",
                        "Memories:",
                        "Schedules:",
                        "",
                        "",
                        "",
                        "",
                    ];
                    let right_vals = [
                        runbooks_count.to_string(),
                        scripts_count.to_string(),
                        memory_items_count.to_string(),
                        schedule_count.to_string(),
                        "".to_string(),
                        "".to_string(),
                        "".to_string(),
                        "".to_string(),
                    ];

                    for i in 0..8 {
                        let lk = left_keys[i];
                        let lv = &left_vals[i];
                        let rk = right_keys[i];
                        let rv = &right_vals[i];

                        let l_color = if lk == "Circuit:" {
                            match lv.as_str() {
                                "closed" => "\x1b[32m",
                                "open" => "\x1b[31m",
                                _ => "\x1b[33m",
                            }
                        } else {
                            bold_white
                        };

                        let r_color = bold_white;

                        let lv_chars: Vec<char> = lv.chars().collect();
                        let lv_trunc = if lv_chars.len() > col_width - 17 {
                            let max_len = col_width - 17 - 3;
                            let trunc: String =
                                lv_chars[lv_chars.len() - max_len..].iter().collect();
                            format!("...{}", trunc)
                        } else {
                            lv.to_string()
                        };

                        let l_str = format!("{:<16} {}{}{reset}", lk, l_color, lv_trunc);
                        let l_vis_len = 17 + lv_trunc.chars().count();
                        let l_pad = if l_vis_len < col_width {
                            col_width - l_vis_len
                        } else {
                            0
                        };
                        let l_pad_str = " ".repeat(l_pad);

                        if rk.is_empty() {
                            println!("{l_str}{l_pad_str} {deep_yellow}|{reset}");
                        } else {
                            let r_str = format!("{:<16} {}{}{reset}", rk, r_color, rv);
                            println!("{l_str}{l_pad_str} {deep_yellow}|{reset} {r_str}");
                        }
                    }

                    println!();
                    println!("{}Recent Commands{}", blood_red, reset);
                    let bottom_sep = format!("{:-<width$}", "", width = col_width + 35);
                    println!("{deep_yellow}{}{reset}", bottom_sep);
                    if recent_commands.is_empty() {
                        println!("  (none)");
                    } else {
                        for (i, cmd) in recent_commands.iter().enumerate() {
                            println!("  {}. {}{}{}", i + 1, bold_white, cmd, reset);
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
