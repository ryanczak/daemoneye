use anyhow::{Result, bail};
use async_trait::async_trait;
use futures_util::StreamExt;
use reqwest::header::{CONTENT_TYPE, HeaderMap, HeaderValue};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::Duration;
use tokio::sync::mpsc::UnboundedSender;

static TOOL_CALL_ID: AtomicU64 = AtomicU64::new(1);

fn next_tool_id() -> String {
    format!("tc_{}", TOOL_CALL_ID.fetch_add(1, Ordering::Relaxed))
}

/// Single shared HTTP client for the lifetime of the process.
/// `reqwest::Client` manages a connection pool internally; creating one per
/// request throws away that pool on every turn.
static HTTP_CLIENT: OnceLock<reqwest::Client> = OnceLock::new();

fn http() -> &'static reqwest::Client {
    HTTP_CLIENT.get_or_init(reqwest::Client::new)
}

// ---------------------------------------------------------------------------
// Message types for conversation history
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub tool_call_id: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    /// All tool results for a single assistant turn, batched together.
    /// Anthropic and Gemini require them in one message; OpenAI expands them
    /// into separate `role: "tool"` messages inside `convert_messages`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_results: Option<Vec<ToolResult>>,
}

// ---------------------------------------------------------------------------
// Events sent back to the GTK main thread
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum AiEvent {
    Token(String),
    ToolCall(String, String, bool), // id, command, background
    Done,
    Error(String),
}

// ---------------------------------------------------------------------------
// Trait
// ---------------------------------------------------------------------------

#[async_trait]
pub trait AiClient: Send + Sync {
    /// Stream response tokens by sending `AiEvent` values to `tx`.
    async fn chat(&self, system: &str, messages: Vec<Message>, tx: UnboundedSender<AiEvent>) -> Result<()>;
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Send an HTTP request, retrying up to 2 times on 429 / 5xx responses
/// with exponential backoff (2 s, then 4 s). Any other non-success status
/// bails immediately without retrying.
async fn send_with_retry(make_req: impl Fn() -> reqwest::RequestBuilder) -> Result<reqwest::Response> {
    let mut delay = Duration::from_secs(2);
    for attempt in 0u32..3 {
        let response = make_req().send().await?;
        let status = response.status();
        if status.is_success() {
            return Ok(response);
        }
        let retryable = status.is_server_error()
            || status == reqwest::StatusCode::TOO_MANY_REQUESTS;
        if !retryable || attempt == 2 {
            let text = response.text().await.unwrap_or_default();
            bail!("API error {}: {}", status, text);
        }
        // Honour Retry-After header on 429 (capped at 30 s); fall back to
        // the exponential schedule when absent or unparseable.
        let sleep_for = if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            response
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.parse::<u64>().ok())
                .map(Duration::from_secs)
                .map(|d| d.min(Duration::from_secs(30)))
                .unwrap_or(delay)
        } else {
            delay
        };
        tokio::time::sleep(sleep_for).await;
        delay *= 2;
    }
    unreachable!()
}

fn get_tool_definition() -> Value {
    json!([
        {
            "name": "run_terminal_command",
            "description": "Execute a bash command in one of two modes:\n\
             - background=true: Runs as a subprocess on the DAEMON HOST. Output is captured silently and returned to you. Use for read-only diagnostics (ls, ps, cat, grep, df, curl, etc.). If the user is SSH'd into a remote host, this still runs locally on the daemon machine. Supports sudo: the user will be prompted for their password in the chat interface.\n\
             - background=false: Injects the command into the USER'S TERMINAL PANE via tmux send-keys. The command is visible and interactive. Use for state-changing commands, service restarts, file edits, or anything that must run on the user's active host. If the user's pane is SSH'd to a remote machine, the command runs there. Supports sudo: the user types their password directly in the terminal pane.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "command": {"type": "string", "description": "The bash command to execute."},
                    "background": {"type": "boolean", "description": "true = daemon host subprocess (invisible); false = user's terminal pane (visible, interactive, possibly remote)."}
                },
                "required": ["command", "background"]
            }
        }
    ])
}

fn get_openai_tool_definition() -> Value {
    json!([
        {
            "type": "function",
            "function": {
                "name": "run_terminal_command",
                "description": "Execute a bash command in one of two modes:\n\
                 - background=true: Runs as a subprocess on the DAEMON HOST. Output is captured silently. Use for read-only diagnostics. If the user is SSH'd to a remote host, this still runs on the daemon machine. Supports sudo via chat interface.\n\
                 - background=false: Injects the command into the USER'S TERMINAL PANE via tmux. Visible and interactive. Use for state-changing commands or anything needing the user's active host. If the pane is SSH'd, the command runs remotely. Sudo requires the user to type their password in the pane.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "command": {"type": "string", "description": "The bash command to execute."},
                        "background": {"type": "boolean", "description": "true = daemon host subprocess (invisible); false = user's terminal pane (visible, interactive, possibly remote)."}
                    },
                    "required": ["command", "background"]
                }
            }
        }
    ])
}

// ---------------------------------------------------------------------------
// Anthropic
// ---------------------------------------------------------------------------

pub struct AnthropicClient {
    api_key: String,
    model: String,
}

impl AnthropicClient {
    pub fn new(api_key: String, model: String) -> Self {
        AnthropicClient { api_key, model }
    }

    fn convert_messages(&self, messages: Vec<Message>) -> Vec<Value> {
        let mut result = Vec::new();
        for m in messages {
            if let Some(trs) = m.tool_results {
                // Anthropic requires all tool results in a single user message.
                let content: Vec<Value> = trs.into_iter().map(|tr| json!({
                    "type": "tool_result",
                    "tool_use_id": tr.tool_call_id,
                    "content": tr.content
                })).collect();
                result.push(json!({"role": "user", "content": content}));
            } else if let Some(tcs) = m.tool_calls {
                let mut content = Vec::new();
                if !m.content.is_empty() {
                    content.push(json!({"type": "text", "text": m.content}));
                }
                for tc in tcs {
                    let args: Value = serde_json::from_str(&tc.arguments).unwrap_or(json!({}));
                    content.push(json!({
                        "type": "tool_use",
                        "id": tc.id,
                        "name": tc.name,
                        "input": args
                    }));
                }
                result.push(json!({
                    "role": "assistant",
                    "content": content
                }));
            } else {
                result.push(json!({
                    "role": m.role,
                    "content": m.content
                }));
            }
        }
        result
    }
}

#[async_trait]
impl AiClient for AnthropicClient {
    async fn chat(&self, system: &str, messages: Vec<Message>, tx: UnboundedSender<AiEvent>) -> Result<()> {
        let mut headers = HeaderMap::new();
        headers.insert("x-api-key", HeaderValue::from_str(&self.api_key)?);
        headers.insert("anthropic-version", HeaderValue::from_static("2023-06-01"));
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

        let converted = self.convert_messages(messages);

        let body = json!({
            "model": self.model,
            "max_tokens": 4096,
            "stream": true,
            "system": system,
            "messages": converted,
            "tools": get_tool_definition()
        });

        let response = send_with_retry(|| {
            http()
                .post("https://api.anthropic.com/v1/messages")
                .headers(headers.clone())
                .json(&body)
        }).await?;

        let mut stream = response.bytes_stream();
        let mut tool_id = String::new();
        let mut tool_args = String::new();
        let mut leftover = String::new();

        'outer: while let Some(chunk) = stream.next().await {
            let bytes = chunk?;
            leftover.push_str(&String::from_utf8_lossy(&bytes));

            while let Some(pos) = leftover.find('\n') {
                let line = leftover[..pos].trim().to_string();
                leftover = leftover[pos + 1..].to_string();

                if let Some(data) = line.strip_prefix("data: ") {
                    if data == "[DONE]" {
                        break 'outer;
                    }
                    if let Ok(v) = serde_json::from_str::<Value>(data) {
                        let msg_type = v["type"].as_str().unwrap_or("");

                        if msg_type == "content_block_start" {
                            if v["content_block"]["type"] == "tool_use" {
                                if let Some(id) = v["content_block"]["id"].as_str() {
                                    tool_id = id.to_string();
                                    tool_args.clear();
                                }
                            }
                        } else if msg_type == "content_block_delta" {
                            if v["delta"]["type"] == "text_delta" {
                                if let Some(t) = v["delta"]["text"].as_str() {
                                    if !t.is_empty() {
                                        let _ = tx.send(AiEvent::Token(t.to_string()));
                                    }
                                }
                            } else if v["delta"]["type"] == "input_json_delta" {
                                if let Some(partial) = v["delta"]["partial_json"].as_str() {
                                    tool_args.push_str(partial);
                                }
                            }
                        } else if msg_type == "content_block_stop" {
                            if !tool_id.is_empty() {
                                if let Ok(args_json) = serde_json::from_str::<Value>(&tool_args) {
                                    if let Some(cmd) = args_json["command"].as_str() {
                                        let bg = args_json["background"].as_bool().unwrap_or(false);
                                        let _ = tx.send(AiEvent::ToolCall(tool_id.clone(), cmd.to_string(), bg));
                                    }
                                }
                                tool_id.clear();
                                tool_args.clear();
                            }
                        }
                    }
                }
            }
        }
        let _ = tx.send(AiEvent::Done);
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// OpenAI-compatible
// ---------------------------------------------------------------------------

pub struct OpenAiClient {
    api_key: String,
    model: String,
    base_url: String,
}

impl OpenAiClient {
    pub fn new(api_key: String, model: String) -> Self {
        OpenAiClient {
            api_key,
            model,
            base_url: std::env::var("OPENAI_API_BASE")
                .unwrap_or_else(|_| "https://api.openai.com/v1".to_string()),
        }
    }

    fn convert_messages(&self, messages: Vec<Message>) -> Vec<Value> {
        let mut result = Vec::new();
        for m in messages {
            if let Some(trs) = m.tool_results {
                // OpenAI expects one role: "tool" message per result.
                for tr in trs {
                    result.push(json!({
                        "role": "tool",
                        "tool_call_id": tr.tool_call_id,
                        "content": tr.content
                    }));
                }
            } else if let Some(tcs) = m.tool_calls {
                let mut tool_calls = Vec::new();
                for tc in tcs {
                    tool_calls.push(json!({
                        "id": tc.id,
                        "type": "function",
                        "function": {
                            "name": tc.name,
                            "arguments": tc.arguments
                        }
                    }));
                }
                result.push(json!({
                    "role": "assistant",
                    "content": m.content,
                    "tool_calls": tool_calls
                }));
            } else {
                result.push(json!({
                    "role": m.role,
                    "content": m.content
                }));
            }
        }
        result
    }
}

#[async_trait]
impl AiClient for OpenAiClient {
    async fn chat(&self, system: &str, messages: Vec<Message>, tx: UnboundedSender<AiEvent>) -> Result<()> {
        let converted = self.convert_messages(messages);
        let mut full_messages = vec![json!({"role": "system", "content": system})];
        full_messages.extend(converted);

        let body = json!({
            "model": self.model,
            "max_tokens": 4096,
            "stream": true,
            "messages": full_messages,
            "tools": get_openai_tool_definition()
        });

        let response = send_with_retry(|| {
            http()
                .post(format!("{}/chat/completions", self.base_url))
                .bearer_auth(&self.api_key)
                .json(&body)
        }).await?;

        let mut stream = response.bytes_stream();
        let mut tool_id = String::new();
        let mut tool_args = String::new();
        let mut leftover = String::new();

        'outer: while let Some(chunk) = stream.next().await {
            let bytes = chunk?;
            leftover.push_str(&String::from_utf8_lossy(&bytes));

            while let Some(pos) = leftover.find('\n') {
                let line = leftover[..pos].trim().to_string();
                leftover = leftover[pos + 1..].to_string();

                if let Some(data) = line.strip_prefix("data: ") {
                    if data == "[DONE]" {
                        break 'outer;
                    }
                    if let Ok(v) = serde_json::from_str::<Value>(data) {
                        if let Some(delta) = v["choices"][0]["delta"].as_object() {
                            if let Some(content) = delta.get("content").and_then(|c| c.as_str()) {
                                if !content.is_empty() {
                                    let _ = tx.send(AiEvent::Token(content.to_string()));
                                }
                            }
                            if let Some(tool_calls) = delta.get("tool_calls").and_then(|t| t.as_array()) {
                                if let Some(tc) = tool_calls.get(0) {
                                    if let Some(id) = tc.get("id").and_then(|i| i.as_str()) {
                                        if !tool_id.is_empty() && tool_id != id {
                                            if let Ok(args_json) = serde_json::from_str::<Value>(&tool_args) {
                                                if let Some(cmd) = args_json["command"].as_str() {
                                                    let bg = args_json["background"].as_bool().unwrap_or(false);
                                                    let _ = tx.send(AiEvent::ToolCall(tool_id.clone(), cmd.to_string(), bg));
                                                }
                                            }
                                        }
                                        tool_id = id.to_string();
                                        tool_args.clear();
                                    }
                                    if let Some(f) = tc.get("function") {
                                        if let Some(args) = f.get("arguments").and_then(|a| a.as_str()) {
                                            tool_args.push_str(args);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        // Final flush of any buffered tool call
        if !tool_id.is_empty() {
            if let Ok(args_json) = serde_json::from_str::<Value>(&tool_args) {
                if let Some(cmd) = args_json["command"].as_str() {
                    let bg = args_json["background"].as_bool().unwrap_or(false);
                    let _ = tx.send(AiEvent::ToolCall(tool_id.clone(), cmd.to_string(), bg));
                }
            }
        }

        let _ = tx.send(AiEvent::Done);
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Gemini
// ---------------------------------------------------------------------------

pub struct GeminiClient {
    api_key: String,
    model: String,
}

impl GeminiClient {
    pub fn new(api_key: String, model: String) -> Self {
        GeminiClient { api_key, model }
    }

    fn convert_messages(&self, messages: Vec<Message>) -> Vec<Value> {
        let mut result = Vec::new();
        for m in messages {
            if let Some(trs) = m.tool_results {
                // Gemini batches all function responses into one user turn.
                let parts: Vec<Value> = trs.into_iter().map(|tr| json!({
                    "functionResponse": {
                        "name": "run_terminal_command",
                        "response": {
                            "name": "run_terminal_command",
                            "content": tr.content
                        }
                    }
                })).collect();
                result.push(json!({"role": "user", "parts": parts}));
            } else if let Some(tcs) = m.tool_calls {
                let mut parts = Vec::new();
                if !m.content.is_empty() {
                    parts.push(json!({"text": m.content}));
                }
                for tc in tcs {
                    let args: Value = serde_json::from_str(&tc.arguments).unwrap_or(json!({}));
                    parts.push(json!({
                        "functionCall": {
                            "name": tc.name,
                            "args": args
                        }
                    }));
                }
                result.push(json!({
                    "role": "model",
                    "parts": parts
                }));
            } else {
                result.push(json!({
                    "role": if m.role == "assistant" { "model" } else { "user" },
                    "parts": [{"text": m.content}]
                }));
            }
        }
        result
    }
}

#[async_trait]
impl AiClient for GeminiClient {
    async fn chat(&self, system: &str, messages: Vec<Message>, tx: UnboundedSender<AiEvent>) -> Result<()> {
        let url = format!(
            "https://generativelanguage.googleapis.com/v1beta/models/{}:streamGenerateContent?alt=sse&key={}",
            self.model, self.api_key
        );
        let converted = self.convert_messages(messages);
        let body = json!({
            "system_instruction": {"parts": [{"text": system}]},
            "contents": converted,
            "tools": [
                {
                    "function_declarations": [
                        {
                            "name": "run_terminal_command",
                            "description": "Execute a bash command in one of two modes:\n- background=true: Runs on the DAEMON HOST as a subprocess. Output captured silently. Use for read-only diagnostics. Supports sudo via chat.\n- background=false: Runs in the USER'S TERMINAL PANE via tmux send-keys. Visible and interactive. If the user is SSH'd, runs remotely. Sudo requires the user to type password in the pane.",
                            "parameters": {
                                "type": "OBJECT",
                                "properties": {
                                    "command": {"type": "STRING", "description": "The bash command to execute."},
                                    "background": {"type": "BOOLEAN", "description": "true = daemon host subprocess (invisible); false = user's terminal pane (visible, possibly remote)."}
                                },
                                "required": ["command", "background"]
                            }
                        }
                    ]
                }
            ]
        });

        let response = send_with_retry(|| http().post(&url).json(&body)).await?;

        let mut stream = response.bytes_stream();
        let mut leftover = String::new();

        while let Some(chunk) = stream.next().await {
            let bytes = chunk?;
            leftover.push_str(&String::from_utf8_lossy(&bytes));

            while let Some(pos) = leftover.find('\n') {
                let line = leftover[..pos].trim().to_string();
                leftover = leftover[pos + 1..].to_string();

                if let Some(data) = line.strip_prefix("data: ") {
                    if let Ok(v) = serde_json::from_str::<Value>(data) {
                        if let Some(candidates) = v.get("candidates").and_then(|c| c.as_array()) {
                            if let Some(candidate) = candidates.get(0) {
                                if let Some(parts) = candidate["content"].get("parts").and_then(|p| p.as_array()) {
                                    for part in parts {
                                        if let Some(t) = part.get("text").and_then(|text| text.as_str()) {
                                            if !t.is_empty() {
                                                let _ = tx.send(AiEvent::Token(t.to_string()));
                                            }
                                        }
                                        if let Some(call) = part.get("functionCall") {
                                            if call["name"] == "run_terminal_command" {
                                                if let Some(args) = call.get("args") {
                                                    let cmd = args["command"].as_str().unwrap_or_default();
                                                    let bg = args["background"].as_bool().unwrap_or(false);
                                                    let _ = tx.send(AiEvent::ToolCall(next_tool_id(), cmd.to_string(), bg));
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        let _ = tx.send(AiEvent::Done);
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Factory
// ---------------------------------------------------------------------------

pub fn make_client(provider: &str, api_key: String, model: String) -> Box<dyn AiClient> {
    match provider {
        "openai" => Box::new(OpenAiClient::new(api_key, model)),
        "gemini" => Box::new(GeminiClient::new(api_key, model)),
        _ => Box::new(AnthropicClient::new(api_key, model)),
    }
}
