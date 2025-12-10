//! Tool system - our own trait, no rig dependency
//!
//! Tools implement the `Tool` trait and are registered with `ToolRegistry`.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

/// Tool definition for LLM (matches OpenAI format)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

/// Result of a tool execution
#[derive(Debug, Clone)]
pub struct ToolResult {
    pub output: String,
    pub is_error: bool,
}

impl ToolResult {
    pub fn success(output: impl Into<String>) -> Self {
        Self {
            output: output.into(),
            is_error: false,
        }
    }

    pub fn error(message: impl Into<String>) -> Self {
        Self {
            output: message.into(),
            is_error: true,
        }
    }
}

/// Context passed to tools during execution
#[derive(Clone)]
pub struct ToolContext {
    pub working_dir: PathBuf,
    pub cancellation: CancellationToken,
}

impl ToolContext {
    pub fn new(working_dir: PathBuf, cancellation: CancellationToken) -> Self {
        Self {
            working_dir,
            cancellation,
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancellation.is_cancelled()
    }
}

/// Our own Tool trait - no rig dependency
#[async_trait]
pub trait Tool: Send + Sync {
    /// Tool name (used for dispatch)
    fn name(&self) -> &str;

    /// Get the tool definition for LLM
    fn definition(&self) -> ToolDefinition;

    /// Execute the tool with given arguments
    async fn execute(&self, args: Value, ctx: &ToolContext) -> ToolResult;

    /// Humanize a tool call into a concise summary for coagent context.
    ///
    /// Takes the arguments and result, returns a short markdown summary.
    /// Used to compress tool calls when passing context between agents.
    ///
    /// Default implementation uses tool name + truncated args.
    /// Override for tool-specific formatting.
    fn humanize(&self, args: &Value, result: &ToolResult) -> Option<String> {
        let args_preview = summarize_args(args);
        if result.is_error {
            Some(format!("{} failed: {}", self.name(), truncate(&result.output, 100)))
        } else {
            Some(format!("{}({})", self.name(), args_preview))
        }
    }
}

/// Summarize args into a short string for humanization
fn summarize_args(args: &Value) -> String {
    match args {
        Value::Object(map) => {
            let parts: Vec<String> = map
                .iter()
                .take(2) // max 2 args shown
                .map(|(k, v)| {
                    let val = match v {
                        Value::String(s) => truncate(s, 30),
                        Value::Number(n) => n.to_string(),
                        Value::Bool(b) => b.to_string(),
                        _ => "...".to_string(),
                    };
                    format!("{}={}", k, val)
                })
                .collect();
            parts.join(", ")
        }
        _ => "...".to_string(),
    }
}

/// Truncate a string with ellipsis
fn truncate(s: &str, max: usize) -> String {
    let s = s.trim();
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...", &s[..max])
    }
}

/// Registry of available tools
#[derive(Clone)]
pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
        }
    }

    /// Register a tool
    pub fn register<T: Tool + 'static>(&mut self, tool: T) {
        let name = tool.name().to_string();
        self.tools.insert(name, Arc::new(tool));
    }

    /// Get a tool by name
    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.get(name).cloned()
    }

    /// List all tool names
    pub fn names(&self) -> Vec<&str> {
        self.tools.keys().map(|s| s.as_str()).collect()
    }

    /// Get all tool definitions (for LLM)
    pub fn definitions(&self) -> Vec<ToolDefinition> {
        self.tools.values().map(|t| t.definition()).collect()
    }

    /// Convert to OpenAI ChatCompletionTool format
    pub fn to_openai_tools(&self) -> Vec<async_openai::types::ChatCompletionTool> {
        self.tools
            .values()
            .map(|t| {
                let def = t.definition();
                async_openai::types::ChatCompletionTool {
                    r#type: async_openai::types::ChatCompletionToolType::Function,
                    function: async_openai::types::FunctionObject {
                        name: def.name,
                        description: Some(def.description),
                        parameters: Some(def.parameters),
                        strict: None,
                    },
                }
            })
            .collect()
    }

    /// Execute a tool by name
    pub async fn execute(&self, name: &str, args: Value, ctx: &ToolContext) -> ToolResult {
        match self.get(name) {
            Some(tool) => tool.execute(args, ctx).await,
            None => ToolResult::error(format!("Unknown tool: {}", name)),
        }
    }

    /// Humanize a tool call (delegates to the tool's humanize method)
    pub fn humanize(&self, name: &str, args: &Value, result: &ToolResult) -> Option<String> {
        self.get(name).and_then(|tool| tool.humanize(args, result))
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// Implement ToolExecutor for ToolRegistry so it can be used with BaseAgent
use crate::agent::ToolExecutor;

#[async_trait]
impl ToolExecutor for ToolRegistry {
    async fn execute(
        &self,
        name: &str,
        args: Value,
        working_dir: &PathBuf,
        cancellation: &CancellationToken,
    ) -> Result<String, String> {
        let ctx = ToolContext::new(working_dir.clone(), cancellation.clone());
        let result = self.execute(name, args, &ctx).await;
        if result.is_error {
            Err(result.output)
        } else {
            Ok(result.output)
        }
    }
}
