//! Task complete tool - signals task completion

use anyhow::Result;
use rig::tool::Tool;
use serde::{Deserialize, Serialize};

/// Call when the user's task is complete.
#[derive(Debug, Serialize, Deserialize)]
pub struct TaskCompleteInput {
    /// A summary of what was accomplished
    pub summary: String,
}

#[derive(Clone, Default)]
pub struct TaskComplete;

impl TaskComplete {
    pub fn new() -> Self {
        Self
    }
}

impl Tool for TaskComplete {
    const NAME: &'static str = "task_complete";

    type Error = std::convert::Infallible;
    type Args = TaskCompleteInput;
    type Output = String;

    async fn definition(&self, _prompt: String) -> rig::completion::ToolDefinition {
        rig::completion::ToolDefinition {
            name: Self::NAME.to_string(),
            description: r#"Call when the user's task is complete and correct.
Use this tool when you have finished the work and want to signal completion.
The summary will be shown to the user as the final response."#.to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "summary": {
                        "type": "string",
                        "description": "A summary of what was accomplished"
                    }
                },
                "required": ["summary"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        Ok(format!("Task completed: {}", args.summary))
    }
}
