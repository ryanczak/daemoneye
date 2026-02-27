use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};

use crate::config::Config;
use crate::ipc::{Request, Response, DEFAULT_SOCKET_PATH};

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

    println!();
    println!("# Add this to your ~/.tmux.conf:");
    println!("bind-key T split-window {} 't1000 chat'", split_flag);
    println!();
    println!("# Then reload tmux config:");
    println!("tmux source-file ~/.tmux.conf");

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
    ask_with_session(query, None).await
}

pub async fn run_chat() -> Result<()> {
    let session_id = new_session_id();
    println!("T1000 AI Agent Connected. Type 'exit' to quit.");

    loop {
        print!("\n> ");
        use std::io::Write;
        std::io::stdout().flush()?;

        let mut query = String::new();
        std::io::stdin().read_line(&mut query)?;
        let query = query.trim().to_string();

        if query.is_empty() { continue; }
        if query == "exit" || query == "quit" { break; }

        if let Err(e) = ask_with_session(query, Some(&session_id)).await {
            eprintln!("[Connection error]: {}", e);
        }
    }

    Ok(())
}

async fn ask_with_session(query: String, session_id: Option<&str>) -> Result<()> {
    let stream = connect().await?;
    let (rx, mut tx) = stream.into_split();
    let mut rx = BufReader::new(rx);

    let tmux_pane = std::env::var("TMUX_PANE").ok();
    send_request(&mut tx, Request::Ask {
        query,
        tmux_pane,
        session_id: session_id.map(|s| s.to_string()),
    }).await?;

    // For single-shot `ask` commands show a progress indicator while waiting
    // for the first token.  For `chat` the SessionInfo line already provides
    // immediate feedback, so skip it there.
    let show_thinking = session_id.is_none();
    if show_thinking {
        use std::io::Write;
        print!("Thinking...");
        std::io::stdout().flush()?;
    }
    let mut first_token = true;

    loop {
        match recv(&mut rx).await? {
            Response::Ok => {
                println!();
                break;
            }
            Response::Error(e) => {
                if show_thinking && first_token {
                    // Clear "Thinking..." before printing the error.
                    use std::io::Write;
                    print!("\r             \r");
                    std::io::stdout().flush()?;
                }
                eprintln!("\n[Error]: {}", e);
                break;
            }
            Response::SessionInfo { message_count } => {
                // Only show for chat sessions (session_id is Some).
                if session_id.is_some() {
                    if message_count == 0 {
                        println!("[New session]");
                    } else {
                        println!("[Resuming — {} messages]", message_count);
                    }
                }
            }
            Response::Token(t) => {
                use std::io::Write;
                if show_thinking && first_token {
                    // Erase the "Thinking..." indicator on the first token.
                    print!("\r             \r");
                    first_token = false;
                }
                print!("{}", t);
                std::io::stdout().flush()?;
            }
            Response::ToolCallPrompt { id, command, background } => {
                println!("\n\n[Tool Call] AI wants to run:");
                if background {
                    println!("  $ {} &  (background)", command);
                } else {
                    println!("  $ {}", command);
                }
                print!("Approve? (y/N) ");
                use std::io::Write;
                std::io::stdout().flush()?;
                let mut input = String::new();
                std::io::stdin().read_line(&mut input)?;
                let approved = input.trim().eq_ignore_ascii_case("y");
                send_request(&mut tx, Request::ToolCallResponse { id, approved }).await?;
            }
        }
    }

    Ok(())
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
