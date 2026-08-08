use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

/// Agent-level tool access policy.
///
/// `allow` and `deny` are mutually exclusive. When `allow` is `Some`, only the
/// listed tool names are permitted. When `deny` is `Some`, all tools except the
/// listed names are permitted. When both are `None`, all tools are allowed
/// (default / unrestricted).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ToolPolicy {
    /// If `Some`, only the listed tool names are permitted.
    #[serde(default)]
    pub allow: Option<Vec<String>>,
    /// If `Some`, all tools except the listed names are permitted.
    #[serde(default)]
    pub deny: Option<Vec<String>>,
}

impl ToolPolicy {
    /// Check whether a tool name is permitted under this policy.
    ///
    /// Returns `true` when the policy is `None` (unrestricted), when the tool
    /// is in the `allow` list, or when the tool is NOT in the `deny` list.
    pub fn permits(&self, tool_name: &str) -> bool {
        match (&self.allow, &self.deny) {
            (Some(allow), _) => allow.iter().any(|t| t == tool_name),
            (None, Some(deny)) => !deny.iter().any(|t| t == tool_name),
            (None, None) => true,
        }
    }

    /// True only when this policy names `tool_name` in an explicit `allow`
    /// list. A `deny` policy that merely fails to deny the tool, and an
    /// unrestricted policy, both return `false` — "not forbidden" is not
    /// "explicitly allowed" (M12 D5, the ghost `tmux_control` gate).
    pub fn explicitly_allows(&self, tool_name: &str) -> bool {
        self.allow
            .as_ref()
            .is_some_and(|allow| allow.iter().any(|t| t == tool_name)) // explicit allow only
    }

    /// Validate that `allow` and `deny` are not both set.
    pub fn validate(&self) -> Result<()> {
        if self.allow.is_some() && self.deny.is_some() {
            bail!("tool policy cannot have both allow and deny lists set");
        }
        Ok(())
    }

    /// Return a human-readable list of permitted tool names, or `None` when
    /// unrestricted. Used for system prompt annotation.
    pub fn permitted_tools(&self) -> Option<Vec<String>> {
        match &self.allow {
            Some(allow) => Some(allow.clone()),
            None => {
                if self.deny.is_some() {
                    // In deny mode everything except the deny list is permitted;
                    // we cannot enumerate "all tools" here, so return None.
                    None
                } else {
                    // Unrestricted
                    None
                }
            }
        }
    }

    /// Returns `true` when this policy is not the default (unrestricted).
    pub fn is_restricted(&self) -> bool {
        self.allow.is_some() || self.deny.is_some()
    }
}

/// Format a tool restriction block for the system prompt.
///
/// Returns `None` when the policy is unrestricted.
pub fn format_tool_restriction_block(policy: &ToolPolicy) -> Option<String> {
    if !policy.is_restricted() {
        return None;
    }
    match &policy.allow {
        Some(allow) => {
            let list = allow.join(", ");
            Some(format!(
                "## Tool Restrictions\n\
                 This session has a restricted tool policy. The following tools are available: {}.\n\
                 Attempting to call any other tool will result in a denial.",
                list
            ))
        }
        None => {
            if let Some(deny) = &policy.deny {
                let list = deny.join(", ");
                Some(format!(
                    "## Tool Restrictions\n\
                     This session has a restricted tool policy. The following tools are NOT available: {}.\n\
                     Attempting to call any of these tools will result in a denial.",
                    list
                ))
            } else {
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicitly_allows_matches_only_the_allow_list() {
        let allowed = ToolPolicy {
            allow: Some(vec!["tmux_control".to_string()]),
            deny: None,
        };
        assert!(allowed.explicitly_allows("tmux_control"));

        let other_allow = ToolPolicy {
            allow: Some(vec!["read_pane".to_string()]),
            deny: None,
        };
        assert!(!other_allow.explicitly_allows("tmux_control"));

        // The negative case that matters: a deny list that merely fails to
        // deny the tool is NOT an explicit allow.
        let deny_other = ToolPolicy {
            allow: None,
            deny: Some(vec!["read_pane".to_string()]),
        };
        assert!(deny_other.permits("tmux_control"));
        assert!(!deny_other.explicitly_allows("tmux_control"));

        let unrestricted = ToolPolicy::default();
        assert!(unrestricted.permits("tmux_control"));
        assert!(!unrestricted.explicitly_allows("tmux_control"));
    }

    #[test]
    fn permits_still_allows_unrestricted() {
        assert!(ToolPolicy::default().permits("any_tool_at_all"));
    }

    #[test]
    fn allows_unlisted_when_deny_mode() {
        let policy = ToolPolicy {
            allow: None,
            deny: Some(vec![
                "dangerous_tool".to_string(),
                "another_bad".to_string(),
            ]),
        };
        assert!(policy.permits("read_file"));
        assert!(policy.permits("search_repository"));
        assert!(!policy.permits("dangerous_tool"));
        assert!(!policy.permits("another_bad"));
    }

    #[test]
    fn denies_listed_when_allow_mode() {
        let policy = ToolPolicy {
            allow: Some(vec![
                "read_file".to_string(),
                "search_repository".to_string(),
            ]),
            deny: None,
        };
        assert!(policy.permits("read_file"));
        assert!(policy.permits("search_repository"));
        assert!(!policy.permits("edit_file"));
        assert!(!policy.permits("run_terminal_command"));
    }

    #[test]
    fn unrestricted_permits_everything() {
        let policy = ToolPolicy::default();
        assert!(policy.permits("anything"));
    }

    #[test]
    fn validation_rejects_both_set() {
        let policy = ToolPolicy {
            allow: Some(vec!["a".to_string()]),
            deny: Some(vec!["b".to_string()]),
        };
        assert!(policy.validate().is_err());
    }

    #[test]
    fn validation_accepts_single_mode() {
        let allow_only = ToolPolicy {
            allow: Some(vec!["a".to_string()]),
            deny: None,
        };
        assert!(allow_only.validate().is_ok());

        let deny_only = ToolPolicy {
            allow: None,
            deny: Some(vec!["b".to_string()]),
        };
        assert!(deny_only.validate().is_ok());
    }

    #[test]
    fn is_restricted_flag() {
        assert!(!ToolPolicy::default().is_restricted());
        assert!(
            ToolPolicy {
                allow: Some(vec!["a".to_string()]),
                deny: None,
            }
            .is_restricted()
        );
        assert!(
            ToolPolicy {
                allow: None,
                deny: Some(vec!["b".to_string()]),
            }
            .is_restricted()
        );
    }

    #[test]
    fn permitted_tools_returns_allow_list() {
        let policy = ToolPolicy {
            allow: Some(vec![
                "read_file".to_string(),
                "search_repository".to_string(),
            ]),
            deny: None,
        };
        assert_eq!(
            policy.permitted_tools(),
            Some(vec![
                "read_file".to_string(),
                "search_repository".to_string()
            ])
        );
    }

    #[test]
    fn permitted_tools_none_for_deny_and_unrestricted() {
        let deny = ToolPolicy {
            allow: None,
            deny: Some(vec!["bad".to_string()]),
        };
        assert!(deny.permitted_tools().is_none());
        assert!(ToolPolicy::default().permitted_tools().is_none());
    }

    #[test]
    fn format_block_allow_mode() {
        let policy = ToolPolicy {
            allow: Some(vec!["read_file".to_string()]),
            deny: None,
        };
        let block = format_tool_restriction_block(&policy).unwrap();
        assert!(block.contains("## Tool Restrictions"));
        assert!(block.contains("read_file"));
    }

    #[test]
    fn format_block_deny_mode() {
        let policy = ToolPolicy {
            allow: None,
            deny: Some(vec!["dangerous".to_string()]),
        };
        let block = format_tool_restriction_block(&policy).unwrap();
        assert!(block.contains("## Tool Restrictions"));
        assert!(block.contains("NOT available"));
        assert!(block.contains("dangerous"));
    }

    #[test]
    fn format_block_none_when_unrestricted() {
        assert!(format_tool_restriction_block(&ToolPolicy::default()).is_none());
    }

    #[test]
    fn toml_roundtrip_allow() {
        let policy = ToolPolicy {
            allow: Some(vec![
                "read_file".to_string(),
                "search_repository".to_string(),
            ]),
            deny: None,
        };
        let toml = toml::to_string(&policy).unwrap();
        let parsed: ToolPolicy = toml::from_str(&toml).unwrap();
        assert_eq!(parsed.allow, policy.allow);
        assert!(parsed.deny.is_none());
    }

    #[test]
    fn toml_roundtrip_deny() {
        let policy = ToolPolicy {
            allow: None,
            deny: Some(vec!["dangerous".to_string()]),
        };
        let toml = toml::to_string(&policy).unwrap();
        let parsed: ToolPolicy = toml::from_str(&toml).unwrap();
        assert!(parsed.allow.is_none());
        assert_eq!(parsed.deny, policy.deny);
    }
}
