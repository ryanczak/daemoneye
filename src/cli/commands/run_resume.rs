//! `daemoneye run resume <id> "<directive>"` — send the resume request to the
//! daemon and print the replies the user reads.

use anyhow::Result;
use tokio::io::BufReader;

use crate::ipc::{Request, Response};

use super::ipc_client::{connect, recv, send_request};

/// Resume a finished ghost run: connect, send
/// `Request::ResumeRun { run_id, directive }`, print every `SystemMsg`
/// (the daemon's read reason for each refusal, or the new session id), and
/// stop after `Ok`.
pub async fn run_resume(run: &str, directive: &str) -> Result<()> {
    let stream = connect().await?;
    let (rx, mut tx) = stream.into_split();
    let mut rx = BufReader::new(rx);
    send_request(
        &mut tx,
        Request::ResumeRun {
            run_id: run.to_string(),
            directive: directive.to_string(),
        },
    )
    .await?;
    loop {
        match recv(&mut rx).await? {
            Response::SystemMsg(msg) => println!("{msg}"),
            Response::Ok => return Ok(()),
            other => anyhow::bail!("unexpected reply to a resume request: {other:?}"),
        }
    }
}
