//! Harness shell-out backend (ADR-0004): reuse the LLM already configured
//! in an agent harness (`pi --print`, `claude -p`, `codex exec`) instead of
//! managing keys and models ourselves. No MCP pass-through needed: we
//! simply run the harness in print mode and capture stdout.
//!
//! Prompts travel through the child's stdin (research prompts regularly
//! exceed Linux's 128 KiB per-argument limit). Output modes use each
//! harness's structured channel when one exists: `claude -p
//! --output-format json` returns an envelope carrying the answer plus
//! `total_cost_usd`, and `codex exec --output-last-message <file>` writes
//! the final answer to a file. Plain stdout remains the fallback.

use async_trait::async_trait;
use serde::Deserialize;
use std::process::Stdio;
use tokio::io::AsyncWriteExt;
use vygr_core::VygrError;

use crate::{CompletionRequest, CompletionResponse, LlmClient, Role};

/// Instruction placed in argv when the real prompt travels via stdin.
pub const STDIN_TASK: &str = "Complete the task provided on standard input.";

/// Linux's `MAX_ARG_STRLEN` is 128 KiB; stay safely below it.
const MAX_ARG_PROMPT: usize = 120_000;

/// How the prompt reaches the harness process: piped through stdin with
/// `arg` as the positional argument (`-` makes codex read the prompt from
/// stdin), or as the final argv element (size-guarded).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptChannel {
    Stdin(&'static str),
    Arg,
}

/// How the answer comes back from the harness.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputMode {
    /// Raw stdout text (pi and the generic fallback).
    Plain,
    /// `claude -p --output-format json`: parse the envelope's `result`
    /// and capture `total_cost_usd`.
    ClaudeJson,
    /// `codex exec --output-last-message <file>`: read the answer file.
    CodexLastMessage,
}

pub struct HarnessLlm {
    program: String,
    args: Vec<String>,
    channel: PromptChannel,
    output: OutputMode,
}

impl HarnessLlm {
    pub fn new(program: &str, args: &[&str], channel: PromptChannel, output: OutputMode) -> Self {
        Self {
            program: program.to_string(),
            args: args.iter().map(|s| s.to_string()).collect(),
            channel,
            output,
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

/// `claude -p --output-format json` envelope (only the fields we use).
#[derive(Debug, Deserialize)]
struct ClaudeEnvelope {
    #[serde(default)]
    result: Option<String>,
    #[serde(default)]
    total_cost_usd: Option<f64>,
}

/// Parse a claude envelope; `None` when the stdout is not the envelope
/// (older versions, unexpected output) so the caller can fall back.
fn parse_claude_envelope(stdout: &str) -> Option<(String, Option<f64>)> {
    let envelope: ClaudeEnvelope = serde_json::from_str(stdout.trim()).ok()?;
    let result = envelope.result?;
    Some((result, envelope.total_cost_usd))
}

/// Unique temp path for a codex `--output-last-message` file.
fn codex_last_message_path() -> std::path::PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!("vygr-codex-{}-{nanos}.txt", std::process::id()))
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
        let last_message_file =
            (self.output == OutputMode::CodexLastMessage).then(codex_last_message_path);
        if let Some(path) = &last_message_file {
            command
                .arg("--output-last-message")
                .arg(path)
                .arg("-")
                .stdin(Stdio::piped());
        } else {
            match self.channel {
                PromptChannel::Stdin(arg) => {
                    command.arg(arg).stdin(Stdio::piped());
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
        }

        let mut child = command.spawn().map_err(|e| {
            VygrError::Config(format!(
                "failed to launch harness '{}': {e} (is it installed and on PATH?)",
                self.program
            ))
        })?;

        if child.stdin.is_some() {
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

        let stdout = String::from_utf8_lossy(&output.stdout);
        let (content, cost_usd) = match self.output {
            OutputMode::ClaudeJson => match parse_claude_envelope(&stdout) {
                Some((result, cost)) => (result, cost),
                // Not an envelope: fall back to the raw stdout text.
                None => (stdout.trim().to_string(), None),
            },
            OutputMode::CodexLastMessage => {
                let from_file = last_message_file
                    .as_deref()
                    .and_then(|p| std::fs::read_to_string(p).ok())
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty());
                if let Some(path) = &last_message_file {
                    let _ = std::fs::remove_file(path);
                }
                (from_file.unwrap_or_else(|| stdout.trim().to_string()), None)
            }
            OutputMode::Plain => (stdout.trim().to_string(), None),
        };
        Ok(CompletionResponse {
            content,
            model: format!("harness:{}", self.program),
            usage: None,
            cost_usd,
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
        // The wrapper shell consumes the instruction argument; cat echoes
        // the prompt that arrives on stdin.
        let llm = HarnessLlm::new(
            "sh",
            &["-c", "cat"],
            PromptChannel::Stdin(STDIN_TASK),
            OutputMode::Plain,
        );
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
    async fn codex_mode_reads_the_last_message_file() {
        // Simulate codex: write the prompt into the file named by the flag.
        // The "sim" argv is a placeholder $0 so the flag lands on $1.
        let script = r#"out=""; while [ $# -gt 0 ]; do case "$1" in --output-last-message) out="$2"; shift 2;; *) shift;; esac; done; cat > "$out""#;
        let llm = HarnessLlm::new(
            "sh",
            &["-c", script, "sim"],
            PromptChannel::Stdin("-"),
            OutputMode::CodexLastMessage,
        );
        let resp = llm
            .complete(&CompletionRequest {
                messages: vec![ChatMessage::user("final answer text")],
                max_tokens: None,
                temperature: None,
                json_object: false,
            })
            .await
            .unwrap();
        assert_eq!(resp.content, "final answer text");
    }

    #[test]
    fn claude_envelope_parsing() {
        let raw = r#"{"type":"result","subtype":"success","result":"the answer","total_cost_usd":0.0123,"session_id":"s1"}"#;
        let (content, cost) = parse_claude_envelope(raw).unwrap();
        assert_eq!(content, "the answer");
        assert_eq!(cost, Some(0.0123));
        // Non-envelope stdout falls back to None.
        assert!(parse_claude_envelope("just plain text").is_none());
    }

    #[tokio::test]
    async fn arg_channel_rejects_oversized_prompts() {
        let llm = HarnessLlm::new("cat", &[], PromptChannel::Arg, OutputMode::Plain);
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
