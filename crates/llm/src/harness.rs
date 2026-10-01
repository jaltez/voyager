//! Harness shell-out backend (ADR-0004): reuse the LLM already configured
//! in an agent harness (`pi --print`, `claude -p`, `codex exec`) instead of
//! managing keys and models ourselves. No MCP pass-through needed — we
//! simply run the harness in print mode and capture stdout.

use async_trait::async_trait;
use std::process::Stdio;
use vygr_core::VygrError;

use crate::{CompletionRequest, CompletionResponse, LlmClient, Role};

pub struct HarnessLlm {
    program: String,
    args: Vec<String>,
}

impl HarnessLlm {
    pub fn new(program: &str, args: &[&str]) -> Self {
        Self {
            program: program.to_string(),
            args: args.iter().map(|s| s.to_string()).collect(),
        }
    }
}

fn render_prompt(messages: &[crate::ChatMessage]) -> String {
    messages
        .iter()
        .map(|m| match m.role {
            Role::System => format!("System instructions:\n{}", m.content),
            _ => m.content.clone(),
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

#[async_trait]
impl LlmClient for HarnessLlm {
    async fn complete(&self, req: &CompletionRequest) -> Result<CompletionResponse, VygrError> {
        let prompt = render_prompt(&req.messages);
        let output = tokio::process::Command::new(&self.program)
            .args(&self.args)
            .arg(&prompt)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .map_err(|e| {
                VygrError::Config(format!(
                    "failed to launch harness '{}': {e} (is it installed and on PATH?)",
                    self.program
                ))
            })?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let excerpt: String = stderr.chars().take(300).collect();
            return Err(VygrError::provider(
                format!("harness:{}", self.program),
                format!("exited with {}: {excerpt}", output.status),
            ));
        }
        let content = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Ok(CompletionResponse {
            content,
            model: format!("harness:{}", self.program),
            usage: None,
            cost_usd: None,
        })
    }

    fn describe(&self) -> String {
        format!(
            "harness shell-out: {} {}",
            self.program,
            self.args.join(" ")
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ChatMessage;

    #[test]
    fn renders_system_then_user() {
        let prompt = render_prompt(&[ChatMessage::system("be terse"), ChatMessage::user("hello")]);
        assert_eq!(prompt, "System instructions:\nbe terse\n\nhello");
    }
}
