//! Task complete tool - signals task completion with LLM evaluation
//!
//! When called, this tool evaluates whether the task is actually complete
//! by making an LLM call with the conversation history. This prevents
//! premature completion claims.

use crate::tool::{Tool, ToolContext, ToolDefinition, ToolResult};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Debug, Deserialize)]
struct Args {
    summary: String,
}

/// Structured response from the evaluator LLM
#[derive(Debug, Deserialize, Serialize)]
struct EvaluatorResponse {
    /// Whether the task is actually complete
    complete: bool,
    /// Reason for the decision (feedback if not complete)
    reason: String,
}

/// JSON schema for structured output
fn evaluator_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "complete": {
                "type": "boolean",
                "description": "Whether the task is actually complete"
            },
            "reason": {
                "type": "string",
                "description": "Brief explanation of the decision"
            }
        },
        "required": ["complete", "reason"],
        "additionalProperties": false
    })
}

pub struct TaskCompleteTool;

impl TaskCompleteTool {
    pub fn new() -> Self {
        Self
    }

    /// Evaluate completion using LLM with the actual conversation history
    async fn evaluate_completion(
        &self,
        ctx: &ToolContext,
    ) -> Result<EvaluatorResponse, String> {
        let provider = ctx
            .provider
            .as_ref()
            .ok_or("No provider available for evaluation")?;

        let messages = ctx
            .messages
            .as_ref()
            .ok_or("No messages available for evaluation")?;

        // Use tool_choice to force structured output (works with thinking models)
        provider
            .chat_tool_structured(
                messages.clone(),
                "evaluate_completion",
                "Evaluate if the task is complete based on the conversation",
                evaluator_schema(),
                None,
            )
            .await
    }
}

impl Default for TaskCompleteTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for TaskCompleteTool {
    fn name(&self) -> &str {
        "task_complete"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "task_complete".to_string(),
            description: "Call when the task is complete. Provide a summary of what was done. \
                An evaluator will verify the work before accepting completion."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "summary": {
                        "type": "string",
                        "description": "Summary of what was accomplished"
                    }
                },
                "required": ["summary"]
            }),
        }
    }

    fn humanize(&self, args: &Value, result: &ToolResult) -> Option<String> {
        let summary = args.get("summary").and_then(|v| v.as_str()).unwrap_or("");
        if result.is_error {
            Some(format!("✗ rejected: {}", result.output))
        } else {
            Some(format!("✓ completed: {}", summary))
        }
    }

    async fn execute(&self, args: serde_json::Value, ctx: &ToolContext) -> ToolResult {
        let args: Args = match serde_json::from_value(args) {
            Ok(a) => a,
            Err(e) => return ToolResult::error(format!("Invalid arguments: {}", e)),
        };

        // If we have messages and provider, run the evaluator
        if ctx.messages.is_some() && ctx.provider.is_some() {
            match self.evaluate_completion(ctx).await {
                Ok(evaluation) => {
                    if evaluation.complete {
                        ToolResult::success(format!(
                            "{}\n\n[Verified: {}]",
                            args.summary, evaluation.reason
                        ))
                    } else {
                        ToolResult::error(format!(
                            "Task not complete: {}\n\nPlease address the feedback and try again.",
                            evaluation.reason
                        ))
                    }
                }
                Err(e) => {
                    // Evaluation failed - log but allow completion (fail open)
                    tracing::warn!("Task completion evaluation failed: {}. Allowing completion.", e);
                    ToolResult::success(args.summary)
                }
            }
        } else {
            // No context available - fall back to simple completion (subagents)
            ToolResult::success(args.summary)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::ToolContext;
    use async_openai::types::ChatCompletionRequestMessage;
    use std::path::PathBuf;
    use tokio_util::sync::CancellationToken;

    #[tokio::test]
    async fn test_task_complete_without_context() {
        let tool = TaskCompleteTool::new();
        let ctx = ToolContext::new(PathBuf::from("/tmp"), CancellationToken::new());

        let result = tool.execute(json!({"summary": "Did the thing"}), &ctx).await;

        assert!(!result.is_error);
        assert_eq!(result.output, "Did the thing");
    }

    #[tokio::test]
    async fn test_task_complete_with_context_but_no_provider() {
        use async_openai::types::ChatCompletionRequestUserMessageArgs;

        let tool = TaskCompleteTool::new();
        let messages = vec![
            ChatCompletionRequestMessage::User(
                ChatCompletionRequestUserMessageArgs::default()
                    .content("Do something")
                    .build()
                    .unwrap()
            )
        ];

        // Context with messages but no provider - should fall back
        let mut ctx = ToolContext::new(PathBuf::from("/tmp"), CancellationToken::new());
        ctx.messages = Some(messages);

        let result = tool.execute(json!({"summary": "Did something"}), &ctx).await;

        assert!(!result.is_error);
        assert_eq!(result.output, "Did something");
    }

    #[tokio::test]
    async fn test_evaluator_schema() {
        let schema = evaluator_schema();
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["complete"]["type"] == "boolean");
        assert!(schema["properties"]["reason"]["type"] == "string");
    }

    #[tokio::test]
    async fn test_task_complete_with_full_context_calls_evaluator() {
        use async_openai::types::ChatCompletionRequestUserMessageArgs;
        use crate::provider::{ProviderClient, ProviderConfig};
        use std::sync::Arc;

        let tool = TaskCompleteTool::new();
        let messages = vec![
            ChatCompletionRequestMessage::User(
                ChatCompletionRequestUserMessageArgs::default()
                    .content("Create a file")
                    .build()
                    .unwrap()
            )
        ];

        // Create a provider pointing to a non-existent server
        // This will fail the evaluation, testing the fail-open behavior
        let config = ProviderConfig {
            name: "test".to_string(),
            base_url: "http://localhost:99999".to_string(),
            default_model: "test-model".to_string(),
            api_key_env: "TEST_API_KEY".to_string(),
        };

        // Set env var for the test
        std::env::set_var("TEST_API_KEY", "test-key");

        let provider = Arc::new(ProviderClient::new(config).unwrap());

        let ctx = ToolContext::with_llm(
            PathBuf::from("/tmp"),
            CancellationToken::new(),
            messages,
            provider,
        );

        let result = tool.execute(json!({"summary": "Created the file"}), &ctx).await;

        // Should fail open - evaluation fails but completion is allowed
        assert!(!result.is_error, "Expected success (fail open), got error: {}", result.output);
        assert_eq!(result.output, "Created the file");
    }

    /// Integration test that actually calls the LLM
    /// Run with: cargo test --features integration task_complete_integration -- --ignored
    /// Or: cargo test task_complete_integration -- --ignored
    #[tokio::test]
    #[ignore] // Only run manually - requires LM Studio running
    async fn task_complete_integration_test() {
        use async_openai::types::{
            ChatCompletionRequestUserMessageArgs,
            ChatCompletionRequestAssistantMessageArgs,
            ChatCompletionRequestSystemMessageArgs,
        };
        use crate::provider::{ProviderClient, ProviderConfig};
        use std::sync::Arc;

        // Use LM Studio
        let config = ProviderConfig {
            name: "lm-studio".to_string(),
            base_url: "http://coast-after-3:1234/v1".to_string(),
            default_model: "qwen3-30b-a3b".to_string(),
            api_key_env: "LM_STUDIO_API_KEY".to_string(),
        };
        std::env::set_var("LM_STUDIO_API_KEY", "lm-studio");

        let provider = Arc::new(ProviderClient::new(config).unwrap());

        // Simulate a completed conversation
        let messages = vec![
            ChatCompletionRequestMessage::System(
                ChatCompletionRequestSystemMessageArgs::default()
                    .content("You are a helpful coding assistant.")
                    .build()
                    .unwrap()
            ),
            ChatCompletionRequestMessage::User(
                ChatCompletionRequestUserMessageArgs::default()
                    .content("Create a file called test.txt with 'hello world'")
                    .build()
                    .unwrap()
            ),
            ChatCompletionRequestMessage::Assistant(
                ChatCompletionRequestAssistantMessageArgs::default()
                    .content("I'll create the file for you.")
                    .build()
                    .unwrap()
            ),
            // Simulate tool result
            ChatCompletionRequestMessage::User(
                ChatCompletionRequestUserMessageArgs::default()
                    .content("[Tool result: write_file succeeded - created test.txt with content 'hello world']")
                    .build()
                    .unwrap()
            ),
        ];

        // First test the provider directly
        let test_messages = vec![
            ChatCompletionRequestMessage::User(
                ChatCompletionRequestUserMessageArgs::default()
                    .content("Say hello")
                    .build()
                    .unwrap()
            ),
        ];

        let schema = json!({
            "type": "object",
            "properties": {
                "greeting": { "type": "string" }
            },
            "required": ["greeting"],
            "additionalProperties": false
        });

        let structured_result: Result<serde_json::Value, String> = provider
            .chat_structured(test_messages, "test", schema, None)
            .await;

        println!("Direct structured call result: {:?}", structured_result);

        assert!(structured_result.is_ok(), "Structured call failed: {:?}", structured_result);

        // Now test task_complete
        let tool = TaskCompleteTool::new();
        let ctx = ToolContext::with_llm(
            PathBuf::from("/tmp"),
            CancellationToken::new(),
            messages,
            provider,
        );

        let result = tool.execute(json!({"summary": "Created test.txt with hello world"}), &ctx).await;

        println!("Result: {:?}", result);
        println!("Output: {}", result.output);

        // Should succeed with verification
        assert!(!result.is_error, "Expected success, got error: {}", result.output);
        assert!(result.output.contains("[Verified:"), "Expected verification message, got: {}", result.output);
    }
}
