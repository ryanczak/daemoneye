/// System prompt for the auto-naming LLM call.
pub const AUTONAME_SYSTEM_PROMPT: &str = "\
You are a session naming assistant. Given a conversation snippet, output exactly two lines:\n\
Line 1: a short kebab-case slug (1–4 words, lowercase letters and hyphens only, no punctuation).\n\
Line 2: a one-sentence description (under 80 characters).\n\
Output nothing else. No labels, no explanations.";

/// Timeout for the auto-naming LLM call in seconds.
pub const AUTONAME_TIMEOUT_SECS: u64 = 20;

/// Call the digest model to suggest a `(name, description)` pair for an unnamed session.
/// Returns `None` on timeout, API error, or if the response cannot be parsed into a valid name.
pub async fn suggest_session_name(
    messages: &[crate::ai::Message],
    config: &crate::config::Config,
) -> Option<(String, String)> {
    // Build a compact transcript: at most the last 20 user/assistant exchanges.
    let transcript: String = messages
        .iter()
        .rev()
        .take(20)
        .rev()
        .filter(|m| m.role == "user" || m.role == "assistant")
        .map(|m| {
            format!(
                "[{}] {}",
                m.role,
                m.content.chars().take(400).collect::<String>()
            )
        })
        .collect::<Vec<_>>()
        .join("\n");

    let user_msg = crate::ai::Message {
        role: "user".to_string(),
        content: format!("Conversation:\n\n{transcript}"),
        tool_calls: None,
        tool_results: None,
        turn: None,
    };

    let model_entry =
        config.resolve_model(config.models.contains_key("digest").then_some("digest"));
    let client = crate::ai::make_client(
        &model_entry.provider,
        model_entry.resolve_api_key(),
        model_entry.model.clone(),
        model_entry.effective_base_url(),
    );

    let (ev_tx, mut ev_rx) = tokio::sync::mpsc::unbounded_channel::<crate::ai::AiEvent>();
    let chat_fut = client.chat(AUTONAME_SYSTEM_PROMPT, vec![user_msg], ev_tx, false);
    let timeout = std::time::Duration::from_secs(AUTONAME_TIMEOUT_SECS);

    match tokio::time::timeout(timeout, chat_fut).await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => {
            log::debug!("auto-name: LLM error: {}", e);
            return None;
        }
        Err(_) => {
            log::debug!("auto-name: timed out after {}s", AUTONAME_TIMEOUT_SECS);
            return None;
        }
    }

    let mut text = String::new();
    while let Some(ev) = ev_rx.recv().await {
        if let crate::ai::AiEvent::Token(t) = ev {
            text.push_str(&t);
        }
    }

    let mut lines = text.trim().lines();
    let raw_name = lines.next().unwrap_or("").trim().to_string();
    let description = lines.next().unwrap_or("").trim().to_string();

    // Validate: must pass session name rules.
    if crate::session_store::validate_session_name(&raw_name).is_err() {
        log::debug!("auto-name: suggested name '{}' failed validation", raw_name);
        return None;
    }

    Some((raw_name, description))
}
