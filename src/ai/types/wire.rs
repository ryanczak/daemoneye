use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
    #[serde(skip)]
    pub thought_signature: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub tool_call_id: String,
    pub tool_name: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_results: Option<Vec<ToolResult>>,
    /// Turn number this message belongs to.  Set when the message is pushed
    /// onto the session history so compaction and `/pin` can identify the
    /// turn boundary even after the message vec has been reindexed.
    /// Legacy messages loaded from pre-upgrade session files have `None` and
    /// cannot be pinned, but remain valid for playback.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub turn: Option<usize>,
}

/// Four-bucket token breakdown for a single AI call.
///
/// Providers report `prompt_tokens` as the *total* tokens sent (including cache
/// reads and writes); the `input_tokens` field here is the **uncached** remainder.
/// Backends must subtract cache totals before populating.
#[derive(Debug, Clone, Default, Serialize)]
pub struct TokenBreakdown {
    /// Billed at input rate (prompt_tokens − cache_read − cache_write).
    #[serde(default)]
    pub input_tokens: u32,
    /// Billed at output rate.
    #[serde(default)]
    pub output_tokens: u32,
    /// Billed at cache-read rate (Anthropic ephemeral cache; Gemini implicit cache).
    #[serde(default)]
    pub cache_read_tokens: u32,
    /// Billed at cache-write rate (Anthropic cache creation).
    #[serde(default)]
    pub cache_write_tokens: u32,
}

impl TokenBreakdown {
    /// Sum of all four buckets.
    pub fn total(&self) -> u32 {
        self.input_tokens
            .saturating_add(self.output_tokens)
            .saturating_add(self.cache_read_tokens)
            .saturating_add(self.cache_write_tokens)
    }

    /// Uncached input tokens that are billed at the full input rate.
    /// `input_tokens` already excludes cache_read and cache_write after backend
    /// subtraction, so this is simply the field value.
    ///
    /// Note: Phase 3 cost computation must bill all four fields independently
    /// (`input_tokens`, `output_tokens`, `cache_read_tokens`, `cache_write_tokens`)
    /// at their respective rates. This method returns only the non-cached portion
    /// of the input — not the total billing basis.
    pub fn uncached_input_tokens(&self) -> u32 {
        self.input_tokens
    }
}

// Custom deserializer: accepts both `TokenBreakdown` (new) and `AiUsage` (legacy) shapes.
impl<'de> Deserialize<'de> for TokenBreakdown {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::{MapAccess, Visitor};

        struct TokenBreakdownVisitor;

        impl<'de> Visitor<'de> for TokenBreakdownVisitor {
            type Value = TokenBreakdown;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("a TokenBreakdown or legacy AiUsage object")
            }

            fn visit_map<M>(self, mut map: M) -> Result<TokenBreakdown, M::Error>
            where
                M: MapAccess<'de>,
            {
                let mut input_tokens: Option<u32> = None;
                let mut output_tokens: Option<u32> = None;
                let mut cache_read_tokens: Option<u32> = None;
                let mut cache_write_tokens: Option<u32> = None;
                let mut prompt_tokens: Option<u32> = None;
                let mut completion_tokens: Option<u32> = None;

                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "input_tokens" => input_tokens = Some(map.next_value()?),
                        "output_tokens" => output_tokens = Some(map.next_value()?),
                        "cache_read_tokens" => cache_read_tokens = Some(map.next_value()?),
                        "cache_write_tokens" => cache_write_tokens = Some(map.next_value()?),
                        "prompt_tokens" => prompt_tokens = Some(map.next_value()?),
                        "completion_tokens" => completion_tokens = Some(map.next_value()?),
                        _ => {
                            let _: serde::de::IgnoredAny = map.next_value()?;
                        }
                    }
                }

                if input_tokens.is_some()
                    || output_tokens.is_some()
                    || cache_read_tokens.is_some()
                    || cache_write_tokens.is_some()
                {
                    Ok(TokenBreakdown {
                        input_tokens: input_tokens.unwrap_or(0),
                        output_tokens: output_tokens.unwrap_or(0),
                        cache_read_tokens: cache_read_tokens.unwrap_or(0),
                        cache_write_tokens: cache_write_tokens.unwrap_or(0),
                    })
                } else if let (Some(pt), Some(ct)) = (prompt_tokens, completion_tokens) {
                    Ok(TokenBreakdown {
                        input_tokens: pt,
                        output_tokens: ct,
                        cache_read_tokens: 0,
                        cache_write_tokens: 0,
                    })
                } else {
                    Ok(TokenBreakdown::default())
                }
            }
        }

        deserializer.deserialize_map(TokenBreakdownVisitor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user_msg(content: &str) -> Message {
        Message {
            role: "user".to_string(),
            content: content.to_string(),
            tool_calls: None,
            tool_results: None,
            turn: None,
        }
    }

    #[test]
    fn message_roundtrip_plain() {
        let msg = user_msg("test content");
        let json = serde_json::to_string(&msg).unwrap();
        let back: Message = serde_json::from_str(&json).unwrap();
        assert_eq!(back.role, "user");
        assert_eq!(back.content, "test content");
        assert!(back.tool_calls.is_none());
        assert!(back.tool_results.is_none());
    }

    #[test]
    fn message_tool_calls_skipped_when_none() {
        let msg = user_msg("hi");
        let json = serde_json::to_string(&msg).unwrap();
        assert!(!json.contains("tool_calls"));
        assert!(!json.contains("tool_results"));
    }

    #[test]
    fn tool_call_roundtrip() {
        let tc = ToolCall {
            id: "tc_99".to_string(),
            name: "run_terminal_command".to_string(),
            arguments: r#"{"command":"echo hi","background":true}"#.to_string(),
            thought_signature: None,
        };
        let json = serde_json::to_string(&tc).unwrap();
        let back: ToolCall = serde_json::from_str(&json).unwrap();
        assert_eq!(back.id, "tc_99");
        assert_eq!(back.name, "run_terminal_command");
    }

    // ── TokenBreakdown ────────────────────────────────────────────────────

    #[test]
    fn token_breakdown_total_sums_all_buckets() {
        let tb = TokenBreakdown {
            input_tokens: 1000,
            output_tokens: 500,
            cache_read_tokens: 800,
            cache_write_tokens: 200,
        };
        assert_eq!(tb.total(), 2500);
    }

    #[test]
    fn token_breakdown_zero_tokens_is_zero() {
        let tb = TokenBreakdown::default();
        assert_eq!(tb.total(), 0);
        assert_eq!(tb.uncached_input_tokens(), 0);
    }

    #[test]
    fn token_breakdown_uncached_input_tokens_returns_input_field() {
        let tb = TokenBreakdown {
            input_tokens: 300,
            output_tokens: 100,
            cache_read_tokens: 500,
            cache_write_tokens: 100,
        };
        assert_eq!(tb.uncached_input_tokens(), 300);
    }

    #[test]
    fn token_breakdown_serializes_all_fields() {
        let tb = TokenBreakdown {
            input_tokens: 10,
            output_tokens: 20,
            cache_read_tokens: 30,
            cache_write_tokens: 40,
        };
        let json = serde_json::to_string(&tb).unwrap();
        assert!(json.contains("\"input_tokens\":10"));
        assert!(json.contains("\"output_tokens\":20"));
        assert!(json.contains("\"cache_read_tokens\":30"));
        assert!(json.contains("\"cache_write_tokens\":40"));
    }

    #[test]
    fn legacy_ai_usage_jsonl_deserializes_into_token_breakdown() {
        let legacy_json = r#"{"prompt_tokens":1500,"completion_tokens":800}"#;
        let tb: TokenBreakdown = serde_json::from_str(legacy_json).unwrap();
        assert_eq!(tb.input_tokens, 1500);
        assert_eq!(tb.output_tokens, 800);
        assert_eq!(tb.cache_read_tokens, 0);
        assert_eq!(tb.cache_write_tokens, 0);
        assert_eq!(tb.total(), 2300);
    }

    #[test]
    fn token_breakdown_new_format_deserializes_directly() {
        let new_json = r#"{
            "input_tokens": 200,
            "output_tokens": 100,
            "cache_read_tokens": 500,
            "cache_write_tokens": 50
        }"#;
        let tb: TokenBreakdown = serde_json::from_str(new_json).unwrap();
        assert_eq!(tb.input_tokens, 200);
        assert_eq!(tb.output_tokens, 100);
        assert_eq!(tb.cache_read_tokens, 500);
        assert_eq!(tb.cache_write_tokens, 50);
    }

    #[test]
    fn token_breakdown_zero_cache_when_provider_omits_field() {
        let partial_json = r#"{"input_tokens":100,"output_tokens":50}"#;
        let tb: TokenBreakdown = serde_json::from_str(partial_json).unwrap();
        assert_eq!(tb.cache_read_tokens, 0);
        assert_eq!(tb.cache_write_tokens, 0);
        assert_eq!(tb.input_tokens, 100);
        assert_eq!(tb.output_tokens, 50);
    }
}
