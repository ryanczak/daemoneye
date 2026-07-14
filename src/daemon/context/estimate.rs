use crate::ai::Message;

/// Fixed per-message overhead (role tags, framing) in estimated tokens.
const PER_MESSAGE_OVERHEAD: u64 = 8;
/// Fixed per-tool-call / per-tool-result framing overhead.
const PER_TOOL_ITEM_OVERHEAD: u64 = 12;

/// Estimate the prompt-token footprint of one message: ~4 chars per token
/// over all textual payloads, plus fixed framing overheads.
pub fn estimate_message_tokens(msg: &Message) -> u64 {
    let mut chars = msg.content.len() as u64;
    let mut items = 0u64;
    if let Some(calls) = &msg.tool_calls {
        for c in calls {
            chars += (c.name.len() + c.arguments.len()) as u64;
            items += 1;
        }
    }
    if let Some(results) = &msg.tool_results {
        for r in results {
            chars += (r.tool_name.len() + r.content.len()) as u64;
            items += 1;
        }
    }
    chars.div_ceil(4) + PER_MESSAGE_OVERHEAD + items * PER_TOOL_ITEM_OVERHEAD
}

/// Sum of `estimate_message_tokens` over a history slice.
pub fn estimate_history_tokens(messages: &[Message]) -> u64 {
    messages.iter().map(estimate_message_tokens).sum()
}

/// Update the calibration scale on a session entry using the latest observation.
///
/// When `last_prompt_tokens > 0` and the estimated history tokens are also
/// positive, blends the new observed ratio into the EMA.  When either is 0,
/// leaves the scale untouched (no observation to calibrate against).
pub fn update_token_scale(entry: &mut crate::daemon::session::SessionEntry, messages: &[Message]) {
    let est = estimate_history_tokens(messages);
    if est > 0 && entry.last_prompt_tokens > 0 {
        let observed = entry.last_prompt_tokens as f64 / est as f64;
        entry.token_scale = (0.7 * entry.token_scale + 0.3 * observed).clamp(0.5, 4.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::types::ToolCall;

    fn make_plain(content: &str) -> Message {
        Message {
            role: "user".to_string(),
            content: content.to_string(),
            tool_calls: None,
            tool_results: None,
            turn: None,
        }
    }

    #[test]
    fn estimate_plain_message_pins_formula() {
        // 100-char content → 100 / 4 = 25 + 8 overhead = 33
        let msg = make_plain(&"x".repeat(100));
        let tokens = estimate_message_tokens(&msg);
        assert_eq!(tokens, 33);
    }

    #[test]
    fn estimate_counts_tool_calls_and_results() {
        let content = "a".repeat(40);
        let msg = Message {
            role: "assistant".to_string(),
            content: content.clone(),
            tool_calls: Some(vec![ToolCall {
                id: "tc1".to_string(),
                name: "run_terminal_command".to_string(),
                arguments: r#"{"command":"ls"}"#.to_string(),
                thought_signature: None,
            }]),
            tool_results: None,
            turn: None,
        };
        // content=40, tool_call name=20 + args=15 = 35
        // total chars = 40 + 35 = 75; items = 1
        // tokens = 75 / 4 (ceil) + 8 + 1*12 = 19 + 8 + 12 = 39
        let tokens = estimate_message_tokens(&msg);
        assert_eq!(tokens, 39);
    }

    #[test]
    fn estimate_history_sums_messages() {
        let msgs = vec![
            make_plain(&"x".repeat(100)), // 33
            make_plain(&"y".repeat(200)), // 58
        ];
        assert_eq!(estimate_history_tokens(&msgs), 33 + 58);
    }

    #[test]
    fn update_token_scale_converges_and_clamps() {
        let mut entry = crate::daemon::session::SessionEntry {
            messages: vec![],
            last_accessed: std::time::Instant::now(),
            chat_pane: None,
            default_target_pane: None,
            bg_windows: vec![],
            last_prompt_tokens: 0,
            token_scale: 1.5,
            tmux_session: "test".to_string(),
            last_detach: None,
            detach_time_utc: None,
            messages_at_detach: 0,
            pipe_source_pane: None,
            is_ghost: false,
            ghost_config: None,
            ghost_bg_prefix: "",
            started_at: chrono::Utc::now(),
            turn_count: 0,
            tool_calls_this_session: 0,
            active_model: None,
            last_snapshot_activity: 0,
            saved_name: None,
            dirty: false,
            artifacts_created: vec![],
            auto_name_suggested: false,
            ghost_task_message: None,
            loaded_tools: std::collections::HashSet::new(),
            cost_usd: 0.0,
            cost_by_agent: std::collections::HashMap::new(),
            has_untracked_cost: false,
        };

        // Feed a constant observed/estimated ratio of 2.0 repeatedly.
        // We simulate by setting last_prompt_tokens to 2 * estimated.
        let msgs = vec![make_plain(&"x".repeat(100))];
        let est = estimate_history_tokens(&msgs);

        // 10 iterations: scale should monotonically approach 2.0
        let mut prev = 1.5;
        for _ in 0..10 {
            entry.last_prompt_tokens = (est * 2) as u32;
            update_token_scale(&mut entry, &msgs);
            assert!(
                entry.token_scale >= prev,
                "Scale should increase monotonically toward 2.0"
            );
            assert!(
                entry.token_scale <= 2.0,
                "Scale should not overshoot 2.0 with constant observed=2.0"
            );
            prev = entry.token_scale;
        }

        // Test clamping with adversarial ratios
        let msgs = vec![make_plain(&"x".repeat(100))];
        let est = estimate_history_tokens(&msgs);

        // Extremely low ratio: observed = 0.01 * est
        entry.token_scale = 1.5;
        entry.last_prompt_tokens = (est as f64 * 0.01) as u32;
        update_token_scale(&mut entry, &msgs);
        assert!(
            entry.token_scale >= 0.5,
            "Scale should clamp to >= 0.5 with very low observed ratio"
        );

        // Extremely high ratio: observed = 1000.0 * est
        entry.token_scale = 1.5;
        entry.last_prompt_tokens = (est as f64 * 1000.0) as u32;
        update_token_scale(&mut entry, &msgs);
        assert!(
            entry.token_scale <= 4.0,
            "Scale should clamp to <= 4.0 with very high observed ratio"
        );
    }

    #[test]
    fn update_token_scale_noop_when_no_observation() {
        let mut entry = crate::daemon::session::SessionEntry {
            messages: vec![],
            last_accessed: std::time::Instant::now(),
            chat_pane: None,
            default_target_pane: None,
            bg_windows: vec![],
            last_prompt_tokens: 0,
            token_scale: 1.5,
            tmux_session: "test".to_string(),
            last_detach: None,
            detach_time_utc: None,
            messages_at_detach: 0,
            pipe_source_pane: None,
            is_ghost: false,
            ghost_config: None,
            ghost_bg_prefix: "",
            started_at: chrono::Utc::now(),
            turn_count: 0,
            tool_calls_this_session: 0,
            active_model: None,
            last_snapshot_activity: 0,
            saved_name: None,
            dirty: false,
            artifacts_created: vec![],
            auto_name_suggested: false,
            ghost_task_message: None,
            loaded_tools: std::collections::HashSet::new(),
            cost_usd: 0.0,
            cost_by_agent: std::collections::HashMap::new(),
            has_untracked_cost: false,
        };

        let original_scale = entry.token_scale;

        // Empty history → no update
        let empty: &[Message] = &[];
        entry.last_prompt_tokens = 100;
        update_token_scale(&mut entry, empty);
        assert_eq!(
            entry.token_scale, original_scale,
            "Scale should be unchanged when history is empty"
        );

        // last_prompt_tokens == 0 → no update
        let msgs = vec![make_plain("hello")];
        entry.last_prompt_tokens = 0;
        update_token_scale(&mut entry, &msgs);
        assert_eq!(
            entry.token_scale, original_scale,
            "Scale should be unchanged when last_prompt_tokens is 0"
        );
    }
}
