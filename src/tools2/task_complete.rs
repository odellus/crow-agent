//! Task complete tool - signals task completion

use crate::tool::{Tool, ToolContext, ToolDefinition, ToolResult};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;

#[derive(Debug, Deserialize)]
struct Args {
    summary: String,
}

pub struct TaskCompleteTool;

impl TaskCompleteTool {
    pub fn new() -> Self {
        Self
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
            description: "Call when the task is complete. Provide a summary of what was done."
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

    async fn execute(&self, args: serde_json::Value, _ctx: &ToolContext) -> ToolResult {
        let args: Args = match serde_json::from_value(args) {
            Ok(a) => a,
            Err(e) => return ToolResult::error(format!("Invalid arguments: {}", e)),
        };

        // The summary is returned - the ReAct loop checks for this tool name
        // and breaks out when it sees it
        ToolResult::success(args.summary)
    }
}
