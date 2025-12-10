//! Task tool - launches subagents to handle complex tasks
//!
//! The Task tool spawns a child agent to handle a specific subtask.
//! It looks up the agent config from AgentRegistry, creates a BaseAgent,
//! and runs it to completion.

use crate::agent::{AgentConfig, AgentRegistry, BaseAgent};
use crate::events::AgentEvent;
use crate::provider::ProviderClient;
use crate::tool::{Tool, ToolContext, ToolDefinition, ToolRegistry, ToolResult};
use async_openai::types::{ChatCompletionRequestMessage, ChatCompletionRequestUserMessageArgs};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;
use tokio::sync::mpsc;

/// Task tool description template
const DESCRIPTION_TEMPLATE: &str = r#"Launch a new agent to handle complex, multi-step tasks autonomously.

Available agent types:
{agents}

Usage notes:
- Launch multiple agents concurrently for parallel tasks
- The agent returns a single message with its results
- Each agent invocation is stateless
- Clearly specify whether you want code written or just research

When NOT to use:
- For reading specific files (use read_file)
- For searching within 2-3 files (use grep)
- For finding files by name (use find_path)"#;

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct Args {
    /// Short description of the task (3-5 words)
    description: String,
    /// The full task prompt for the agent
    prompt: String,
    /// Which subagent to use (e.g., "general")
    subagent_type: String,
    /// Max iterations (optional, default 50)
    #[serde(default)]
    max_iterations: Option<usize>,
}

/// Task tool for spawning subagents
///
/// Requires:
/// - AgentRegistry for looking up subagent configs
/// - ProviderClient for LLM calls
/// - ToolRegistry for the child agent's tools
pub struct TaskTool {
    description: String,
    registry: AgentRegistry,
    provider: Arc<ProviderClient>,
    tool_registry: ToolRegistry,
}

impl TaskTool {
    /// Create a new Task tool
    pub fn new(
        registry: AgentRegistry,
        provider: Arc<ProviderClient>,
        tool_registry: ToolRegistry,
    ) -> Self {
        // Build description with placeholder - will be filled lazily
        let description = DESCRIPTION_TEMPLATE.replace(
            "{agents}",
            "- general: General-purpose agent for research and multi-step tasks\n\
             - executor: Implementation agent for code tasks",
        );

        Self {
            description,
            registry,
            provider,
            tool_registry,
        }
    }

    /// Create with dynamic agent list from registry
    pub async fn new_with_registry(
        registry: AgentRegistry,
        provider: Arc<ProviderClient>,
        tool_registry: ToolRegistry,
    ) -> Self {
        let agent_desc = registry.subagent_descriptions().await;
        let description = DESCRIPTION_TEMPLATE.replace("{agents}", &agent_desc);

        Self {
            description,
            registry,
            provider,
            tool_registry,
        }
    }

    /// Run a subagent with the given config and prompt
    async fn run_subagent(
        &self,
        config: AgentConfig,
        prompt: &str,
        ctx: &ToolContext,
    ) -> Result<String, String> {
        // Create the base agent
        let agent = BaseAgent::new(config.clone(), self.provider.clone(), ctx.working_dir.clone());

        // Build initial message
        let user_msg = ChatCompletionRequestUserMessageArgs::default()
            .content(prompt)
            .build()
            .map_err(|e| format!("Failed to build user message: {}", e))?;

        let mut messages: Vec<ChatCompletionRequestMessage> =
            vec![ChatCompletionRequestMessage::User(user_msg)];

        // Get tools (filtered by agent config)
        let tools: Vec<_> = self
            .tool_registry
            .to_openai_tools()
            .into_iter()
            .filter(|t| config.is_tool_enabled(&t.function.name))
            .collect();

        // Create event channel (we'll collect but not emit to parent)
        let (event_tx, mut event_rx) = mpsc::unbounded_channel::<AgentEvent>();

        // Spawn a task to drain events (we just log them for now)
        let drain_handle = tokio::spawn(async move {
            while let Some(_event) = event_rx.recv().await {
                // Could log events or forward to telemetry
                // For now, just drain them
            }
        });

        // Run the agent turn
        let result = agent
            .execute_turn(
                &mut messages,
                &tools,
                &self.tool_registry,
                &event_tx,
                ctx.cancellation.clone(),
            )
            .await;

        // Close event channel and wait for drain
        drop(event_tx);
        let _ = drain_handle.await;

        match result {
            Ok(turn_result) => {
                // Return the agent's response
                if let Some(text) = turn_result.text {
                    Ok(text)
                } else {
                    // No text response - summarize what happened
                    let tool_summary: Vec<String> = turn_result
                        .tool_calls
                        .iter()
                        .map(|tc| format!("- {}: {}", tc.name, if tc.is_error { "error" } else { "ok" }))
                        .collect();

                    if tool_summary.is_empty() {
                        Ok("Agent completed without producing output.".to_string())
                    } else {
                        Ok(format!(
                            "Agent completed. Tool calls:\n{}",
                            tool_summary.join("\n")
                        ))
                    }
                }
            }
            Err(e) => Err(e),
        }
    }
}

#[async_trait]
impl Tool for TaskTool {
    fn name(&self) -> &str {
        "task"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "task".to_string(),
            description: self.description.clone(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "description": {
                        "type": "string",
                        "description": "Short (3-5 word) description of the task"
                    },
                    "prompt": {
                        "type": "string",
                        "description": "The detailed task for the agent to perform"
                    },
                    "subagent_type": {
                        "type": "string",
                        "description": "Agent type to use (e.g., 'general')"
                    },
                    "max_iterations": {
                        "type": "integer",
                        "description": "Optional max iterations (default: 50)"
                    }
                },
                "required": ["description", "prompt", "subagent_type"]
            }),
        }
    }

    async fn execute(&self, args_value: serde_json::Value, ctx: &ToolContext) -> ToolResult {
        if ctx.is_cancelled() {
            return ToolResult::error("Cancelled");
        }

        let args: Args = match serde_json::from_value(args_value) {
            Ok(a) => a,
            Err(e) => return ToolResult::error(format!("Invalid arguments: {}", e)),
        };

        // Look up the agent config
        let agent_config = match self.registry.get(&args.subagent_type).await {
            Some(config) => config,
            None => {
                return ToolResult::error(format!(
                    "Unknown agent type: '{}'. Available subagents: {}",
                    args.subagent_type,
                    self.registry
                        .get_subagents()
                        .await
                        .iter()
                        .map(|a| a.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ))
            }
        };

        // Verify it's a subagent
        if !agent_config.is_subagent() {
            return ToolResult::error(format!(
                "Agent '{}' cannot be used as a subagent (mode: {:?})",
                args.subagent_type, agent_config.mode
            ));
        }

        // Override max_iterations if provided
        let mut config = agent_config;
        if let Some(max_iter) = args.max_iterations {
            config.max_iterations = Some(max_iter);
        }

        // Run the subagent
        match self.run_subagent(config, &args.prompt, ctx).await {
            Ok(output) => ToolResult::success(output),
            Err(e) => ToolResult::error(format!("Subagent error: {}", e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_description_template() {
        let desc = DESCRIPTION_TEMPLATE.replace("{agents}", "- test: Test agent");
        assert!(desc.contains("- test: Test agent"));
        assert!(desc.contains("Launch a new agent"));
    }
}
