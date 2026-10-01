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
    #[serde(skip_serializing_if = "Option::is_none")]
    response_format: Option<RespFormat>,
}

#[derive(Serialize)]
struct RespFormat {
    #[serde(rename = "type")]
    kind: &'static str,
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
    /// Reasoning models (qwen3.5, deepseek-r1, …) put their answer here
    /// when `content` comes back empty.
    #[serde(default)]
    reasoning: Option<String>,
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
            response_format: req.json_object.then_some(RespFormat {
                kind: "json_object",
            }),
        };

        let mut request = self
            .http
            .post(format!(
                "{}/chat/completions",
                self.base_url.trim_end_matches('/')
            ))
            // Local models may cold-load and reason for minutes; the shared
            // 30s client timeout does not apply to chat completions.
            .timeout(std::time::Duration::from_secs(600))
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

        let content = extract_content(parsed.choices.into_iter().next().map(|c| c.message));
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

/// Answer text extraction: `content` is authoritative; when it comes back
/// empty (reasoning models) fall back to the `reasoning` field. Inline
/// `<think>…</think>` blocks are stripped in both cases.
fn extract_content(message: Option<RespMessage>) -> String {
    let Some(m) = message else {
        return String::new();
    };
    let content = strip_think(&m.content.unwrap_or_default());
    if !content.trim().is_empty() {
        return content;
    }
    strip_think(&m.reasoning.unwrap_or_default())
}

fn strip_think(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("<think>") {
        out.push_str(&rest[..start]);
        let after = &rest[start + "<think>".len()..];
        match after.find("</think>") {
            Some(end) => rest = &after[end + "</think>".len()..],
            // Unclosed think block: everything after the tag is reasoning.
            None => {
                rest = "";
                break;
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_think_blocks_and_handles_unclosed() {
        assert_eq!(strip_think("a<think>hidden</think>b"), "ab");
        assert_eq!(strip_think("<think>only reasoning"), "");
        assert_eq!(strip_think("plain"), "plain");
    }

    #[test]
    fn reasoning_field_fallback() {
        let m = RespMessage {
            content: Some("  ".to_string()),
            reasoning: Some("final text".to_string()),
        };
        assert_eq!(extract_content(Some(m)), "final text");
        let m = RespMessage {
            content: Some("<think>x</think>answer".to_string()),
            reasoning: None,
        };
        assert_eq!(extract_content(Some(m)), "answer");
    }
}
