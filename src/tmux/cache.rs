use std::collections::HashMap;
use std::sync::RwLock;
use anyhow::Result;
use crate::tmux;
use crate::ai::filter::mask_sensitive;

#[derive(Debug, Clone)]
pub struct PaneState {
    pub buffer: String,
    pub summary: String,
    pub last_updated: std::time::Instant,
}

pub struct SessionCache {
    pub session_name: String,
    pub panes: RwLock<HashMap<String, PaneState>>,
    pub active_pane: RwLock<Option<String>>,
}

impl SessionCache {
    pub fn new(session_name: &str) -> Self {
        Self {
            session_name: session_name.to_string(),
            panes: RwLock::new(HashMap::new()),
            active_pane: RwLock::new(None),
        }
    }

    /// Refresh the cache by listing panes and capturing their content.
    pub fn refresh(&self) -> Result<()> {
        let active = tmux::get_active_pane(&self.session_name)?;
        {
            let mut active_lock = self.active_pane.write().unwrap();
            *active_lock = Some(active);
        }

        let pane_ids = tmux::list_panes(&self.session_name)?;
        
        for id in pane_ids {
            // Capture the last 100 lines for now
            if let Ok(content) = tmux::capture_pane(&id, 100) {
                let mut panes = self.panes.write().unwrap();
                let entry = panes.entry(id.clone()).or_insert_with(|| PaneState {
                    buffer: String::new(),
                    summary: String::new(),
                    last_updated: std::time::Instant::now(),
                });
                
                if entry.buffer != content {
                    entry.buffer = content;
                    entry.summary = self.summarize(&entry.buffer);
                    entry.last_updated = std::time::Instant::now();
                }
            }
        }
        
        Ok(())
    }

    /// Simple heuristic-based summarization of pane content.
    fn summarize(&self, buffer: &str) -> String {
        let lines: Vec<&str> = buffer.lines().filter(|l| !l.trim().is_empty()).collect();
        if lines.is_empty() {
            return "Empty pane".to_string();
        }
        
        // Take the last non-empty line as a hint of what's happening
        let last_line = lines.last().unwrap_or(&"").trim();
        
        if last_line.starts_with('$') || last_line.starts_with('#') {
            format!("Idle shell at: {}", last_line)
        } else if last_line.contains("top - ") || last_line.contains("htop") {
            "Running system monitor".to_string()
        } else if last_line.contains("GET /") || last_line.contains("POST /") {
            "Tailing web logs".to_string()
        } else {
            format!("Active: {}", last_line.chars().take(50).collect::<String>())
        }
    }

    /// Get a full context summary for the AI.
    pub fn get_context_summary(&self) -> String {
        let panes = self.panes.read().unwrap();
        let active_id = self.active_pane.read().unwrap();
        
        let mut summary = String::from("Current Tmux Session State:\n");
        for (id, state) in panes.iter() {
            let marker = if Some(id) == active_id.as_ref() { " (ACTIVE)" } else { "" };
            let masked_summary = mask_sensitive(&state.summary);
            summary.push_str(&format!("- Pane {}{}: {}\n", id, marker, masked_summary));
        }
        
        summary
    }

}
