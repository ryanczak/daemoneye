//! `daemoneye sandbox build` — build the agent image and record its id.

use crate::config::Config;
use crate::daemon::executor::container::{
    SandboxLock, is_valid_image_id, lock_path, read_lock, write_lock,
};
use crate::tmux::bounded_output_with;
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Render the operator-facing result. Pure, so the wording is unit-testable.
fn format_build_result(image: &str, image_id: &str, rebuilt: bool) -> String {
    let action = if rebuilt { "Rebuilt" } else { "Built" };
    format!(
        "{action} image '{image}' (id {image_id}).\nRecorded in {}",
        lock_path().display()
    )
}

/// `daemoneye sandbox build` — build the agent image and record its id.
pub fn run_sandbox_build() {
    let cfg = Config::load().unwrap_or_default();
    let image = cfg.sandbox.image.clone();
    let docker_host = cfg.sandbox.docker_host.clone();

    let mut cmd = Command::new("docker");
    cmd.args([
        "build",
        "-q",
        "-t",
        &image,
        "-f",
        "containers/Dockerfile",
        "containers",
    ])
    .env("DOCKER_HOST", &docker_host);
    let output = match bounded_output_with(&mut cmd, Duration::from_secs(600)) {
        Ok(output) => output,
        Err(e) => {
            eprintln!("Failed to spawn docker: {}", e);
            std::process::exit(1);
        }
    };
    if !output.status.success() {
        eprintln!(
            "docker build failed for image '{}' (runtime {}):\n{}",
            image,
            docker_host,
            String::from_utf8_lossy(&output.stderr).trim()
        );
        std::process::exit(1);
    }

    let raw_stdout = String::from_utf8_lossy(&output.stdout);
    let image_id = raw_stdout.trim();
    if !is_valid_image_id(image_id) {
        eprintln!(
            "docker build printed an invalid image id ({:?}); refusing to write a lock.",
            image_id
        );
        std::process::exit(1);
    }

    let previous = read_lock();
    let rebuilt = previous
        .as_ref()
        .map(|lock| lock.image_id != image_id)
        .unwrap_or(false);

    let written = write_lock(&SandboxLock {
        image: image.clone(),
        image_id: image_id.to_string(),
        built_at: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
    });
    if let Err(e) = written {
        eprintln!(
            "Failed to write lock file at {}: {}",
            lock_path().display(),
            e
        );
        std::process::exit(1);
    }

    println!("{}", format_build_result(&image, image_id, rebuilt));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sandbox_lock_build_result_distinguishes_first_build_from_rebuild() {
        let first = format_build_result("daemoneye-agent-base", "sha256:j", false);
        let rebuilt = format_build_result("daemoneye-agent-base", "sha256:j", true);
        assert_ne!(first, rebuilt);
        assert!(first.contains("daemoneye-agent-base"));
        assert!(first.contains("sha256:j"));
        assert!(rebuilt.contains("daemoneye-agent-base"));
        assert!(rebuilt.contains("sha256:j"));
    }
}
