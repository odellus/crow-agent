//! Thinking tool - A scratchpad for the agent to reason through problems

use rig::completion::ToolDefinition;
use rig::tool::Tool;
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Debug, thiserror::Error)]
#[error("Thinking tool error: {0}")]
pub struct ThinkingError(String);

#[derive(Debug, Deserialize)]
pub struct ThinkingArgs {
    /// The thought or reasoning to process
    pub thought: String,
}

/// A tool that allows the agent to "think out loud" without taking action.
///
/// This is useful for:
/// - Breaking down complex problems
/// - Planning multi-step approaches
/// - Reasoning about tool selection
/// - Reflecting on previous results
#[derive(Debug, Serialize, Deserialize)]
pub struct Thinking;

impl Tool for Thinking {
    const NAME: &'static str = "thinking";

    type Error = ThinkingError;
    type Args = ThinkingArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: "thinking".to_string(),
            description: r#"Use this tool to think through a problem step by step.

This is a scratchpad for reasoning - use it to:
- Break down complex tasks into steps
- Plan your approach before taking action
- Reason about which tools to use
- Reflect on results and decide next steps

The thought will be recorded but no action will be taken."#.to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "thought": {
                        "type": "string",
                        "description": "Your reasoning, analysis, or plan"
                    }
                },
                "required": ["thought"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        // The thinking tool just echoes back acknowledgment
        // The actual value is in the logging/telemetry
        Ok(format!("Recorded thought ({} chars)", args.thought.len()))
    }
}
