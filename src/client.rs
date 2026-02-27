use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};

use crate::config::Config;
use crate::ipc::{Request, Response, DEFAULT_SOCKET_PATH};

/// A persistent async stdin reader owned for the lifetime of a chat session.
/// Using one reader for both the main query loop and tool-call approvals
/// guarantees a single internal buffer — no bytes get lost between the two
/// call sites.
type ChatStdin = tokio::io::Lines<tokio::io::BufReader<tokio::io::Stdin>>;

pub fn run_setup() -> Result<()> {
    // Write the systemd user service file.
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    let systemd_dir = PathBuf::from(&home).join(".config/systemd/user");
    let service_path = systemd_dir.join("t1000.service");

    let service_content = "\
[Unit]
Description=T1000 AI Tmux Daemon
After=network.target

[Service]
Type=simple
ExecStart=%h/.cargo/bin/t1000 daemon
ExecStop=%h/.cargo/bin/t1000 stop
Restart=on-failure
RestartSec=5
Environment=\"PATH=%h/.cargo/bin:/usr/local/bin:/usr/bin:/bin\"

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
            println!("systemctl --user enable --now t1000");
            println!();
            println!("# Check status and view logs:");
            println!("systemctl --user status t1000");
            println!("t1000 logs");
        }
        Err(e) => {
            eprintln!("Warning: could not write service file: {}", e);
            eprintln!("You can install it manually:");
            eprintln!("  mkdir -p ~/.config/systemd/user");
            eprintln!("  cp t1000.service ~/.config/systemd/user/");
        }
    }

    let position = Config::load()
        .unwrap_or_default()
        .ai
        .position;
    let split_flag = match position.as_str() {
        "right"  => "-h",
        "left"   => "-bh",
        "top"    => "-bv",
        _        => "-v",   // "bottom" or any unrecognised value
    };

    // Use the absolute path to the running binary so the bind-key works even
    // when ~/.cargo/bin is not in the PATH inherited by the tmux session (a
    // common issue when the daemon created the session from a background
    // process or service with a minimal environment).
    let t1000_bin = std::env::current_exe()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "t1000".to_string());

    println!();
    println!("# Add this to your ~/.tmux.conf:");
    println!(
        "bind-key T split-window {} -e \"T1000_SOURCE_PANE=#{{pane_id}}\" '{} chat'",
        split_flag, t1000_bin
    );
    println!();
    println!("# Then reload tmux config:");
    println!("tmux source-file ~/.tmux.conf");
    println!();
    println!("# If you already have a bind-key that uses the bare name 't1000',");
    println!("# replace it with the full path above — the tmux session may not");
    println!("# inherit ~/.cargo/bin in its PATH.");

    Ok(())
}

pub fn run_logs(path: PathBuf) -> Result<()> {
    if !path.exists() {
        eprintln!("No log file found at {}.", path.display());
        eprintln!("The daemon writes logs there by default when started with: t1000 daemon");
        std::process::exit(1);
    }
    use std::os::unix::process::CommandExt;
    let err = std::process::Command::new("tail")
        .args(["-f", path.to_str().unwrap_or("")])
        .exec();
    anyhow::bail!("Failed to exec tail: {}", err)
}

pub async fn run_stop() -> Result<()> {
    match connect().await {
        Err(_) => {
            println!("Daemon is not running.");
            std::process::exit(1);
        }
        Ok(stream) => {
            let (rx, mut tx) = stream.into_split();
            let mut rx = BufReader::new(rx);
            send_request(&mut tx, Request::Shutdown).await?;
            match recv(&mut rx).await {
                Ok(Response::Ok) => println!("Daemon stopped."),
                _ => {
                    println!("Daemon did not respond to shutdown.");
                    std::process::exit(1);
                }
            }
        }
    }
    Ok(())
}

pub async fn run_ping() -> Result<()> {
    match connect().await {
        Err(_) => {
            println!("Daemon is not running.");
            std::process::exit(1);
        }
        Ok(stream) => {
            let (rx, mut tx) = stream.into_split();
            let mut rx = BufReader::new(rx);
            send_request(&mut tx, Request::Ping).await?;
            match recv(&mut rx).await {
                Ok(Response::Ok) => println!("Daemon is running."),
                _ => {
                    println!("Daemon is not responding.");
                    std::process::exit(1);
                }
            }
        }
    }
    Ok(())
}

pub async fn run_ask(query: String) -> Result<()> {
    // Single-shot: create a fresh stdin reader just for any tool-call approvals.
    let mut stdin = tokio::io::BufReader::new(tokio::io::stdin()).lines();
    ask_with_session(query, None, &mut stdin).await
}

pub async fn run_chat() -> Result<()> {
    let result = run_chat_inner().await;
    if let Err(ref e) = result {
        // The ChatStdin async reader has been dropped by now, so using the
        // synchronous stdin here is safe.
        use std::io::Write;
        eprintln!("\n\x1b[31m✗\x1b[0m t1000 error: {}", e);
        eprint!("\x1b[2mPress Enter to close this pane…\x1b[0m");
        std::io::stderr().flush().ok();
        let _ = std::io::stdin().read_line(&mut String::new());
    }
    result
}

async fn run_chat_inner() -> Result<()> {
    use std::io::Write;

    let session_id = new_session_id();

    // One async stdin reader shared by the main query loop AND any tool-call
    // approval prompts inside ask_with_session. Sharing a single BufReader
    // means there is only one internal buffer — no bytes can silently disappear
    // between the two readers.
    let mut stdin = tokio::io::BufReader::new(tokio::io::stdin()).lines();

    // Header box — round corners, bright cyan, total visible width = 48.
    // Inner content width = 46 chars.
    let d46 = "─".repeat(46);
    let d27 = "─".repeat(27);
    let sp14 = " ".repeat(14);
    println!("\x1b[1m\x1b[96m╭─ T1000  AI  Agent {d27}╮\x1b[0m");
    println!("\x1b[1m\x1b[96m│\x1b[0m  Type '\x1b[1mexit\x1b[0m' or \x1b[1mCtrl-C\x1b[0m to close{sp14}\x1b[1m\x1b[96m│\x1b[0m");
    println!("\x1b[1m\x1b[96m╰{d46}╯\x1b[0m");
    println!();

    // Send an automatic opening message so the AI greets the user immediately
    // rather than waiting for them to type first.
    if let Err(e) = ask_with_session("Hello!".to_string(), Some(&session_id), &mut stdin).await {
        eprintln!("\x1b[31m✗\x1b[0m Could not reach the daemon: {}", e);
        eprintln!("  Make sure it is running:  \x1b[1mt1000 daemon --console\x1b[0m");
        eprintln!("  \x1b[2mWaiting for your input…\x1b[0m");
    }

    loop {
        print!("\n\x1b[92m❯\x1b[0m ");
        std::io::stdout().flush()?;

        match stdin.next_line().await? {
            None => break, // EOF / pane closed
            Some(line) => {
                let query = line.trim().to_string();
                if query.is_empty() { continue; }
                if query == "exit" || query == "quit" { break; }
                if let Err(e) = ask_with_session(query, Some(&session_id), &mut stdin).await {
                    eprintln!("\n\x1b[31m✗\x1b[0m {}", e);
                }
            }
        }
    }

    println!("\n\x1b[2mGoodbye.\x1b[0m");
    Ok(())
}

async fn ask_with_session(query: String, session_id: Option<&str>, stdin: &mut ChatStdin) -> Result<()> {
    use std::io::Write;
    use std::time::Duration;

    let stream = connect().await?;
    let (rx, mut tx) = stream.into_split();
    let mut rx = BufReader::new(rx);

    // T1000_SOURCE_PANE is set by the recommended tmux bind-key:
    //   split-window -h -e "T1000_SOURCE_PANE=#{pane_id}" 't1000 chat'
    // It records the user's working pane before the split so the daemon
    // captures context from — and injects commands into — the right pane.
    // Falls back to TMUX_PANE, which is correct when `t1000 chat` or
    // `t1000 ask` is run directly from the user's working pane.
    let tmux_pane = std::env::var("T1000_SOURCE_PANE")
        .ok()
        .or_else(|| std::env::var("TMUX_PANE").ok());
    send_request(&mut tx, Request::Ask {
        query,
        tmux_pane,
        session_id: session_id.map(|s| s.to_string()),
    }).await?;

    // Braille-pattern spinner frames, updated every 80 ms while waiting for
    // the first response from the daemon.
    const SPINNER: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
    let mut spin = 0usize;
    let mut response_started = false;

    loop {
        // Phase 1 — waiting for the first content: poll recv() with a short
        // timeout so we can animate the spinner between each check.
        let msg = if !response_started {
            loop {
                match tokio::time::timeout(Duration::from_millis(80), recv(&mut rx)).await {
                    Err(_timeout) => {
                        print!("\r\x1b[36m{}\x1b[0m \x1b[2mThinking…\x1b[0m", SPINNER[spin]);
                        std::io::stdout().flush()?;
                        spin = (spin + 1) % SPINNER.len();
                    }
                    Ok(r) => break r?,
                }
            }
        } else {
            // Phase 2 — streaming: wait directly without spinning.
            recv(&mut rx).await?
        };

        match msg {
            Response::Ok => {
                println!();
                break;
            }
            Response::Error(e) => {
                if !response_started {
                    print!("\r\x1b[K"); // erase spinner line
                }
                eprintln!("\n\x1b[31m✗\x1b[0m {}", e);
                break;
            }
            Response::SessionInfo { .. } => {
                // Silently consumed — spinner continues until the first token.
            }
            Response::Token(t) => {
                if !response_started {
                    print!("\r\x1b[K"); // erase spinner line
                    response_started = true;
                }
                print!("{}", t);
                std::io::stdout().flush()?;
            }
            Response::ToolCallPrompt { id, command, background } => {
                if !response_started {
                    print!("\r\x1b[K");
                    response_started = true;
                }
                let where_label = if background {
                    "daemon · runs silently"
                } else {
                    "terminal · visible to you"
                };
                println!("\n\n\x1b[33m⚙\x1b[0m  \x1b[1m$\x1b[0m {}  \x1b[2m({})\x1b[0m",
                    command, where_label);
                print!("   \x1b[32mApprove?\x1b[0m [y/N] \x1b[32m›\x1b[0m ");
                std::io::stdout().flush()?;
                // Use the shared async stdin reader so the buffer stays
                // consistent with the main query loop — no bytes dropped.
                let input = stdin.next_line().await?.unwrap_or_default();
                let approved = input.trim().eq_ignore_ascii_case("y");
                if approved {
                    println!("   \x1b[32m✓ approved\x1b[0m");
                } else {
                    println!("   \x1b[2m✗ skipped\x1b[0m");
                }
                send_request(&mut tx, Request::ToolCallResponse { id, approved }).await?;
            }
            Response::SudoPrompt { id, command } => {
                println!("\n\x1b[33m⚠\x1b[0m  \x1b[1msudo required\x1b[0m  \x1b[1m$\x1b[0m {}", command);
                // read_password_silent uses synchronous stdin with echo disabled.
                // The async BufReader hasn't consumed any bytes yet at this point
                // because the user hasn't typed anything since the last prompt.
                let password = read_password_silent("   \x1b[33mPassword:\x1b[0m ").unwrap_or_default();
                send_request(&mut tx, Request::SudoPassword { id, password }).await?;
            }
        }
    }

    Ok(())
}

/// Read a password from stdin with terminal echo disabled so it is not shown.
fn read_password_silent(prompt: &str) -> anyhow::Result<String> {
    use std::io::{BufRead, Write};
    print!("{}", prompt);
    std::io::stdout().flush()?;

    let fd = libc::STDIN_FILENO;
    let mut old: libc::termios = unsafe { std::mem::zeroed() };
    let termios_ok = unsafe { libc::tcgetattr(fd, &mut old) } == 0;

    if termios_ok {
        let mut new = old;
        new.c_lflag &= !(libc::ECHO | libc::ECHOE | libc::ECHOK | libc::ECHONL);
        unsafe { libc::tcsetattr(fd, libc::TCSANOW, &new) };
    }

    let mut input = String::new();
    let result = std::io::stdin().lock().read_line(&mut input);

    if termios_ok {
        unsafe { libc::tcsetattr(fd, libc::TCSANOW, &old) };
    }
    println!(); // newline after silent input
    result?;
    Ok(input.trim_end_matches('\n').trim_end_matches('\r').to_string())
}

/// Generate a random session ID from /dev/urandom.
fn new_session_id() -> String {
    let mut bytes = [0u8; 8];
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        use std::io::Read;
        let _ = f.read_exact(&mut bytes);
    }
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

async fn connect() -> Result<UnixStream> {
    let socket_path = Path::new(DEFAULT_SOCKET_PATH);
    UnixStream::connect(socket_path)
        .await
        .with_context(|| format!("Failed to connect to daemon at {}", DEFAULT_SOCKET_PATH))
}

async fn send_request(tx: &mut OwnedWriteHalf, req: Request) -> Result<()> {
    let mut data = serde_json::to_vec(&req)?;
    data.push(b'\n');
    tx.write_all(&data).await?;
    Ok(())
}

async fn recv(rx: &mut BufReader<OwnedReadHalf>) -> Result<Response> {
    let mut line = String::new();
    let n = rx.read_line(&mut line).await?;
    if n == 0 {
        anyhow::bail!("Daemon closed connection unexpectedly.");
    }
    let response: Response = serde_json::from_str(line.trim())?;
    Ok(response)
}
