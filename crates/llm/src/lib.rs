//! LLM backends for voyager (ADR-0004).
//!
//! Every backend implements [`LlmClient`]. Backends are selected with a
//! spec string (CLI `--llm` or `[llm]` in the config file):
//!
//! | spec                       | backend                                   |
//! |----------------------------|-------------------------------------------|
//! | `pi` / `claude` / `codex`  | harness shell-out (reuses the agent's LLM) |
//! | `ollama:<model>`           | local Ollama (OpenAI-compatible endpoint)  |
//! | `openai-compat` + base_url | any custom OpenAI-compatible endpoint      |
//! | `<provider>:<model>`       | models.dev provider, e.g. `openrouter:anthropic/claude-sonnet-4.5` |

pub mod harness;
pub mod models_dev;
pub mod ollama;
pub mod openai_compat;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use vygr_core::config::LlmConf;
use vygr_core::VygrError;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: Role,
    pub content: String,
}

impl ChatMessage {
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: Role::System,
            content: content.into(),
        }
    }
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: content.into(),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct CompletionRequest {
    pub messages: Vec<ChatMessage>,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f64>,
    /// Ask JSON-object-capable backends for `response_format` (M2.5).
    /// Backends without support simply ignore it; callers must still
    /// instruct the model to emit JSON and validate the reply.
    pub json_object: bool,
}

#[derive(Debug, Clone)]
pub struct CompletionResponse {
    pub content: String,
    pub model: String,
    pub usage: Option<TokenUsage>,
    /// Cost of this call when the backend knows its price table.
    pub cost_usd: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TokenUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

#[async_trait]
pub trait LlmClient: Send + Sync {
    async fn complete(&self, req: &CompletionRequest) -> Result<CompletionResponse, VygrError>;
    /// Human-readable one-liner for logs and reports.
    fn describe(&self) -> String;

    /// Upper-bound cost estimate for a call of roughly `prompt_chars`
    /// with `max_tokens` of generation, when the backend knows its price
    /// table. Budget guards call this before committing to a call.
    fn estimate_cost_usd(&self, prompt_chars: usize, max_tokens: Option<u32>) -> Option<f64> {
        let _ = (prompt_chars, max_tokens);
        None
    }
}

/// Resolve an LLM backend from a spec string and the file configuration.
pub async fn resolve(
    spec: Option<&str>,
    cfg: &LlmConf,
    http: &reqwest::Client,
) -> Result<Box<dyn LlmClient>, VygrError> {
    let spec = spec
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or_else(|| cfg.backend.clone())
        .or_else(|| cfg.model.as_ref().map(|_| "openai-compat".to_string()))
        .ok_or_else(|| {
            VygrError::Config(
                "no LLM backend configured: pass --llm <spec> or set [llm] backend/model in ~/.config/voyager/config.toml \
                 (specs: pi | claude | codex | ollama:<model> | openai-compat | <models.dev provider>:<model>)"
                    .to_string(),
            )
        })?;

    // Split "backend[:model]" once; model ids may themselves contain
    // colons (e.g. "ollama:qwen3:8b", "openrouter:x:y" stays intact).
    let (backend, spec_model) = match spec.as_str().split_once(':') {
        Some((b, m)) => (b.to_string(), Some(m.to_string())),
        None => (spec.clone(), None),
    };

    match backend.as_str() {
        // Structured channels where the harness offers one: claude's JSON
        // envelope (carrying real cost), codex's last-message file; pi
        // answers on plain stdout.
        "pi" => Ok(Box::new(harness::HarnessLlm::new(
            "pi",
            &["--print"],
            harness::PromptChannel::Stdin(harness::STDIN_TASK),
            harness::OutputMode::Plain,
        ))),
        "claude" => Ok(Box::new(harness::HarnessLlm::new(
            "claude",
            &["-p", "--output-format", "json"],
            harness::PromptChannel::Stdin(harness::STDIN_TASK),
            harness::OutputMode::ClaudeJson,
        ))),
        "codex" => Ok(Box::new(harness::HarnessLlm::new(
            "codex",
            &["exec"],
            harness::PromptChannel::Stdin("-"),
            harness::OutputMode::CodexLastMessage,
        ))),
        "ollama" => {
            let model = spec_model.or_else(|| cfg.model.clone()).ok_or_else(|| {
                VygrError::Config(
                    "ollama needs a model, e.g. --llm ollama:qwen3:8b or [llm] model".into(),
                )
            })?;
            let base = cfg
                .base_url
                .clone()
                .unwrap_or_else(|| "http://localhost:11434".to_string());
            Ok(Box::new(ollama::OllamaClient::new(
                http.clone(),
                base,
                model,
            )))
        }
        "openai-compat" | "openai-compatible" | "custom" => {
            let base = cfg
                .base_url
                .clone()
                .ok_or_else(|| VygrError::Config("openai-compat needs [llm] base_url".into()))?;
            let model = spec_model
                .or_else(|| cfg.model.clone())
                .ok_or_else(|| VygrError::Config("openai-compat needs [llm] model".into()))?;
            let key = cfg
                .api_key_env
                .as_deref()
                .and_then(|name| std::env::var(name).ok())
                .filter(|k| !k.trim().is_empty());
            Ok(Box::new(openai_compat::OpenAiCompatible::new(
                http.clone(),
                base,
                key,
                model,
                None,
            )))
        }
        pid => {
            // Treat as a models.dev provider id.
            let catalog = models_dev::load(http).await?;
            let info = catalog.provider(pid).ok_or_else(|| {
                VygrError::Config(format!(
                    "unknown LLM provider '{pid}' (see `vygr models` for the models.dev catalog)"
                ))
            })?;
            let base = info.api.clone().ok_or_else(|| {
                VygrError::Config(format!(
                    "provider '{pid}' has no OpenAI-compatible endpoint in models.dev; route it via openrouter instead"
                ))
            })?;
            let env_name = cfg
                .api_key_env
                .clone()
                .or_else(|| info.env.first().cloned())
                .ok_or_else(|| {
                    VygrError::Config(format!("provider '{pid}' documents no API key env var"))
                })?;
            let key = std::env::var(&env_name)
                .ok()
                .filter(|k| !k.trim().is_empty())
                .ok_or_else(|| VygrError::Auth(format!("set {env_name} to use {pid}")))?;
            let model = spec_model
                .or_else(|| cfg.model.clone())
                .or_else(|| catalog.tool_call_default_model(pid))
                .ok_or_else(|| {
                    VygrError::Config(format!("provider '{pid}' needs an explicit model"))
                })?;
            let cost = catalog.cost(pid, &model);
            Ok(Box::new(openai_compat::OpenAiCompatible::new(
                http.clone(),
                base,
                Some(key),
                model,
                cost,
            )))
        }
    }
}
