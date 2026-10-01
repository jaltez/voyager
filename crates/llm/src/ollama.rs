//! Native Ollama client (`/api/chat`). The OpenAI-compatible endpoint
//! ignores the `think` control, which reasoning models (qwen3.5…) need
//! disabled for short structured outputs, so vygr talks the native API:
//! `think: false` keeps planner/reflection JSON affordable and
//! `format: "json"` gives real JSON-constrained output for
//! `--output-schema` (M2.5).

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use vygr_core::VygrError;

use crate::{CompletionRequest, CompletionResponse, LlmClient, Role, TokenUsage};

pub struct OllamaClient {
    http: reqwest::Client,
    base_url: String,
    model: String,
}

impl OllamaClient {
    pub fn new(http: reqwest::Client, base_url: String, model: String) -> Self {
        Self {
            http,
            base_url,
            model,
        }
    }
}

#[derive(Serialize)]
struct ReqBody {
    model: String,
    messages: Vec<ReqMessage>,
    stream: bool,
    think: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    format: Option<&'static str>,
    options: Options,
}

#[derive(Serialize)]
struct Options {
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    num_predict: Option<u32>,
}

#[derive(Serialize)]
struct ReqMessage {
    role: &'static str,
    content: String,
}

#[derive(Deserialize)]
struct RespBody {
    #[serde(default)]
    message: Option<RespMessage>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    prompt_eval_count: Option<u64>,
    #[serde(default)]
    eval_count: Option<u64>,
}

#[derive(Deserialize)]
struct RespMessage {
    #[serde(default)]
    content: Option<String>,
}

fn role_str(role: Role) -> &'static str {
    match role {
        Role::System => "system",
        Role::User => "user",
        Role::Assistant => "assistant",
    }
}

/// Request body construction is pure for tests.
fn build_body(model: &str, req: &CompletionRequest) -> ReqBody {
    ReqBody {
        model: model.to_string(),
        messages: req
            .messages
            .iter()
            .map(|m| ReqMessage {
                role: role_str(m.role),
                content: m.content.clone(),
            })
            .collect(),
        stream: false,
        think: false,
        format: req.json_object.then_some("json"),
        options: Options {
            temperature: req.temperature,
            num_predict: req.max_tokens,
        },
    }
}

#[async_trait]
impl LlmClient for OllamaClient {
    async fn complete(&self, req: &CompletionRequest) -> Result<CompletionResponse, VygrError> {
        let body = build_body(&self.model, req);
        let resp = self
            .http
            .post(format!("{}/api/chat", self.base_url.trim_end_matches('/')))
            // Local models may cold-load; allow minutes per call.
            .timeout(std::time::Duration::from_secs(600))
            .json(&body)
            .send()
            .await
            .map_err(|e| VygrError::Network(format!("ollama {}: {e}", self.model)))?;
        let status = resp.status();
        if status.as_u16() == 404 {
            return Err(VygrError::Config(format!(
                "ollama model '{}' not found (HTTP 404); pull it with `ollama pull {}`",
                self.model, self.model
            )));
        }
        if !status.is_success() {
            return Err(VygrError::provider_status(
                format!("llm:ollama:{}", self.model),
                format!("HTTP {status}"),
                status.as_u16(),
            ));
        }
        let parsed: RespBody = resp
            .json()
            .await
            .map_err(|e| VygrError::Parse(format!("ollama response: {e}")))?;
        let content = parsed.message.and_then(|m| m.content).unwrap_or_default();
        let usage = match (parsed.prompt_eval_count, parsed.eval_count) {
            (Some(p), Some(c)) => Some(TokenUsage {
                input_tokens: p,
                output_tokens: c,
            }),
            _ => None,
        };
        Ok(CompletionResponse {
            content,
            model: parsed.model.unwrap_or_else(|| self.model.clone()),
            usage,
            // Local tokens are free.
            cost_usd: None,
        })
    }

    fn describe(&self) -> String {
        format!("ollama {} ({})", self.model, self.base_url)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ChatMessage;

    #[test]
    fn body_carries_think_false_and_json_format() {
        let req = CompletionRequest {
            messages: vec![ChatMessage::user("hi")],
            max_tokens: Some(500),
            temperature: Some(0.4),
            json_object: true,
        };
        let body = build_body("m", &req);
        assert!(!body.think);
        assert!(!body.stream);
        assert_eq!(body.format, Some("json"));
        assert_eq!(body.options.num_predict, Some(500));
        assert_eq!(body.messages.len(), 1);
        assert_eq!(body.messages[0].role, "user");

        let plain = build_body("m", &CompletionRequest::default());
        assert!(plain.format.is_none());
    }

    #[test]
    fn parses_native_response() {
        let raw = r#"{"model":"qwen3.5:9b","message":{"role":"assistant","content":"{\"subqueries\":[\"a\"]}"},"prompt_eval_count":120,"eval_count":45}"#;
        let parsed: RespBody = serde_json::from_str(raw).unwrap();
        assert_eq!(
            parsed.message.unwrap().content.unwrap(),
            "{\"subqueries\":[\"a\"]}"
        );
        assert_eq!(parsed.prompt_eval_count, Some(120));
        assert_eq!(parsed.eval_count, Some(45));
    }
}
