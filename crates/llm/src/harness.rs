//! Harness shell-out backend (ADR-0004): reuse the LLM already configured
//! in an agent harness (`pi --print`, `claude -p`, `codex exec`) instead of
//! managing keys and models ourselves. No MCP pass-through needed — we
//! simply run the harness in print mode and capture stdout.
//!
//! Prompts are piped through the child's stdin (M2.4): research prompts
//! regularly exceed Linux's 128 KiB `MAX_ARG_STRLEN` per-argument limit,
//! so argv only carries a short instruction. `pi` and `claude` document
//! piped-stdin support; `codex exec` keeps the legacy argv channel with a
//! size guard.

use async_trait::async_trait;
use std::process::Stdio;
use tokio::io::AsyncWriteExt;
use vygr_core::VygrError;

use crate::{CompletionRequest, CompletionResponse, LlmClient, Role};

/// Instruction placed in argv when the real prompt travels via stdin.
const STDIN_TASK: &str = "Complete the task provided on standard input.";

/// Linux's `MAX_ARG_STRLEN` is 128 KiB; stay safely below it.
const MAX_ARG_PROMPT: usize = 120_000;

/// How the prompt reaches the harness process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptChannel {
    /// Full prompt piped to stdin; argv carries [`STDIN_TASK`].
    Stdin,
    /// Prompt passed as the final argv element (size-guarded).
    Arg,
}

pub struct HarnessLlm {
    program: String,
    args: Vec<String>,
    channel: PromptChannel,
}

impl HarnessLlm {
    pub fn new(program: &str, args: &[&str], channel: PromptChannel) -> Self {
        Self {
            program: program.to_string(),
            args: args.iter().map(|s| s.to_string()).collect(),
            channel,
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

        let mut command = tokio::process::Command::new(&self.program);
        command
            .args(&self.args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        match self.channel {
            PromptChannel::Stdin => {
                command.arg(STDIN_TASK).stdin(Stdio::piped());
            }
            PromptChannel::Arg => {
                if prompt.len() > MAX_ARG_PROMPT {
                    return Err(VygrError::Config(format!(
                        "prompt of {} bytes exceeds the argv limit of the '{}' backend; \
use --llm pi, --llm claude or an API backend instead",
                        prompt.len(),
                        self.program
                    )));
                }
                command.arg(&prompt).stdin(Stdio::null());
            }
        }

        let mut child = command.spawn().map_err(|e| {
            VygrError::Config(format!(
                "failed to launch harness '{}': {e} (is it installed and on PATH?)",
                self.program
            ))
        })?;

        if self.channel == PromptChannel::Stdin {
            if let Some(mut stdin) = child.stdin.take() {
                stdin
                    .write_all(prompt.as_bytes())
                    .await
                    .map_err(|e| VygrError::Network(format!("writing to {}: {e}", self.program)))?;
                // Dropping closes the pipe so the harness sees EOF.
                drop(stdin);
            }
        }

        let output = child
            .wait_with_output()
            .await
            .map_err(|e| VygrError::Network(format!("waiting for {}: {e}", self.program)))?;
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

    #[tokio::test]
    async fn stdin_channel_delivers_full_prompt() {
        // `cat` with no file arguments reads stdin; the instruction argv
        // element that HarnessLlm adds is consumed by the shell wrapper.
        let llm = HarnessLlm::new("sh", &["-c", "cat"], PromptChannel::Stdin);
        let resp = llm
            .complete(&CompletionRequest {
                messages: vec![
                    ChatMessage::system("sys"),
                    ChatMessage::user("the actual question"),
                ],
                max_tokens: None,
                temperature: None,
                json_object: false,
            })
            .await
            .unwrap();
        assert_eq!(
            resp.content,
            "System instructions:\nsys\n\nthe actual question"
        );
    }

    #[tokio::test]
    async fn arg_channel_rejects_oversized_prompts() {
        let llm = HarnessLlm::new("cat", &[], PromptChannel::Arg);
        let err = llm
            .complete(&CompletionRequest {
                messages: vec![ChatMessage::user("x".repeat(130_000))],
                max_tokens: None,
                temperature: None,
                json_object: false,
            })
            .await
            .unwrap_err();
        assert!(matches!(err, VygrError::Config(_)));
    }
}
