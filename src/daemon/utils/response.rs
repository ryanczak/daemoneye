use crate::ipc::Response;
use tokio::io::AsyncWriteExt;
use tokio::net::UnixStream;

pub async fn send_response(stream: &mut UnixStream, response: Response) -> anyhow::Result<()> {
    let mut data = serde_json::to_vec(&response)?;
    data.push(b'\n');
    stream.write_all(&data).await?;
    Ok(())
}

pub async fn send_response_split<W>(tx: &mut W, response: Response) -> anyhow::Result<()>
where
    W: tokio::io::AsyncWriteExt + Unpin + ?Sized,
{
    let mut data = serde_json::to_vec(&response)?;
    data.push(b'\n');
    tx.write_all(&data).await?;
    Ok(())
}

pub fn fire_notification(job_name: &str, msg: &str, config: &crate::config::Config) {
    let cmd = &config.notifications.on_alert;
    if cmd.is_empty() {
        return;
    }
    let _ = std::process::Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .env("DAEMONEYE_JOB", job_name)
        .env("DAEMONEYE_MSG", msg)
        .spawn();
}
