use anyhow::Result;

mod agents;
mod approval;
mod ask;
mod audit_prompts;
mod chat;
mod chat_presence;
mod costs;
mod interrupt;
pub(crate) mod ipc_client;
mod lifecycle;
mod pane;
mod profile;
mod reindex;
mod run_resume;
mod scorecard;
mod setup;
mod slash;
mod stream;
mod top;

pub use ask::run_ask;
pub use audit_prompts::run_audit_prompts;
pub use costs::{GroupBy, run_costs};
pub use interrupt::InterruptState;
pub use ipc_client::{connect, recv, send_request};
pub use lifecycle::{liveness_line, run_logs, run_ping, run_stop, run_stop_run};
pub use profile::run_profile;
pub use reindex::run_reindex;
pub use run_resume::run_resume;
pub use scorecard::{run_records_between, run_scorecard};
pub use setup::run_setup;
pub use top::{TopFingerprint, fingerprint, run_top};

use chat::run_chat_inner;

/// Validate a `--resume <id>` argument before any connection is made.
///
/// A resumed id must have the shape `new_session_id` produces: 16 lowercase
/// hex characters. Anything else is refused naming the id.
pub fn validate_resume_id(id: &str) -> Result<()> {
    let valid = id.len() == 16
        && id
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase());
    if valid {
        Ok(())
    } else {
        Err(anyhow::anyhow!(
            "Invalid --resume id '{id}': a session id is 16 lowercase hex characters (see the id in the status bar of the session you want to resume)"
        ))
    }
}

/// The one-line hint chat prints on exit, naming the command that resumes
/// this session.
pub fn resume_hint(id: &str) -> String {
    format!("To resume this session later:  daemoneye chat --resume {id}")
}

pub async fn run_chat(resume_id: Option<String>) -> Result<()> {
    if let Some(ref id) = resume_id {
        validate_resume_id(id)?;
    }
    let result = run_chat_inner(resume_id).await;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chat_resume_rejects_a_malformed_id() {
        for bad in ["ABC", "0a1b2c3d4e5f607", "../x", "", "0A1B2C3D4E5F6071"] {
            assert!(validate_resume_id(bad).is_err(), "'{bad}' must be refused");
            let err = validate_resume_id(bad).expect_err("refused with a message");
            assert!(
                err.to_string().contains(bad),
                "the error must name the id: {err}"
            );
        }
        assert!(
            validate_resume_id("0a1b2c3d4e5f6071").is_ok(),
            "a valid 16-lowercase-hex id is accepted"
        );
    }

    #[test]
    fn resume_hint_names_the_resume_command() {
        let hint = resume_hint("0a1b2c3d4e5f6071");
        assert!(
            hint.contains("daemoneye chat --resume 0a1b2c3d4e5f6071"),
            "the hint must name the resume command: {hint}"
        );
    }
}
