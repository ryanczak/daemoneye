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

/// Monotonically increasing counter used to generate unique tool call IDs
/// within the daemon process lifetime (e.g. `tc_1`, `tc_2`, …).
static TOOL_CALL_ID: AtomicU64 = AtomicU64::new(1);

/// Generate the next unique tool call ID (process-scoped, not session-scoped).
pub fn next_tool_id() -> String {
    format!("tc_{}", TOOL_CALL_ID.fetch_add(1, Ordering::Relaxed))
}

/// Single shared HTTP client for the lifetime of the process.
/// `reqwest::Client` manages a connection pool internally; creating one per
/// request throws away that pool on every turn.
static HTTP_CLIENT: OnceLock<reqwest::Client> = OnceLock::new();

/// Return a reference to the process-wide HTTP client, initialising it on first call.
fn http() -> &'static reqwest::Client {
    HTTP_CLIENT.get_or_init(reqwest::Client::new)
}

// ---------------------------------------------------------------------------
// Message types for conversation history
// ---------------------------------------------------------------------------

/// A tool call emitted by the AI during a response stream.
/// Stored in `Message::tool_calls` and echoed back as a tool-result message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    /// Unique identifier for this invocation (used to correlate results).
    pub id: String,
    /// Always `"run_terminal_command"` — the single tool DaemonEye exposes.
    pub name: String,
    /// JSON-encoded arguments: `{"command": "...", "background": bool}`.
    pub arguments: String,
}

/// The captured output of a completed tool call, sent back to the AI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    /// Matches the `ToolCall::id` this result belongs to.
    pub tool_call_id: String,
    /// stdout/stderr text (normalised and potentially truncated).
    pub content: String,
}

/// A single turn in the conversation history.
/// Serialised to JSONL in `~/.daemoneye/sessions/<id>.jsonl` for persistence.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    /// `"user"` or `"assistant"`.
    pub role: String,
    /// Plain text content of the message.
    pub content: String,
    /// Tool calls emitted by the assistant (present only on assistant turns
    /// that triggered at least one tool invocation).
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

/// Events streamed from the AI client task back to the daemon's request handler.
#[derive(Debug)]
pub enum AiEvent {
    /// A partial response token to forward to the client immediately.
    Token(String),
    /// A complete `run_terminal_command` tool call: (id, command, background, target_pane).
    ToolCall(String, String, bool, Option<String>),
    /// Schedule a command to run once or on a repeating interval.
    ScheduleCommand {
        id: String,
        name: String,
        command: String,
        is_script: bool,
        run_at: Option<String>,
        interval: Option<String>,
        runbook: Option<String>,
    },
    /// Request the current list of scheduled jobs.
    ListSchedules { id: String },
    /// Cancel a scheduled job by UUID.
    CancelSchedule { id: String, job_id: String },
    /// Write (create or update) a script in `~/.daemoneye/scripts/`.
    WriteScript { id: String, script_name: String, content: String },
    /// Request the list of scripts in `~/.daemoneye/scripts/`.
    ListScripts { id: String },
    /// Read the content of a named script.
    ReadScript { id: String, script_name: String },
    /// Passively watch a background pane for output changes (P8).
    WatchPane { id: String, pane_id: String, timeout_secs: u64 },
    /// The stream finished normally; no more events will follow.
    Done,
    /// A non-retryable error terminated the stream.
    Error(String),
}

// ---------------------------------------------------------------------------
// Trait
// ---------------------------------------------------------------------------

/// Abstraction over AI provider backends (Anthropic, OpenAI, Gemini).
///
/// Implementors stream response events through an unbounded channel rather than
/// returning a value so that the daemon can forward tokens to the client in
/// real time without buffering the full response first.
#[async_trait]
pub trait AiClient: Send + Sync {
    /// Stream response tokens and tool calls by sending [`AiEvent`] values to `tx`.
    /// Must send exactly one [`AiEvent::Done`] or [`AiEvent::Error`] as the final event.
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
             - background=true: Runs in a dedicated tmux window on the DAEMON HOST. Output is captured silently and returned to you. Use for read-only diagnostics (ls, ps, cat, grep, df, curl, etc.). If the user is SSH'd into a remote host, this still runs locally on the daemon machine. Supports sudo: the user will be prompted for their password in the chat interface.\n\
             - background=false (default): Injects the command into the USER'S TERMINAL PANE via tmux send-keys. The command is visible and interactive. Use for state-changing commands, service restarts, file edits, or anything that must run on the user's active host. If the user's pane is SSH'd to a remote machine, the command runs there. Supports sudo: the user types their password directly in the terminal pane.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "command": {"type": "string", "description": "The bash command to execute."},
                    "background": {"type": "boolean", "default": false, "description": "true = daemon host tmux window (captured output); false = user's terminal pane (visible, interactive, possibly remote). Defaults to false."},
                    "target_pane": {"type": "string", "description": "Optional: tmux pane ID (e.g. \"%3\") to target for foreground commands. Only specify when context shows multiple panes and the command must run in a specific one. Background commands always run on the daemon host — do not set target_pane for them."}
                },
                "required": ["command"]
            }
        },
        {
            "name": "schedule_command",
            "description": "Schedule a shell command (or named script) to run once at a specific UTC time or repeatedly on an interval. For watchdog monitoring, specify a runbook name to enable AI analysis of the output.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "name": {"type": "string", "description": "Human-readable name for this scheduled job."},
                    "command": {"type": "string", "description": "Shell command to run, or script name if is_script=true."},
                    "is_script": {"type": "boolean", "default": false, "description": "If true, 'command' is a script name in ~/.daemoneye/scripts/ to execute."},
                    "run_at": {"type": "string", "description": "ISO 8601 UTC datetime for a one-shot job, e.g. '2026-03-01T15:00:00Z'. Omit if using interval."},
                    "interval": {"type": "string", "description": "ISO 8601 duration for repeating jobs, e.g. PT5M (5 min), PT1H (1 hour), P1D (1 day). Omit if using run_at."},
                    "runbook": {"type": "string", "description": "Optional name of a runbook in ~/.daemoneye/runbooks/ for watchdog AI analysis of command output."}
                },
                "required": ["name", "command"]
            }
        },
        {
            "name": "list_schedules",
            "description": "Return the current list of scheduled jobs with their status, schedule, and next run time.",
            "input_schema": {
                "type": "object",
                "properties": {}
            }
        },
        {
            "name": "cancel_schedule",
            "description": "Cancel a scheduled job by its UUID. The job will no longer fire.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "id": {"type": "string", "description": "UUID of the scheduled job to cancel."}
                },
                "required": ["id"]
            }
        },
        {
            "name": "write_script",
            "description": "Create or update a reusable script in ~/.daemoneye/scripts/. The user will be shown the full content and must approve before it is written. Scripts are saved with chmod 700.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "script_name": {"type": "string", "description": "Filename for the script (e.g. 'check-disk.sh')."},
                    "content": {"type": "string", "description": "Full content of the script, including the shebang line."}
                },
                "required": ["script_name", "content"]
            }
        },
        {
            "name": "list_scripts",
            "description": "Return the list of scripts in ~/.daemoneye/scripts/ with their sizes.",
            "input_schema": {
                "type": "object",
                "properties": {}
            }
        },
        {
            "name": "read_script",
            "description": "Read the content of a script from ~/.daemoneye/scripts/.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "script_name": {"type": "string", "description": "Name of the script to read."}
                },
                "required": ["script_name"]
            }
        },
        {
            "name": "watch_pane",
            "description": "Passively monitor a background tmux pane for output changes. Blocks until the pane content changes or timeout_secs elapses, then returns the pane's updated content. Use this to wait for a long-running process to produce output (e.g. a build, test run, or log tail) without polling manually.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "pane_id": {"type": "string", "description": "Tmux pane ID to monitor (e.g. \"%3\"). Get IDs from [BACKGROUND PANE] context blocks."},
                    "timeout_secs": {"type": "integer", "description": "Maximum seconds to wait for output. Defaults to 300 (5 minutes)."}
                },
                "required": ["pane_id"]
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
                 - background=true: Runs in a dedicated tmux window on the DAEMON HOST. Output captured silently. Use for read-only diagnostics. Supports sudo via chat interface.\n\
                 - background=false (default): Injects the command into the USER'S TERMINAL PANE via tmux. Visible and interactive. Use for state-changing commands. Sudo requires the user to type password in the pane.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "command": {"type": "string", "description": "The bash command to execute."},
                        "background": {"type": "boolean", "default": false, "description": "true = daemon host tmux window (captured); false = user's terminal pane (visible, interactive). Defaults to false."},
                        "target_pane": {"type": "string", "description": "Optional: tmux pane ID (e.g. \"%3\") to target for foreground commands."}
                    },
                    "required": ["command"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "schedule_command",
                "description": "Schedule a command or script to run once or on a repeating interval.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "name": {"type": "string"},
                        "command": {"type": "string"},
                        "is_script": {"type": "boolean", "default": false},
                        "run_at": {"type": "string"},
                        "interval": {"type": "string"},
                        "runbook": {"type": "string"}
                    },
                    "required": ["name", "command"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "list_schedules",
                "description": "Return the current list of scheduled jobs.",
                "parameters": {"type": "object", "properties": {}}
            }
        },
        {
            "type": "function",
            "function": {
                "name": "cancel_schedule",
                "description": "Cancel a scheduled job by UUID.",
                "parameters": {
                    "type": "object",
                    "properties": {"id": {"type": "string"}},
                    "required": ["id"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "write_script",
                "description": "Create or update a reusable script in ~/.daemoneye/scripts/ (requires user approval).",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "script_name": {"type": "string"},
                        "content": {"type": "string"}
                    },
                    "required": ["script_name", "content"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "list_scripts",
                "description": "Return the list of scripts in ~/.daemoneye/scripts/.",
                "parameters": {"type": "object", "properties": {}}
            }
        },
        {
            "type": "function",
            "function": {
                "name": "read_script",
                "description": "Read the content of a named script.",
                "parameters": {
                    "type": "object",
                    "properties": {"script_name": {"type": "string"}},
                    "required": ["script_name"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "watch_pane",
                "description": "Monitor a background tmux pane for output changes. Returns when activity is detected or timeout expires.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "pane_id": {"type": "string", "description": "Tmux pane ID (e.g. \"%3\") from [BACKGROUND PANE] context blocks."},
                        "timeout_secs": {"type": "integer", "description": "Max seconds to wait. Defaults to 300."}
                    },
                    "required": ["pane_id"]
                }
            }
        }
    ])
}

// ---------------------------------------------------------------------------
// Tool event dispatcher (shared by all three provider backends)
// ---------------------------------------------------------------------------

/// Given a tool call ID, name, and parsed arguments, produce the corresponding
/// [`AiEvent`].  Returns `None` for unrecognised tool names.
fn dispatch_tool_event(id: &str, name: &str, args: &Value) -> Option<AiEvent> {
    match name {
        "run_terminal_command" => {
            let cmd = args["command"].as_str()?;
            let bg = args["background"].as_bool().unwrap_or(false);
            let target = args["target_pane"].as_str().map(|s| s.to_string());
            Some(AiEvent::ToolCall(id.to_string(), cmd.to_string(), bg, target))
        }
        "schedule_command" => Some(AiEvent::ScheduleCommand {
            id: id.to_string(),
            name: args["name"].as_str().unwrap_or("unnamed").to_string(),
            command: args["command"].as_str().unwrap_or("").to_string(),
            is_script: args["is_script"].as_bool().unwrap_or(false),
            run_at: args["run_at"].as_str().map(|s| s.to_string()),
            interval: args["interval"].as_str().map(|s| s.to_string()),
            runbook: args["runbook"].as_str().map(|s| s.to_string()),
        }),
        "list_schedules" => Some(AiEvent::ListSchedules { id: id.to_string() }),
        "cancel_schedule" => Some(AiEvent::CancelSchedule {
            id: id.to_string(),
            job_id: args["id"].as_str().unwrap_or("").to_string(),
        }),
        "write_script" => Some(AiEvent::WriteScript {
            id: id.to_string(),
            script_name: args["script_name"].as_str().unwrap_or("").to_string(),
            content: args["content"].as_str().unwrap_or("").to_string(),
        }),
        "list_scripts" => Some(AiEvent::ListScripts { id: id.to_string() }),
        "read_script" => Some(AiEvent::ReadScript {
            id: id.to_string(),
            script_name: args["script_name"].as_str().unwrap_or("").to_string(),
        }),
        "watch_pane" => Some(AiEvent::WatchPane {
            id: id.to_string(),
            pane_id: args["pane_id"].as_str().unwrap_or("").to_string(),
            timeout_secs: args["timeout_secs"].as_u64().unwrap_or(300),
        }),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Anthropic
// ---------------------------------------------------------------------------

/// Anthropic API backend (Claude family).
pub struct AnthropicClient {
    api_key: String,
    model: String,
}

impl AnthropicClient {
    /// Create a new Anthropic client for the given model.
    pub fn new(api_key: String, model: String) -> Self {
        AnthropicClient { api_key, model }
    }

    /// Convert the internal `Message` history into Anthropic's JSON format.
    ///
    /// Anthropic's message structure differs from the internal representation:
    /// - Tool results must be batched into a single `role: "user"` message with
    ///   an array of `tool_result` content blocks.
    /// - Tool calls from the assistant must be expressed as `tool_use` content
    ///   blocks alongside any plain text content.
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
        let mut tool_name = String::new();
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
                                    tool_name = v["content_block"]["name"]
                                        .as_str().unwrap_or("").to_string();
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
                                if let Ok(args) = serde_json::from_str::<Value>(&tool_args) {
                                    if let Some(ev) = dispatch_tool_event(&tool_id, &tool_name, &args) {
                                        let _ = tx.send(ev);
                                    }
                                }
                                tool_id.clear();
                                tool_name.clear();
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

/// OpenAI-compatible API backend (GPT family, or any OpenAI-compatible endpoint).
/// The `base_url` defaults to the official OpenAI API but can be overridden for
/// local inference servers (e.g. Ollama, vLLM) via the `OPENAI_BASE_URL` env var.
pub struct OpenAiClient {
    api_key: String,
    model: String,
    base_url: String,
}

impl OpenAiClient {
    /// Create a new OpenAI-compatible client, reading `OPENAI_BASE_URL` for the endpoint.
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
        let mut tool_name = String::new();
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
                                            // Flush previous tool call
                                            if let Ok(args) = serde_json::from_str::<Value>(&tool_args) {
                                                if let Some(ev) = dispatch_tool_event(&tool_id, &tool_name, &args) {
                                                    let _ = tx.send(ev);
                                                }
                                            }
                                        }
                                        tool_id = id.to_string();
                                        tool_args.clear();
                                    }
                                    if let Some(f) = tc.get("function") {
                                        if let Some(n) = f.get("name").and_then(|n| n.as_str()) {
                                            if !n.is_empty() { tool_name = n.to_string(); }
                                        }
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
            if let Ok(args) = serde_json::from_str::<Value>(&tool_args) {
                if let Some(ev) = dispatch_tool_event(&tool_id, &tool_name, &args) {
                    let _ = tx.send(ev);
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

/// Parse a Python-style function call from a Gemini `MALFORMED_FUNCTION_CALL`
/// `finishMessage`, e.g.:
///   `Malformed function call: print(default_api.run_terminal_command(command='cat README.md', background=false))`
///
/// Returns `(command, background)` if parsing succeeds, `None` otherwise.
fn parse_malformed_gemini_call(msg: &str) -> Option<(String, bool)> {
    use regex::Regex;
    use std::sync::OnceLock;
    // Captures the command string (allowing escaped single quotes) and the background bool.
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(r#"run_terminal_command\(command='((?:[^'\\]|\\.)*)'\s*,\s*background=(true|false)\)"#)
            .expect("valid regex")
    });
    let caps = re.captures(msg)?;
    let cmd = caps[1].replace("\\'", "'");
    let bg  = &caps[2] == "true";
    Some((cmd, bg))
}

/// Google Gemini API backend.
pub struct GeminiClient {
    api_key: String,
    model: String,
}

impl GeminiClient {
    /// Create a new Gemini client.
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
                            "description": "Execute a bash command in one of two modes:\n- background=true: Runs in a dedicated tmux window on the DAEMON HOST. Output captured silently. Use for read-only diagnostics. Supports sudo via chat.\n- background=false (default): Runs in the USER'S TERMINAL PANE via tmux send-keys. Visible and interactive. If the user is SSH'd, runs remotely. Sudo requires the user to type password in the pane.",
                            "parameters": {
                                "type": "OBJECT",
                                "properties": {
                                    "command": {"type": "STRING", "description": "The bash command to execute."},
                                    "background": {"type": "BOOLEAN", "description": "true = daemon host tmux window (captured); false = user's terminal pane (visible, possibly remote). Defaults to false."},
                                    "target_pane": {"type": "STRING", "description": "Optional: tmux pane ID for foreground commands."}
                                },
                                "required": ["command"]
                            }
                        },
                        {
                            "name": "schedule_command",
                            "description": "Schedule a command or script to run once or on a repeating interval.",
                            "parameters": {
                                "type": "OBJECT",
                                "properties": {
                                    "name": {"type": "STRING"},
                                    "command": {"type": "STRING"},
                                    "is_script": {"type": "BOOLEAN"},
                                    "run_at": {"type": "STRING"},
                                    "interval": {"type": "STRING"},
                                    "runbook": {"type": "STRING"}
                                },
                                "required": ["name", "command"]
                            }
                        },
                        {
                            "name": "list_schedules",
                            "description": "Return the current list of scheduled jobs.",
                            "parameters": {"type": "OBJECT", "properties": {}}
                        },
                        {
                            "name": "cancel_schedule",
                            "description": "Cancel a scheduled job by UUID.",
                            "parameters": {
                                "type": "OBJECT",
                                "properties": {"id": {"type": "STRING"}},
                                "required": ["id"]
                            }
                        },
                        {
                            "name": "write_script",
                            "description": "Create or update a reusable script (requires user approval).",
                            "parameters": {
                                "type": "OBJECT",
                                "properties": {
                                    "script_name": {"type": "STRING"},
                                    "content": {"type": "STRING"}
                                },
                                "required": ["script_name", "content"]
                            }
                        },
                        {
                            "name": "list_scripts",
                            "description": "Return the list of scripts in ~/.daemoneye/scripts/.",
                            "parameters": {"type": "OBJECT", "properties": {}}
                        },
                        {
                            "name": "read_script",
                            "description": "Read the content of a named script.",
                            "parameters": {
                                "type": "OBJECT",
                                "properties": {"script_name": {"type": "STRING"}},
                                "required": ["script_name"]
                            }
                        },
                        {
                            "name": "watch_pane",
                            "description": "Monitor a background tmux pane for output changes. Returns when activity is detected or timeout expires.",
                            "parameters": {
                                "type": "OBJECT",
                                "properties": {
                                    "pane_id": {"type": "STRING", "description": "Tmux pane ID (e.g. \"%3\") from BACKGROUND PANE context."},
                                    "timeout_secs": {"type": "INTEGER", "description": "Max seconds to wait. Defaults to 300."}
                                },
                                "required": ["pane_id"]
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
                                // Gemini 2.5 Flash (thinking model) sometimes produces a
                                // Python-style function call string instead of a structured
                                // functionCall block.  The API signals this with finishReason
                                // "MALFORMED_FUNCTION_CALL" and a finishMessage containing
                                // the raw call text.  Recover by parsing the finishMessage.
                                if candidate.get("finishReason").and_then(|r| r.as_str())
                                    == Some("MALFORMED_FUNCTION_CALL")
                                {
                                    if let Some(msg) = candidate
                                        .get("finishMessage")
                                        .and_then(|m| m.as_str())
                                    {
                                        if let Some((cmd, bg)) = parse_malformed_gemini_call(msg) {
                                            let _ = tx.send(AiEvent::ToolCall(next_tool_id(), cmd, bg, None));
                                        } else {
                                            let _ = tx.send(AiEvent::Error(format!(
                                                "Gemini produced a malformed function call \
                                                 that could not be recovered.\n\
                                                 Raw: {msg}"
                                            )));
                                            return Ok(());
                                        }
                                    }
                                    continue;
                                }

                                if let Some(parts) = candidate["content"].get("parts").and_then(|p| p.as_array()) {
                                    for part in parts {
                                        if let Some(t) = part.get("text").and_then(|text| text.as_str()) {
                                            if !t.is_empty() {
                                                let _ = tx.send(AiEvent::Token(t.to_string()));
                                            }
                                        }
                                        if let Some(call) = part.get("functionCall") {
                                            let fn_name = call["name"].as_str().unwrap_or("");
                                            if let Some(args) = call.get("args") {
                                                let id = next_tool_id();
                                                if let Some(ev) = dispatch_tool_event(&id, fn_name, args) {
                                                    let _ = tx.send(ev);
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

/// Construct an [`AiClient`] for the given provider name.
/// Defaults to Anthropic for any unrecognised provider string.
pub fn make_client(provider: &str, api_key: String, model: String) -> Box<dyn AiClient> {
    match provider {
        "openai" => Box::new(OpenAiClient::new(api_key, model)),
        "gemini" => Box::new(GeminiClient::new(api_key, model)),
        _ => Box::new(AnthropicClient::new(api_key, model)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user_msg(content: &str) -> Message {
        Message { role: "user".to_string(), content: content.to_string(), tool_calls: None, tool_results: None }
    }
    fn assistant_msg(content: &str) -> Message {
        Message { role: "assistant".to_string(), content: content.to_string(), tool_calls: None, tool_results: None }
    }
    fn client() -> AnthropicClient {
        AnthropicClient::new("key".to_string(), "model".to_string())
    }

    // ── AnthropicClient::convert_messages ─────────────────────────────────────

    #[test]
    fn convert_plain_conversation() {
        let msgs = vec![user_msg("hi"), assistant_msg("hello")];
        let out = client().convert_messages(msgs);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0]["role"], "user");
        assert_eq!(out[0]["content"], "hi");
        assert_eq!(out[1]["role"], "assistant");
    }

    #[test]
    fn convert_tool_call_becomes_tool_use_block() {
        let tc = ToolCall {
            id: "tc_1".to_string(),
            name: "run_terminal_command".to_string(),
            arguments: r#"{"command":"ls","background":true}"#.to_string(),
        };
        let msg = Message {
            role: "assistant".to_string(),
            content: "running ls".to_string(),
            tool_calls: Some(vec![tc]),
            tool_results: None,
        };
        let out = client().convert_messages(vec![msg]);
        assert_eq!(out.len(), 1);
        // Content should be an array with text + tool_use blocks.
        let content = out[0]["content"].as_array().expect("content array");
        assert!(content.iter().any(|b| b["type"] == "tool_use"), "no tool_use block");
        assert!(content.iter().any(|b| b["type"] == "text"), "no text block");
    }

    #[test]
    fn convert_tool_call_without_text_omits_text_block() {
        let tc = ToolCall {
            id: "tc_2".to_string(),
            name: "run_terminal_command".to_string(),
            arguments: r#"{"command":"pwd","background":false}"#.to_string(),
        };
        let msg = Message {
            role: "assistant".to_string(),
            content: String::new(), // no prose
            tool_calls: Some(vec![tc]),
            tool_results: None,
        };
        let out = client().convert_messages(vec![msg]);
        let content = out[0]["content"].as_array().expect("content array");
        assert!(!content.iter().any(|b| b["type"] == "text"), "empty text block should be omitted");
    }

    #[test]
    fn convert_tool_results_become_user_message_with_tool_result_blocks() {
        let tr = ToolResult {
            tool_call_id: "tc_1".to_string(),
            content: "output here".to_string(),
        };
        let msg = Message {
            role: "user".to_string(),
            content: String::new(),
            tool_calls: None,
            tool_results: Some(vec![tr]),
        };
        let out = client().convert_messages(vec![msg]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0]["role"], "user");
        let content = out[0]["content"].as_array().expect("content array");
        assert_eq!(content[0]["type"], "tool_result");
        assert_eq!(content[0]["tool_use_id"], "tc_1");
        assert_eq!(content[0]["content"], "output here");
    }

    // ── make_client dispatch ──────────────────────────────────────────────────

    #[test]
    fn make_client_unknown_defaults_to_anthropic() {
        // We just verify make_client doesn't panic for unknown providers.
        let _c = make_client("unknown_provider", "key".to_string(), "model".to_string());
    }

    #[test]
    fn make_client_openai() {
        let _c = make_client("openai", "key".to_string(), "gpt-4o".to_string());
    }

    #[test]
    fn make_client_gemini() {
        let _c = make_client("gemini", "key".to_string(), "gemini-2.0-flash".to_string());
    }

    // ── Message serialization ─────────────────────────────────────────────────

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
        // `tool_calls` and `tool_results` should not appear in the JSON at all.
        assert!(!json.contains("tool_calls"));
        assert!(!json.contains("tool_results"));
    }

    #[test]
    fn tool_call_roundtrip() {
        let tc = ToolCall {
            id: "tc_99".to_string(),
            name: "run_terminal_command".to_string(),
            arguments: r#"{"command":"echo hi","background":true}"#.to_string(),
        };
        let json = serde_json::to_string(&tc).unwrap();
        let back: ToolCall = serde_json::from_str(&json).unwrap();
        assert_eq!(back.id, "tc_99");
        assert_eq!(back.name, "run_terminal_command");
    }

    // ── parse_malformed_gemini_call ───────────────────────────────────────────

    #[test]
    fn parse_malformed_gemini_call_basic() {
        let msg = "Malformed function call: print(default_api.run_terminal_command(command='cat README.md', background=false))";
        let result = parse_malformed_gemini_call(msg);
        assert_eq!(result, Some(("cat README.md".to_string(), false)));
    }

    #[test]
    fn parse_malformed_gemini_call_background_true() {
        let msg = "Malformed function call: print(default_api.run_terminal_command(command='df -h', background=true))";
        let result = parse_malformed_gemini_call(msg);
        assert_eq!(result, Some(("df -h".to_string(), true)));
    }

    #[test]
    fn parse_malformed_gemini_call_escaped_quote_in_command() {
        let msg = r"Malformed function call: print(default_api.run_terminal_command(command='echo \'hello\'', background=false))";
        let result = parse_malformed_gemini_call(msg);
        assert_eq!(result, Some(("echo 'hello'".to_string(), false)));
    }

    #[test]
    fn parse_malformed_gemini_call_unrecognised_format_returns_none() {
        let msg = "something completely different";
        assert!(parse_malformed_gemini_call(msg).is_none());
    }
}
