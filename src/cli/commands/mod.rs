use anyhow::Result;

mod approval;
mod ask;
mod chat;
mod costs;
mod ipc_client;
mod lifecycle;
mod pane;
mod setup;
mod stream;

pub use ask::run_ask;
pub use costs::{GroupBy, run_costs};
pub use ipc_client::{connect, recv, send_request};
pub use lifecycle::{run_logs, run_ping, run_stop};
pub use setup::run_setup;

use chat::run_chat_inner;

pub async fn run_chat(session_override: Option<String>) -> Result<()> {
    let result = run_chat_inner(session_override).await;
    if let Err(ref e) = result {
        // AsyncStdin has been dropped by now; synchronous stdin is safe.
        use std::io::Write;
        eprintln!("\n\x1b[31m✗\x1b[0m daemoneye error: {}", e);
        eprint!("\x1b[2mPress Enter to close this pane…\x1b[0m");
        std::io::stderr().flush().ok();
        let _ = std::io::stdin().read_line(&mut String::new());
    }
    result
}
