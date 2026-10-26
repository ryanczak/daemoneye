//! Config sections: [hosts.*], [models.*] and pricing, [ai], prompts.

use serde::{Deserialize, Serialize};

/// A remote execution target.  Define one or more named entries in
/// `config.toml` under `[hosts.<name>]`.  The local host is implicit and is
/// **not** configured here.
///
/// Example:
/// ```toml
/// [hosts.web01]
/// ssh = "matt@web01.lan"
/// tags = ["web", "prod"]
///
/// [hosts.db01]
/// ssh = "db01"
/// sudo_scripts = ["pg-failover"]
/// ```
#[derive(Debug, Deserialize, Serialize, Clone, Default)]
pub struct HostEntry {
    /// Any `ssh(1)` destination; `~/.ssh/config` is honoured, so a bare
    /// `Host` alias is valid.  Required — an entry without it is a config
    /// error.
    #[serde(default)]
    pub ssh: String,
    /// Free-form labels for grouping.  Not interpreted by the daemon.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Scripts that may be run under `sudo` on this host, the 1.x
    /// remote-sudoers model. Enforced for ghost runbooks whose `ssh_target`
    /// resolves to this host: a script not named here is refused before it
    /// runs. An **empty list restricts nothing** — every entry written before
    /// this field existed keeps its behaviour.
    #[serde(default)]
    pub sudo_scripts: Vec<String>,
    /// M18 container profile to run this host's commands under, or `"none"`.
    /// Parsed here; **not enforced** — the container work meets the host
    /// model in a later milestone.
    #[serde(default)]
    pub sandbox_profile: Option<String>,
}

/// Per-model AI provider configuration.  Define one or more named entries in
/// `config.toml` under `[models.<name>]`.  A `[models.default]` entry is
/// required; it is used when no model override is in effect.
///
/// Example:
/// ```toml
/// [models.default]
/// provider = "anthropic"
/// model    = "claude-sonnet-4-6"
///
/// [models.opus]
/// provider = "anthropic"
/// model    = "claude-opus-4-6"
///
/// [models.local]
/// provider = "ollama"
/// model    = "llama3:70b"
/// base_url = "http://localhost:11434/v1"
/// context_window_tokens = 8192
/// ```
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ModelEntry {
    /// "anthropic" | "openai" | "gemini" | "ollama" | "lmstudio"
    #[serde(default = "default_provider")]
    pub provider: String,
    /// API key.  Empty → resolved from the provider's environment variable
    /// (`ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `GEMINI_API_KEY`).
    #[serde(default)]
    pub api_key: String,
    /// Model identifier passed to the API (e.g. `"claude-sonnet-4-6"`,
    /// `"gpt-4o"`, `"gemini-2.5-pro"`, `"llama3:70b"`).
    #[serde(default = "default_model")]
    pub model: String,
    /// Override the API base URL.  Useful for custom Ollama/LMStudio hosts or
    /// any OpenAI-compatible proxy.
    /// Defaults: ollama → http://localhost:11434/v1,
    ///           lmstudio → http://localhost:1234/v1,
    ///           openai → https://api.openai.com/v1 (or $OPENAI_API_BASE).
    #[serde(default)]
    pub base_url: Option<String>,
    /// Override the model's context-window size in tokens.
    /// Set this for local models where the automatic lookup is wrong.
    #[serde(default)]
    pub context_window_tokens: Option<u32>,
    /// Maximum tokens the model may generate per response.
    /// Defaults to 8192; raise it for long-form answers or models whose
    /// reasoning tokens count against the output budget.
    #[serde(default)]
    pub max_tokens: Option<u32>,
    /// Per-1M-token input cost in USD. None = unknown (will log warning).
    /// Local providers (lmstudio, ollama) should set this to Some(0.0).
    #[serde(default)]
    pub input_cost_per_mtok: Option<f64>,
    /// Per-1M-token output cost in USD.
    #[serde(default)]
    pub output_cost_per_mtok: Option<f64>,
    /// Cache-read rate (Anthropic ephemeral cache; Gemini implicit cache).
    /// Typically ~10% of input rate for Anthropic.
    #[serde(default)]
    pub cache_read_cost_per_mtok: Option<f64>,
    /// Cache-write rate (Anthropic cache creation; ~125% of input rate).
    #[serde(default)]
    pub cache_write_cost_per_mtok: Option<f64>,
}

fn default_provider() -> String {
    "anthropic".to_string()
}
fn default_model() -> String {
    "claude-sonnet-4-6".to_string()
}

impl Default for ModelEntry {
    fn default() -> Self {
        ModelEntry {
            provider: default_provider(),
            api_key: String::new(),
            model: default_model(),
            base_url: None,
            context_window_tokens: None,
            max_tokens: None,
            input_cost_per_mtok: None,
            output_cost_per_mtok: None,
            cache_read_cost_per_mtok: None,
            cache_write_cost_per_mtok: None,
        }
    }
}

impl ModelEntry {
    /// The environment variable name that holds the API key for this provider.
    pub fn api_key_env_var(&self) -> &'static str {
        match self.provider.as_str() {
            "openai" => "OPENAI_API_KEY",
            "gemini" => "GEMINI_API_KEY",
            "ollama" | "lmstudio" => "",
            _ => "ANTHROPIC_API_KEY",
        }
    }

    /// Resolve the API key: explicit config value → env var → dummy for local providers.
    pub fn resolve_api_key(&self) -> String {
        if !self.api_key.is_empty() {
            return self.api_key.clone();
        }
        // Local providers don't require a real key — use a dummy so the OpenAI
        // client can still set the Authorization header without panicking.
        match self.provider.as_str() {
            "ollama" | "lmstudio" => return "local".to_string(),
            _ => {}
        }
        let env_var = self.api_key_env_var();
        if env_var.is_empty() {
            return String::new();
        }
        std::env::var(env_var).unwrap_or_default()
    }

    /// Resolve the effective API base URL.
    /// Priority: explicit `base_url` → provider default → $OPENAI_API_BASE (openai only).
    pub fn effective_base_url(&self) -> String {
        if let Some(ref u) = self.base_url {
            return u.clone();
        }
        match self.provider.as_str() {
            "ollama" => "http://localhost:11434/v1".to_string(),
            "lmstudio" => "http://localhost:1234/v1".to_string(),
            "openai" => std::env::var("OPENAI_API_BASE")
                .unwrap_or_else(|_| "https://api.openai.com/v1".to_string()),
            _ => String::new(),
        }
    }

    /// Maximum tokens the model may generate per response.
    /// `max_tokens` from the config wins; otherwise 8192.
    pub fn effective_max_tokens(&self) -> u32 {
        self.max_tokens.unwrap_or(8192)
    }

    /// Context-window size in tokens.  `context_window_tokens` wins; otherwise
    /// a built-in table is consulted.  Local/unknown models default to 32 768.
    pub fn context_window(&self) -> u32 {
        if let Some(override_val) = self.context_window_tokens {
            return override_val;
        }
        let m = self.model.as_str();
        if m.starts_with("claude") {
            200_000
        } else if m.starts_with("gemini-1.5-pro") {
            2_000_000
        } else if m.starts_with("gemini") {
            1_000_000
        } else if m.starts_with("gpt-4o") || m.starts_with("gpt-4-turbo") {
            128_000
        } else if m.starts_with("gpt-3.5") {
            16_000
        } else {
            32_768
        }
    }

    /// Resolve pricing for this model entry.
    /// Returns a `Pricing` struct with rates from the user's config, or `None`
    /// if no cost fields are set (cost accounting will report `$0+` for the model).
    /// Local providers always return zero pricing.
    pub fn pricing(&self) -> Option<Pricing> {
        if self.provider == "ollama" || self.provider == "lmstudio" {
            return Some(Pricing::zero());
        }

        let has_user = self.input_cost_per_mtok.is_some()
            || self.output_cost_per_mtok.is_some()
            || self.cache_read_cost_per_mtok.is_some()
            || self.cache_write_cost_per_mtok.is_some();

        if has_user {
            return Some(Pricing {
                input_per_mtok: self.input_cost_per_mtok.unwrap_or(0.0),
                output_per_mtok: self.output_cost_per_mtok.unwrap_or(0.0),
                cache_read_per_mtok: self.cache_read_cost_per_mtok.unwrap_or(0.0),
                cache_write_per_mtok: self.cache_write_cost_per_mtok.unwrap_or(0.0),
                source: PricingSource::UserConfig,
            });
        }

        None
    }
}

// ---------------------------------------------------------------------------
// Pricing schema
// ---------------------------------------------------------------------------

/// Where a pricing rate originated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PricingSource {
    /// Rate came from the user's `config.toml` `[models.<name>]` section.
    /// Alias covers records written before pricing moved to config-only.
    #[serde(alias = "BuiltinDefault")]
    UserConfig,
    /// Provider is local (ollama, lmstudio) — all rates are 0.0.
    Local,
    /// Model is not recognized and no user pricing was set.
    Unknown,
}

/// Resolved per-model pricing rates (per million tokens, in USD).
///
/// All fields are concrete `f64` values — the resolution from `Option<f64>`
/// config fields to defaults happens in `ModelEntry::pricing()`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pricing {
    /// Input cost per 1M tokens (USD).
    pub input_per_mtok: f64,
    /// Output cost per 1M tokens (USD).
    pub output_per_mtok: f64,
    /// Cache-read cost per 1M tokens (USD).
    pub cache_read_per_mtok: f64,
    /// Cache-write cost per 1M tokens (USD).
    pub cache_write_per_mtok: f64,
    /// Where these rates came from.
    pub source: PricingSource,
}

impl Pricing {
    /// Constructor for local providers — all rates are zero.
    pub fn zero() -> Self {
        Pricing {
            input_per_mtok: 0.0,
            output_per_mtok: 0.0,
            cache_read_per_mtok: 0.0,
            cache_write_per_mtok: 0.0,
            source: PricingSource::Local,
        }
    }
}

pub(super) fn default_models() -> std::collections::HashMap<String, ModelEntry> {
    let mut m = std::collections::HashMap::new();
    m.insert("default".to_string(), ModelEntry::default());
    m
}

/// Global AI settings from the `[ai]` section of `config.toml`.
/// Provider and model configuration has moved to `[models.<name>]` entries.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct AiConfig {
    /// Name of a prompt file in `~/.daemoneye/prompts/` (without `.toml`).
    /// Defaults to `"sre"`.
    #[serde(default = "default_prompt")]
    pub prompt: String,
    /// Max wait in seconds for the first real token of a response. Covers slow
    /// prefill on long prompts. Defaults to 600.
    #[serde(default = "default_first_token_timeout_secs")]
    pub first_token_timeout_secs: u64,
    /// Max gap in seconds between chunks once the stream has produced a token.
    /// Defaults to 240.
    #[serde(default = "default_stream_idle_timeout_secs")]
    pub stream_idle_timeout_secs: u64,
    /// TCP/TLS connect budget in seconds. Defaults to 30.
    #[serde(default = "default_connect_timeout_secs")]
    pub connect_timeout_secs: u64,
}

fn default_prompt() -> String {
    "sre".to_string()
}

fn default_first_token_timeout_secs() -> u64 {
    600
}

fn default_stream_idle_timeout_secs() -> u64 {
    240
}

fn default_connect_timeout_secs() -> u64 {
    30
}

impl Default for AiConfig {
    fn default() -> Self {
        AiConfig {
            prompt: default_prompt(),
            first_token_timeout_secs: default_first_token_timeout_secs(),
            stream_idle_timeout_secs: default_stream_idle_timeout_secs(),
            connect_timeout_secs: default_connect_timeout_secs(),
        }
    }
}

// ---------------------------------------------------------------------------
// Prompt definitions
// ---------------------------------------------------------------------------

/// A loaded prompt definition (system message).
/// Loaded from `~/.daemoneye/prompts/<name>.toml` or falling back to built-ins.
#[derive(Debug, Deserialize, Clone)]
pub struct PromptDef {
    pub system: String,
}

impl PromptDef {
    /// Fallback used when no prompt file can be found.
    pub fn builtin_minimal() -> Self {
        PromptDef {
            system: "You are a helpful terminal assistant. \
                     When suggesting commands put each on its own line."
                .to_string(),
        }
    }
}
