//! BaseAgent - Internal agent with ReAct loop
//!
//! This is the internal layer. It runs the ReAct loop:
//! 1. Call LLM (streaming)
//! 2. Execute tool calls
//! 3. Repeat until: text response, task_complete, or cancelled
//!
//! It knows NOTHING about control flow, coagents, or orchestration.
//! That's the job of Agent in control_flow.rs.

use crate::agent::AgentConfig;
use crate::events::{AgentEvent, ExecutedToolCall, TokenUsage, TurnCompleteReason, TurnResult};
use crate::provider::{ProviderClient, StreamDelta};
use crate::telemetry::{Telemetry, TraceBuilder, TraceGuard};
use async_openai::types::{
    ChatCompletionMessageToolCall, ChatCompletionRequestAssistantMessageArgs,
    ChatCompletionRequestMessage, ChatCompletionRequestToolMessageArgs, ChatCompletionTool,
    ChatCompletionToolType, FunctionCall,
};
use std::collections::HashMap;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

/// Doom loop detection threshold - if 3 consecutive tool calls have the same
/// name and identical arguments, we're likely stuck in a loop
const DOOM_LOOP_THRESHOLD: usize = 3;

/// Tracks recent tool calls for doom loop detection
#[derive(Debug, Clone)]
struct ToolCallRecord {
    name: String,
    args_hash: u64,
}

impl ToolCallRecord {
    fn new(name: &str, args: &serde_json::Value) -> Self {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        // Hash the canonical JSON string for consistent comparison
        let args_str = serde_json::to_string(args).unwrap_or_default();
        args_str.hash(&mut hasher);
        Self {
            name: name.to_string(),
            args_hash: hasher.finish(),
        }
    }
}

/// Check if recent tool calls indicate a doom loop
fn is_doom_loop(recent_calls: &VecDeque<ToolCallRecord>) -> bool {
    if recent_calls.len() < DOOM_LOOP_THRESHOLD {
        return false;
    }

    // Get the last DOOM_LOOP_THRESHOLD calls
    let calls: Vec<_> = recent_calls.iter().rev().take(DOOM_LOOP_THRESHOLD).collect();

    // Check if all have the same name and args hash
    let first = &calls[0];
    calls.iter().all(|c| c.name == first.name && c.args_hash == first.args_hash)
}

/// Internal agent that runs the ReAct loop
#[derive(Clone)]
pub struct BaseAgent {
    /// Agent name
    pub name: String,
    /// Agent configuration
    pub config: AgentConfig,
    /// LLM provider client
    provider: Arc<ProviderClient>,
    /// Working directory
    working_dir: PathBuf,
    /// Telemetry (optional)
    telemetry: Option<Arc<Telemetry>>,
    /// Session ID override - when set, traces use this instead of telemetry's session_id
    /// This is used for coagents which need their own internal session ID
    pub session_id_override: Option<Uuid>,
}

impl BaseAgent {
    /// Get working directory
    pub fn working_dir(&self) -> &PathBuf {
        &self.working_dir
    }
}

impl BaseAgent {
    /// Create a new BaseAgent
    pub fn new(config: AgentConfig, provider: Arc<ProviderClient>, working_dir: PathBuf) -> Self {
        Self {
            name: config.name.clone(),
            config,
            provider,
            working_dir,
            telemetry: None,
            session_id_override: None,
        }
    }

    /// Create a new BaseAgent with telemetry
    pub fn with_telemetry(
        config: AgentConfig,
        provider: Arc<ProviderClient>,
        working_dir: PathBuf,
        telemetry: Arc<Telemetry>,
    ) -> Self {
        Self {
            name: config.name.clone(),
            config,
            provider,
            working_dir,
            telemetry: Some(telemetry),
            session_id_override: None,
        }
    }

    /// Set session ID override for trace logging
    /// Used for coagents that need their own internal session ID
    pub fn with_session_id(mut self, session_id: Uuid) -> Self {
        self.session_id_override = Some(session_id);
        self
    }

    /// Execute a full turn (ReAct loop until done)
    ///
    /// Runs until:
    /// - LLM responds with text only (no tool calls)
    /// - task_complete tool is called
    /// - Cancelled via token
    /// - Max iterations reached
    ///
    /// Emits AgentEvents through event_tx as it executes.
    /// Returns TurnResult describing what happened.
    pub async fn execute_turn(
        &self,
        messages: &mut Vec<ChatCompletionRequestMessage>,
        tools: &[ChatCompletionTool],
        tool_executor: &dyn ToolExecutor,
        event_tx: &mpsc::UnboundedSender<AgentEvent>,
        cancellation: CancellationToken,
    ) -> Result<TurnResult, String> {
        let max_iterations = self.config.max_iterations.unwrap_or(50);

        let mut total_usage = TokenUsage::default();
        let mut all_tool_calls: Vec<ExecutedToolCall> = vec![];
        let files_changed: Vec<PathBuf> = vec![];
        let mut final_text: Option<String> = None;
        let mut final_thinking: Option<String> = None;

        // Track recent tool calls for doom loop detection
        let mut recent_tool_calls: VecDeque<ToolCallRecord> = VecDeque::with_capacity(DOOM_LOOP_THRESHOLD + 1);

        // Emit turn start
        let _ = event_tx.send(AgentEvent::TurnStart {
            agent: self.name.clone(),
        });

        for _iteration in 0..max_iterations {
            // Check cancellation
            if cancellation.is_cancelled() {
                let _ = event_tx.send(AgentEvent::Cancelled {
                    agent: self.name.clone(),
                });
                return Ok(TurnResult {
                    text: final_text,
                    thinking: final_thinking,
                    tool_calls: all_tool_calls,
                    reason: TurnCompleteReason::Cancelled,
                    usage: total_usage,
                    files_changed,
                });
            }

            // Create channel for streaming deltas
            let (delta_tx, mut delta_rx) = mpsc::unbounded_channel();

            // Start trace for this LLM call - TraceGuard saves on drop if interrupted
            let model_name = self
                .config
                .model
                .clone()
                .unwrap_or_else(|| self.provider.config().default_model.clone());
            let mut trace_guard = self.telemetry.as_ref().map(|t| {
                let msgs_json = serde_json::to_string(&messages).unwrap_or_default();
                let tools_json = serde_json::to_string(&tools).ok();

                // Use session_id_override if set (for coagents), otherwise use telemetry's session_id
                let session_id = self.session_id_override.unwrap_or_else(|| t.session_id());
                let builder = TraceBuilder::new(
                    session_id,
                    &self.name,
                    &self.provider.config().name,
                    &model_name,
                    msgs_json,
                )
                .with_tools(tools_json.unwrap_or_default());
                let mut guard = TraceGuard::new(t.clone(), builder);
                // Flush immediately to ensure request is saved even on early termination
                guard.flush();
                guard
            });

            // Spawn streaming task
            let provider = self.provider.clone();
            let msgs = messages.clone();
            let tool_defs = tools.to_vec();
            let model = self.config.model.clone();
            let cancel = cancellation.clone();

            let stream_handle = tokio::spawn(async move {
                provider
                    .chat_stream(msgs, tool_defs, model.as_deref(), delta_tx, Some(cancel))
                    .await
            });

            // Accumulate response
            let mut accumulated_text = String::new();
            let mut accumulated_thinking = String::new();
            let mut tool_call_parts: HashMap<usize, (String, String, String)> = HashMap::new();

            // Process streaming deltas
            loop {
                let delta = tokio::select! {
                    biased;
                    _ = cancellation.cancelled() => {
                        stream_handle.abort();
                        let _ = event_tx.send(AgentEvent::Cancelled {
                            agent: self.name.clone(),
                        });
                        return Ok(TurnResult {
                            text: final_text,
                            thinking: final_thinking,
                            tool_calls: all_tool_calls,
                            reason: TurnCompleteReason::Cancelled,
                            usage: total_usage,
                            files_changed,
                        });
                    }
                    delta = delta_rx.recv() => delta,
                };

                let Some(delta) = delta else {
                    break;
                };

                match delta {
                    StreamDelta::Text(text) => {
                        let _ = event_tx.send(AgentEvent::TextDelta {
                            agent: self.name.clone(),
                            delta: text.clone(),
                        });
                        accumulated_text.push_str(&text);
                        // Update trace guard and flush on every delta
                        // DB writes are cheap, losing tokens is not
                        if let Some(ref mut guard) = trace_guard {
                            guard.push_text(&text);
                            guard.flush();
                        }
                    }
                    StreamDelta::Reasoning(text) => {
                        let _ = event_tx.send(AgentEvent::ThinkingDelta {
                            agent: self.name.clone(),
                            delta: text.clone(),
                        });
                        accumulated_thinking.push_str(&text);
                        // Update trace guard and flush on every delta
                        if let Some(ref mut guard) = trace_guard {
                            guard.push_thinking(&text);
                            guard.flush();
                        }
                    }
                    StreamDelta::ToolCall {
                        index,
                        id,
                        name,
                        arguments,
                    } => {
                        let entry = tool_call_parts
                            .entry(index)
                            .or_insert_with(|| (String::new(), String::new(), String::new()));
                        if let Some(id) = id {
                            entry.0 = id;
                        }
                        if let Some(name) = name {
                            entry.1 = name;
                        }
                        entry.2.push_str(&arguments);
                    }
                    StreamDelta::Usage {
                        input,
                        output,
                        reasoning,
                    } => {
                        total_usage.input += input;
                        total_usage.output += output;
                        if let Some(r) = reasoning {
                            total_usage.reasoning = Some(total_usage.reasoning.unwrap_or(0) + r);
                        }
                        // Update trace guard with usage
                        if let Some(ref mut guard) = trace_guard {
                            guard.set_usage(input, output);
                        }

                        let _ = event_tx.send(AgentEvent::Usage {
                            agent: self.name.clone(),
                            input_tokens: input,
                            output_tokens: output,
                            reasoning_tokens: reasoning,
                        });
                    }
                    StreamDelta::Done => break,
                }
            }

            // Wait for stream to complete
            let stream_result = stream_handle.await;

            // Add tool calls to trace guard and flush
            // This ensures tool calls are saved even if we crash during tool execution
            if let Some(ref mut guard) = trace_guard {
                for (_, (id, name, args)) in &tool_call_parts {
                    guard.push_tool_call(id, name, args);
                }
                // Mark error if stream failed
                match &stream_result {
                    Ok(Err(e)) => guard.set_error(e.clone()),
                    Err(e) => guard.set_error(e.to_string()),
                    Ok(Ok(())) => {}
                }
                // Flush to ensure tool calls are persisted
                guard.flush();
            }

            // NOTE: Don't complete trace here - wait until after tool execution
            // so we can capture the final message state including tool results
            // The trace will be completed after tools run, or on early return

            // Emit thinking complete if present
            if !accumulated_thinking.is_empty() {
                let _ = event_tx.send(AgentEvent::ThinkingComplete {
                    agent: self.name.clone(),
                    text: accumulated_thinking.clone(),
                });
                final_thinking = Some(accumulated_thinking);
            }

            // No tool calls = done
            if tool_call_parts.is_empty() {
                if !accumulated_text.is_empty() {
                    // Add assistant message to context before returning
                    // This is critical for coagent sessions to maintain correct message ordering
                    messages.push(ChatCompletionRequestMessage::Assistant(
                        ChatCompletionRequestAssistantMessageArgs::default()
                            .content(accumulated_text.clone())
                            .build()
                            .map_err(|e| format!("Failed to build assistant message: {}", e))?,
                    ));

                    let _ = event_tx.send(AgentEvent::TextComplete {
                        agent: self.name.clone(),
                        text: accumulated_text.clone(),
                    });
                    final_text = Some(accumulated_text);
                }

                // Complete trace with final message state
                if let Some(mut guard) = trace_guard.take() {
                    let msgs_json = serde_json::to_string(&messages).unwrap_or_default();
                    guard.update_request_messages(msgs_json);
                    guard.complete();
                }

                let _ = event_tx.send(AgentEvent::TurnComplete {
                    agent: self.name.clone(),
                    reason: TurnCompleteReason::TextResponse,
                });

                return Ok(TurnResult {
                    text: final_text,
                    thinking: final_thinking,
                    tool_calls: all_tool_calls,
                    reason: TurnCompleteReason::TextResponse,
                    usage: total_usage,
                    files_changed,
                });
            }

            // Build tool calls for context
            let mut openai_tool_calls: Vec<ChatCompletionMessageToolCall> = vec![];
            for (_index, (id, name, args)) in &tool_call_parts {
                openai_tool_calls.push(ChatCompletionMessageToolCall {
                    id: id.clone(),
                    r#type: ChatCompletionToolType::Function,
                    function: FunctionCall {
                        name: name.clone(),
                        arguments: args.clone(),
                    },
                });
            }

            // Add assistant message with tool calls to context
            messages.push(ChatCompletionRequestMessage::Assistant(
                ChatCompletionRequestAssistantMessageArgs::default()
                    .tool_calls(openai_tool_calls)
                    .build()
                    .map_err(|e| format!("Failed to build assistant message: {}", e))?,
            ));

            // Execute each tool
            for (_index, (tool_id, tool_name, tool_args_str)) in tool_call_parts {
                // Check cancellation before each tool
                if cancellation.is_cancelled() {
                    let _ = event_tx.send(AgentEvent::Cancelled {
                        agent: self.name.clone(),
                    });
                    return Ok(TurnResult {
                        text: final_text,
                        thinking: final_thinking,
                        tool_calls: all_tool_calls,
                        reason: TurnCompleteReason::Cancelled,
                        usage: total_usage,
                        files_changed,
                    });
                }

                let args: serde_json::Value =
                    serde_json::from_str(&tool_args_str).unwrap_or(serde_json::json!({}));

                // Track this call for doom loop detection
                let call_record = ToolCallRecord::new(&tool_name, &args);
                recent_tool_calls.push_back(call_record);
                if recent_tool_calls.len() > DOOM_LOOP_THRESHOLD {
                    recent_tool_calls.pop_front();
                }

                // Check for doom loop - 3 identical consecutive tool calls
                if is_doom_loop(&recent_tool_calls) {
                    let doom_error = format!(
                        "Doom loop detected: '{}' called {} times with identical arguments. \
                        You seem to be stuck. Please try a different approach or ask for help.",
                        tool_name, DOOM_LOOP_THRESHOLD
                    );

                    // Emit tool error event
                    let _ = event_tx.send(AgentEvent::ToolCallStart {
                        agent: self.name.clone(),
                        call_id: tool_id.clone(),
                        tool: tool_name.clone(),
                        arguments: args.clone(),
                    });
                    let _ = event_tx.send(AgentEvent::ToolCallEnd {
                        agent: self.name.clone(),
                        call_id: tool_id.clone(),
                        tool: tool_name.clone(),
                        arguments: args.clone(),
                        output: doom_error.clone(),
                        is_error: true,
                        duration_ms: 0,
                    });

                    // Track as error
                    all_tool_calls.push(ExecutedToolCall {
                        id: tool_id.clone(),
                        name: tool_name.clone(),
                        arguments: args,
                        output: doom_error.clone(),
                        is_error: true,
                        duration_ms: 0,
                    });

                    // Add error to context so model sees it
                    messages.push(ChatCompletionRequestMessage::Tool(
                        ChatCompletionRequestToolMessageArgs::default()
                            .content(doom_error)
                            .tool_call_id(tool_id)
                            .build()
                            .map_err(|e| format!("Failed to build tool message: {}", e))?,
                    ));

                    // Clear recent calls so we give the model a fresh chance
                    recent_tool_calls.clear();

                    // Continue to next tool or iteration - don't execute this one
                    continue;
                }

                // Emit tool start
                let _ = event_tx.send(AgentEvent::ToolCallStart {
                    agent: self.name.clone(),
                    call_id: tool_id.clone(),
                    tool: tool_name.clone(),
                    arguments: args.clone(),
                });

                let start = Instant::now();

                // Execute the tool
                let result = tool_executor
                    .execute(&tool_name, args.clone(), &self.working_dir, &cancellation)
                    .await;

                let duration_ms = start.elapsed().as_millis() as u64;
                let (output, is_error) = match result {
                    Ok(output) => (output, false),
                    Err(e) => (e, true),
                };

                // Emit tool end
                let _ = event_tx.send(AgentEvent::ToolCallEnd {
                    agent: self.name.clone(),
                    call_id: tool_id.clone(),
                    tool: tool_name.clone(),
                    arguments: args.clone(),
                    output: output.clone(),
                    is_error,
                    duration_ms,
                });

                // Track this tool call
                all_tool_calls.push(ExecutedToolCall {
                    id: tool_id.clone(),
                    name: tool_name.clone(),
                    arguments: args,
                    output: output.clone(),
                    is_error,
                    duration_ms,
                });

                // Add tool result to context
                messages.push(ChatCompletionRequestMessage::Tool(
                    ChatCompletionRequestToolMessageArgs::default()
                        .content(output.clone())
                        .tool_call_id(tool_id)
                        .build()
                        .map_err(|e| format!("Failed to build tool message: {}", e))?,
                ));

                // CRITICAL: Check for task_complete
                if tool_name == "task_complete" && !is_error {
                    // Complete trace with final message state including task_complete result
                    if let Some(mut guard) = trace_guard.take() {
                        let msgs_json = serde_json::to_string(&messages).unwrap_or_default();
                        guard.update_request_messages(msgs_json);
                        guard.complete();
                    }

                    let _ = event_tx.send(AgentEvent::TurnComplete {
                        agent: self.name.clone(),
                        reason: TurnCompleteReason::TaskComplete {
                            summary: output.clone(),
                        },
                    });

                    return Ok(TurnResult {
                        text: final_text,
                        thinking: final_thinking,
                        tool_calls: all_tool_calls,
                        reason: TurnCompleteReason::TaskComplete { summary: output },
                        usage: total_usage,
                        files_changed,
                    });
                }
            }

            // Complete trace after all tools in this iteration (if not already taken by task_complete)
            if let Some(mut guard) = trace_guard.take() {
                let msgs_json = serde_json::to_string(&messages).unwrap_or_default();
                guard.update_request_messages(msgs_json);
                guard.complete();
            }

            // Continue to next iteration
        }

        // Max iterations reached
        let _ = event_tx.send(AgentEvent::TurnComplete {
            agent: self.name.clone(),
            reason: TurnCompleteReason::MaxIterations,
        });

        Ok(TurnResult {
            text: final_text,
            thinking: final_thinking,
            tool_calls: all_tool_calls,
            reason: TurnCompleteReason::MaxIterations,
            usage: total_usage,
            files_changed,
        })
    }
}

/// Trait for executing tools
///
/// This is passed to execute_turn so the ReAct loop doesn't need to
/// know about tool registry details.
#[async_trait::async_trait]
pub trait ToolExecutor: Send + Sync {
    /// Execute a tool by name
    async fn execute(
        &self,
        name: &str,
        args: serde_json::Value,
        working_dir: &PathBuf,
        cancellation: &CancellationToken,
    ) -> Result<String, String>;
}
