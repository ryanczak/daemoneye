use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::PathBuf;

/// A named runbook loaded from `~/.daemoneye/runbooks/<name>.toml`.
///
/// Runbooks provide context for watchdog AI analysis: the `context` string is
/// injected into the AI system prompt so it knows what "normal" looks like,
/// and `alert_on` contains keyword patterns that signal an alert condition.
#[derive(Debug, Deserialize, Clone)]
pub struct Runbook {
    pub name: String,
    /// Injected into the watchdog AI system prompt as context.
    pub context: String,
    /// Keywords or patterns in command output that trigger an alert.
    #[serde(default)]
    pub alert_on: Vec<String>,
}

/// Return the path to the runbooks directory: `~/.daemoneye/runbooks/`.
pub fn runbooks_dir() -> PathBuf {
    crate::config::config_dir().join("runbooks")
}

/// Load a named runbook from `~/.daemoneye/runbooks/<name>.toml`.
pub fn load_runbook(name: &str) -> Result<Runbook> {
    let path = runbooks_dir().join(format!("{}.toml", name));
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("reading runbook at {}", path.display()))?;
    toml::from_str::<Runbook>(&text)
        .with_context(|| format!("parsing runbook at {}", path.display()))
}

/// Build the watchdog system prompt by injecting runbook context into a base prompt.
pub fn watchdog_system_prompt(runbook: &Runbook) -> String {
    format!(
        "You are an automated watchdog monitor. Analyze the command output below and \
         determine if it indicates an alert condition.\n\n\
         ## Runbook: {}\n{}\n\n\
         ## Alert Conditions\n{}\n\n\
         Respond with ALERT if any alert condition is met, followed by a brief explanation. \
         Respond with OK if everything looks normal.",
        runbook.name,
        runbook.context,
        runbook
            .alert_on
            .iter()
            .map(|p| format!("- {}", p))
            .collect::<Vec<_>>()
            .join("\n")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn watchdog_prompt_contains_runbook_name() {
        let rb = Runbook {
            name: "disk-check".to_string(),
            context: "Normal disk usage is below 80%".to_string(),
            alert_on: vec!["9[0-9]%".to_string(), "100%".to_string()],
        };
        let prompt = watchdog_system_prompt(&rb);
        assert!(prompt.contains("disk-check"));
        assert!(prompt.contains("80%"));
        assert!(prompt.contains("ALERT"));
    }
}
