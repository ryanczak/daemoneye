/// True when `window_name` belongs to a daemon-managed window.
pub(crate) fn is_daemon_window(window_name: &str) -> bool {
    crate::daemon::is_daemon_window(window_name)
}

/// Why `tmux_control(action="kill_window")` must refuse this window, or `None`
/// when it may proceed (M12 D5).
///
/// Pure so the two refusals can be tested without tmux or a cache.
pub(crate) fn kill_window_refusal(window_name: &str, chat_window: Option<&str>) -> Option<String> {
    if is_daemon_window(window_name) {
        return Some(format!(
            "Refusing to kill window '{}': it is daemon-managed. It is reclaimed \
             automatically once its job is done.",
            window_name
        )); // daemon windows belong to the background-window substrate
    }
    if chat_window == Some(window_name) {
        return Some(format!(
            "Refusing to kill window '{}': it contains the chat pane, which would end this \
             conversation.",
            window_name
        )); // never kill the window we are talking through
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kill_window_refusal_refuses_daemon_windows() {
        // A background-window name returns Some.
        let refusal = kill_window_refusal("de-bg-42-1712937600-cargo", None);
        assert!(refusal.is_some(), "daemon bg window should be refused");

        // A ghost-sched prefix also returns Some.
        let refusal = kill_window_refusal("de-gs-ir-42-1712937600-ir", None);
        assert!(refusal.is_some(), "ghost-sched window should be refused");
    }

    #[test]
    fn kill_window_refusal_refuses_the_chat_window() {
        let refusal = kill_window_refusal("my-window", Some("my-window"));
        assert!(refusal.is_some(), "chat window should be refused");
        let msg = refusal.unwrap();
        assert!(
            msg.contains("chat pane"),
            "message should mention chat pane, got: {msg}"
        );
    }

    #[test]
    fn kill_window_refusal_allows_a_plain_user_window() {
        // Neither daemon-owned nor the chat window → None.
        assert_eq!(
            kill_window_refusal("my-editor", None),
            None,
            "plain window with no chat_window should be allowed"
        );

        // chat_window names a different window → allowed.
        assert_eq!(
            kill_window_refusal("my-editor", Some("my-terminal")),
            None,
            "window different from chat_window should be allowed"
        );
    }
}
