//! PromptHook implementation for capturing rig events into our telemetry system
//!
//! This hooks into rig's agent execution to capture:
//! - Completion calls (before LLM request)
//! - Completion responses (after LLM response)
//! - Tool calls (before tool execution)
//! - Tool results (after tool execution)

use crate::telemetry::Telemetry;
use rig::agent::{CancelSignal, PromptHook};
use rig::completion::CompletionModel;
use rig::message::Message;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::Mutex;

/// A PromptHook that logs all events to our telemetry system
#[derive(Clone)]
pub struct TelemetryHook {
    telemetry: Arc<Telemetry>,
    /// Track timing for tool calls
    tool_start: Arc<Mutex<Option<Instant>>>,
    /// Current tool name being executed
    current_tool: Arc<Mutex<Option<String>>>,
    /// Current tool args
    current_args: Arc<Mutex<Option<String>>>,
}

impl TelemetryHook {
    pub fn new(telemetry: Arc<Telemetry>) -> Self {
        Self {
            telemetry,
            tool_start: Arc::new(Mutex::new(None)),
            current_tool: Arc::new(Mutex::new(None)),
            current_args: Arc::new(Mutex::new(None)),
        }
    }
}

impl<M> PromptHook<M> for TelemetryHook
where
    M: CompletionModel,
{
    async fn on_completion_call(
        &self,
        prompt: &Message,
        history: &[Message],
        _cancel_sig: CancelSignal,
    ) {
        let prompt_text = match prompt {
            Message::User { content } => {
                // Extract text from user content
                format!("{:?}", content)
            }
            Message::Assistant { content, .. } => {
                format!("{:?}", content)
            }
        };

        tracing::debug!(
            prompt_len = prompt_text.len(),
            history_len = history.len(),
            "Completion call starting"
        );
    }

    async fn on_completion_response(
        &self,
        _prompt: &Message,
        response: &rig::completion::CompletionResponse<M::Response>,
        _cancel_sig: CancelSignal,
    ) {
        tracing::debug!(
            has_tool_calls = !response.choice.iter().all(|c| {
                matches!(c, rig::message::AssistantContent::Text(_))
            }),
            "Completion response received"
        );
    }

    async fn on_tool_call(
        &self,
        tool_name: &str,
        args: &str,
        _cancel_sig: CancelSignal,
    ) {
        // Store timing and tool info
        *self.tool_start.lock().await = Some(Instant::now());
        *self.current_tool.lock().await = Some(tool_name.to_string());
        *self.current_args.lock().await = Some(args.to_string());

        tracing::info!(
            tool = tool_name,
            args_len = args.len(),
            "Tool call starting"
        );
    }

    async fn on_tool_result(
        &self,
        tool_name: &str,
        args: &str,
        result: &str,
        _cancel_sig: CancelSignal,
    ) {
        let duration_ms = self
            .tool_start
            .lock()
            .await
            .take()
            .map(|start| start.elapsed().as_millis() as u64)
            .unwrap_or(0);

        // Parse args as JSON for storage
        let args_json: serde_json::Value = serde_json::from_str(args)
            .unwrap_or_else(|_| serde_json::Value::String(args.to_string()));

        // Log to telemetry
        self.telemetry
            .log_tool_call(tool_name, &args_json, Ok(result), duration_ms)
            .await;

        tracing::info!(
            tool = tool_name,
            duration_ms = duration_ms,
            result_len = result.len(),
            "Tool call completed"
        );

        // Clear state
        *self.current_tool.lock().await = None;
        *self.current_args.lock().await = None;
    }
}
