//! Generic OpenAI-compatible chat-completions client (also used for
//! Ollama's `/v1` endpoint and any models.dev provider exposing `api`).

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use vygr_core::VygrError;

use crate::models_dev::CostPerMtok;
use crate::{CompletionRequest, CompletionResponse, LlmClient, Role, TokenUsage};

pub struct OpenAiCompatible {
    http: reqwest::Client,
    base_url: String,
    api_key: Option<String>,
    model: String,
    cost: Option<CostPerMtok>,
}

impl OpenAiCompatible {
    pub fn new(
        http: reqwest::Client,
        base_url: String,
        api_key: Option<String>,
        model: String,
        cost: Option<CostPerMtok>,
    ) -> Self {
        Self {
            http,
            base_url,
            api_key,
            model,
            cost,
        }
    }
}

#[derive(Serialize)]
struct ReqBody<'a> {
    model: &'a str,
    messages: Vec<ReqMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f64>,
}

#[derive(Serialize)]
struct ReqMessage {
    role: &'static str,
    content: String,
}

#[derive(Deserialize)]
struct RespBody {
    #[serde(default)]
    choices: Vec<RespChoice>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    usage: Option<RespUsage>,
}

#[derive(Deserialize)]
struct RespChoice {
    message: RespMessage,
}

#[derive(Deserialize)]
struct RespMessage {
    #[serde(default)]
    content: Option<String>,
}

#[derive(Deserialize)]
struct RespUsage {
    #[serde(default)]
    prompt_tokens: Option<u64>,
    #[serde(default)]
    completion_tokens: Option<u64>,
}

fn role_str(role: Role) -> &'static str {
    match role {
        Role::System => "system",
        Role::User => "user",
        Role::Assistant => "assistant",
    }
}

#[async_trait]
impl LlmClient for OpenAiCompatible {
    async fn complete(&self, req: &CompletionRequest) -> Result<CompletionResponse, VygrError> {
        let body = ReqBody {
            model: &self.model,
            messages: req
                .messages
                .iter()
                .map(|m| ReqMessage {
                    role: role_str(m.role),
                    content: m.content.clone(),
                })
                .collect(),
            max_tokens: req.max_tokens,
            temperature: req.temperature,
        };

        let mut request = self
            .http
            .post(format!(
                "{}/chat/completions",
                self.base_url.trim_end_matches('/')
            ))
            .json(&body);
        if let Some(key) = &self.api_key {
            request = request.bearer_auth(key);
        }
        let resp = request
            .send()
            .await
            .map_err(|e| VygrError::Network(format!("llm {}: {e}", self.model)))?;
        let status = resp.status();
        if status.as_u16() == 401 || status.as_u16() == 403 {
            return Err(VygrError::Auth(format!(
                "llm {} rejected credentials (HTTP {status})",
                self.model
            )));
        }
        if !status.is_success() {
            let detail = resp
                .text()
                .await
                .unwrap_or_default()
                .chars()
                .take(300)
                .collect::<String>();
            return Err(VygrError::provider_status(
                format!("llm:{}", self.model),
                format!("HTTP {status} {detail}"),
                status.as_u16(),
            ));
        }
        let parsed: RespBody = resp
            .json()
            .await
            .map_err(|e| VygrError::Parse(format!("llm response: {e}")))?;

        let content = parsed
            .choices
            .into_iter()
            .next()
            .and_then(|c| c.message.content)
            .unwrap_or_default();
        let usage = parsed.usage.map(|u| TokenUsage {
            input_tokens: u.prompt_tokens.unwrap_or(0),
            output_tokens: u.completion_tokens.unwrap_or(0),
        });
        let cost_usd = usage.as_ref().zip(self.cost.as_ref()).map(|(u, c)| {
            (u.input_tokens as f64 * c.input + u.output_tokens as f64 * c.output) / 1_000_000.0
        });

        Ok(CompletionResponse {
            content,
            model: parsed.model.unwrap_or_else(|| self.model.clone()),
            usage,
            cost_usd,
        })
    }

    fn describe(&self) -> String {
        format!("openai-compatible {} ({})", self.model, self.base_url)
    }
}
