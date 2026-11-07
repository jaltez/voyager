//! Generic command shell-out backend (ADR-0019): `--llm "cmd:<template>"`
//! runs *any* command line via `sh -c`, pipes the prompt to its stdin and
//! takes stdout as the completion. This unlocks gemini-cli, aider, or any
//! local wrapper script without a dedicated backend. Cost is untracked
//! (the template knows its own pricing, vygr does not).

use async_trait::async_trait;
use vygr_core::VygrError;

use crate::{CompletionRequest, CompletionResponse, LlmClient};

pub struct CmdLlm {
    template: String,
}

impl CmdLlm {
    pub fn new(template: impl Into<String>) -> Self {
        Self {
            template: template.into(),
        }
    }
}

#[async_trait]
impl LlmClient for CmdLlm {
    async fn complete(&self, req: &CompletionRequest) -> Result<CompletionResponse, VygrError> {
        // Chat semantics flatten to one prompt: system, then turns in order.
        let prompt = req
            .messages
            .iter()
            .map(|m| m.content.as_str())
            .collect::<Vec<_>>()
            .join("\n\n");

        let mut child = tokio::process::Command::new("sh")
            .arg("-c")
            .arg(&self.template)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| VygrError::Config(format!("cmd: cannot spawn: {e}")))?;
        if let Some(mut stdin) = child.stdin.take() {
            use tokio::io::AsyncWriteExt as _;
            stdin
                .write_all(prompt.as_bytes())
                .await
                .map_err(VygrError::Io)?;
            stdin.shutdown().await.map_err(VygrError::Io)?;
        }
        let out = child.wait_with_output().await.map_err(VygrError::Io)?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            let tail: String = stderr.lines().rev().take(3).collect::<Vec<_>>().join(" | ");
            return Err(VygrError::Provider {
                provider: format!("cmd:{}", self.template),
                message: format!("exited with {}: {}", out.status, tail),
                status: None,
            });
        }
        let content = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if content.is_empty() {
            return Err(VygrError::Provider {
                provider: format!("cmd:{}", self.template),
                message: "empty stdout (the command produced no answer)".to_string(),
                status: None,
            });
        }
        Ok(CompletionResponse {
            content,
            model: format!("cmd:{}", self.template),
            usage: None,
            cost_usd: None,
        })
    }

    fn describe(&self) -> String {
        format!("cmd:{}", self.template)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ChatMessage, Role};

    fn request() -> CompletionRequest {
        CompletionRequest {
            messages: vec![
                ChatMessage {
                    role: Role::System,
                    content: "be terse".into(),
                },
                ChatMessage {
                    role: Role::User,
                    content: "say hi".into(),
                },
            ],
            max_tokens: None,
            temperature: None,
            json_object: false,
        }
    }

    #[tokio::test]
    async fn pipes_prompt_through_stdin_and_reads_stdout() {
        let llm = CmdLlm::new("cat; printf ' [ok]'");
        let resp = llm.complete(&request()).await.unwrap();
        assert!(resp.content.starts_with("be terse\n\nsay hi"));
        assert!(resp.content.ends_with("[ok]"));
        assert_eq!(resp.model, "cmd:cat; printf ' [ok]'");
        assert_eq!(llm.describe(), "cmd:cat; printf ' [ok]'");
        assert_eq!(resp.cost_usd, None);
    }

    #[tokio::test]
    async fn failing_commands_surface_stderr() {
        let llm = CmdLlm::new("echo boom >&2; exit 3");
        let err = llm.complete(&request()).await.unwrap_err();
        assert!(err.to_string().contains("boom"), "{err}");
    }

    #[tokio::test]
    async fn empty_stdout_is_an_error_not_a_silent_answer() {
        let llm = CmdLlm::new("true");
        assert!(llm.complete(&request()).await.is_err());
    }
}
