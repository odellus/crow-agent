//! Control Flow Layer - External agent orchestration
//!
//! This is the external layer that:
//! - Orchestrates turns between primary and coagent
//! - Applies autonomy levels (ControlFlow enum)
//! - Manages the session from the CLI/ACP perspective
//!
//! It calls BaseAgent.execute_turn() and decides what happens after each turn.
//!
//! # Control Flow Levels
//!
//! 0. **Passthrough (HITL)** - ReAct turn ends, return control to user
//! 1. **Loop** - ReAct turn ends, immediately call LLM again (needs task_complete)
//! 2. **Static** - ReAct turn ends, inject canned "continue" message (needs task_complete)
//! 3. **Generated** - Run ONE prompt at session start to generate acceptance criteria,
//!    then inject that same AC message every time ReAct stops (needs task_complete)
//! 4. **Coagent** - Full dual-agent system (see below)
//!
//! # Coagent Architecture
//!
//! When using a coagent, two agents collaborate on the task:
//!
//! ## Session Initialization
//!
//! The coagent session is the *inverse* of the primary session:
//! - All user messages become assistant messages
//! - All assistant messages become user messages
//! - This gives coagent full context, seeing itself as the "helper" reviewing primary's work
//! - Coagent opens with "How can I help you today?" (assistants don't start conversations)
//!
//! ## The Orchestration Loop
//!
//! 1. Primary agent does ReAct loop, stops (text response or tools exhausted)
//! 2. Primary's turn gets compressed via `humanize_turn()` (tool calls → readable text)
//! 3. Compressed turn becomes a **user message** to coagent
//! 4. Coagent does its own ReAct loop, stops
//! 5. If coagent calls `task_complete` → done, return to user
//! 6. Otherwise, coagent's turn gets compressed
//! 7. Compressed coagent turn becomes a **user message** back to primary
//! 8. Repeat until someone calls `task_complete`
//!
//! ## From ACP/CLI Perspective
//!
//! - User sees continuous assistant responses (both primary and coagent outputs stream)
//! - User can interrupt at any time
//! - User input goes to whoever is currently active (primary or coagent)
//! - It appears as a single conversation with two agents collaborating internally
//!
//! ## Example Session Flow
//!
//! ```text
//! ACP/CLI SESSION (what user sees):
//!
//! user: execute this task
//! assistant: [primary starts working, streams tool calls]
//! assistant: [coagent reviews, provides feedback]
//! assistant: [primary continues based on feedback]
//! assistant: [coagent approves, calls task_complete]
//! user: [can now give next instruction]
//!
//! PRIMARY AGENT SESSION (internal):
//!
//! user: execute this task
//! assistant: [does ReAct loop with tools]
//! user: [compressed coagent feedback as user message]
//! assistant: [continues work]
//! user: [compressed coagent approval]
//!
//! COAGENT SESSION (internal, inverted from primary):
//!
//! system: [coagent system prompt]
//! assistant: "How can I help you today?"  <-- coagent opens
//! user: "execute this task"                <-- was primary's user message
//! assistant: [primary's first turn]        <-- was primary's assistant turn
//! user: [compressed primary work]          <-- injected by orchestrator
//! assistant: [reviews, provides feedback or calls task_complete]
//! ```
//!
//! ## Compression
//!
//! The `humanize_turn()` function compresses a full ReAct turn into readable text.
//! This is critical for keeping context manageable when passing between agents.
//! Tool calls become summarized text like "read `file.txt` (50 lines)".

use crate::agent::{fixtures::humanize_turn, AgentConfig, BaseAgent, ToolExecutor};
use crate::events::{AgentEvent, TurnCompleteReason, TurnResult};
use crate::provider::ProviderClient;
use crate::telemetry::Telemetry;
use crate::tools2::TodoStore;
use async_openai::types::{
    ChatCompletionMessageToolCall, ChatCompletionRequestAssistantMessageArgs,
    ChatCompletionRequestMessage, ChatCompletionRequestUserMessageArgs, ChatCompletionTool,
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// Initialize a coagent session by inverting roles from the primary session.
///
/// The coagent sees the conversation from the OPPOSITE perspective:
/// - Primary's user messages → Coagent's ASSISTANT messages (coagent "said" them)
/// - Primary's assistant messages → Coagent's USER messages (coagent sees primary's work)
/// - Session opens with user asking "How can I help you today?"
///
/// This way, the coagent thinks IT gave the original instructions and is now
/// reviewing what came back from the primary agent.
///
/// Example:
/// ```text
/// EXTERNAL (what user sees):
///   user: "fix the bug in auth.rs"
///   assistant: [primary works on it]
///
/// INTERNAL COAGENT (inverted):
///   user: "How can I help you today?"        <-- opens conversation
///   assistant: "fix the bug in auth.rs"      <-- coagent "gave" this instruction
///   user: [primary's work, humanized]        <-- coagent reviews this
/// ```
pub fn init_coagent_session(
    primary_messages: &[ChatCompletionRequestMessage],
) -> Vec<ChatCompletionRequestMessage> {
    let mut coagent_messages = Vec::new();

    // Session opens with context-setting message for the judge
    coagent_messages.push(ChatCompletionRequestMessage::User(
        ChatCompletionRequestUserMessageArgs::default()
            .content("You previously gave instructions to a coding agent. Review the work below and decide if the task is complete. If complete, call task_complete. Otherwise provide feedback.")
            .build()
            .unwrap(),
    ));

    // Invert each message from primary session
    for msg in primary_messages {
        match msg {
            ChatCompletionRequestMessage::System(_) => {
                // Skip system messages - coagent has its own system prompt
            }
            ChatCompletionRequestMessage::User(user_msg) => {
                // User messages become ASSISTANT messages for coagent
                // (coagent "said" the original instructions)
                let content = extract_user_content(user_msg);
                if !content.is_empty() {
                    coagent_messages.push(ChatCompletionRequestMessage::Assistant(
                        ChatCompletionRequestAssistantMessageArgs::default()
                            .content(content)
                            .build()
                            .unwrap(),
                    ));
                }
            }
            ChatCompletionRequestMessage::Assistant(asst_msg) => {
                // Assistant messages become user messages for coagent
                // We humanize tool calls if present, otherwise use content
                if let Some(tool_calls) = &asst_msg.tool_calls {
                    if !tool_calls.is_empty() {
                        // Summarize tool calls
                        let summary = summarize_tool_calls(tool_calls);
                        coagent_messages.push(ChatCompletionRequestMessage::User(
                            ChatCompletionRequestUserMessageArgs::default()
                                .content(summary)
                                .build()
                                .unwrap(),
                        ));
                    }
                } else if let Some(content) = &asst_msg.content {
                    let text = extract_content_text(content);
                    if !text.is_empty() {
                        coagent_messages.push(ChatCompletionRequestMessage::User(
                            ChatCompletionRequestUserMessageArgs::default()
                                .content(text)
                                .build()
                                .unwrap(),
                        ));
                    }
                }
            }
            ChatCompletionRequestMessage::Tool(tool_msg) => {
                // Tool results - summarize and add as user message
                let content_str = extract_tool_content(&tool_msg.content);
                let summary = format!("Tool result: {}", truncate_string(&content_str, 200));
                coagent_messages.push(ChatCompletionRequestMessage::User(
                    ChatCompletionRequestUserMessageArgs::default()
                        .content(summary)
                        .build()
                        .unwrap(),
                ));
            }
            _ => {
                // Skip other message types (Function, etc.)
            }
        }
    }

    coagent_messages
}

/// Extract text content from a user message
fn extract_user_content(user_msg: &async_openai::types::ChatCompletionRequestUserMessage) -> String {
    match &user_msg.content {
        async_openai::types::ChatCompletionRequestUserMessageContent::Text(text) => text.clone(),
        async_openai::types::ChatCompletionRequestUserMessageContent::Array(parts) => {
            let mut text = String::new();
            for part in parts {
                if let async_openai::types::ChatCompletionRequestUserMessageContentPart::Text(t) = part {
                    if !text.is_empty() {
                        text.push('\n');
                    }
                    text.push_str(&t.text);
                }
            }
            text
        }
    }
}

/// Extract text from assistant message content
fn extract_content_text(
    content: &async_openai::types::ChatCompletionRequestAssistantMessageContent,
) -> String {
    match content {
        async_openai::types::ChatCompletionRequestAssistantMessageContent::Text(text) => {
            text.clone()
        }
        async_openai::types::ChatCompletionRequestAssistantMessageContent::Array(parts) => {
            let mut text = String::new();
            for part in parts {
                if let async_openai::types::ChatCompletionRequestAssistantMessageContentPart::Text(
                    t,
                ) = part
                {
                    if !text.is_empty() {
                        text.push('\n');
                    }
                    text.push_str(&t.text);
                }
            }
            text
        }
    }
}

/// Summarize tool calls into human-readable text
fn summarize_tool_calls(tool_calls: &[ChatCompletionMessageToolCall]) -> String {
    let mut summaries = Vec::new();
    for call in tool_calls {
        let name = &call.function.name;
        let args = truncate_string(&call.function.arguments, 100);
        summaries.push(format!("Called `{}` with: {}", name, args));
    }
    if summaries.is_empty() {
        String::new()
    } else {
        summaries.join("\n")
    }
}

/// Extract text from tool message content
fn extract_tool_content(
    content: &async_openai::types::ChatCompletionRequestToolMessageContent,
) -> String {
    match content {
        async_openai::types::ChatCompletionRequestToolMessageContent::Text(text) => text.clone(),
        async_openai::types::ChatCompletionRequestToolMessageContent::Array(parts) => {
            let mut text = String::new();
            for part in parts {
                let async_openai::types::ChatCompletionRequestToolMessageContentPart::Text(t) =
                    part;
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(&t.text);
            }
            text
        }
    }
}

/// Truncate a string to max length with ellipsis
fn truncate_string(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len])
    }
}

/// Control flow determines what happens after each ReAct turn
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ControlFlow {
    /// Passthrough (HITL): Return to user after each turn
    /// No task_complete needed - user controls when to continue
    Passthrough,

    /// Loop: Immediately call LLM again after turn ends
    /// Requires task_complete tool to terminate
    #[default]
    Loop,

    /// Static: Inject a canned message after each turn
    /// Requires task_complete tool to terminate
    Static { message: String },

    /// Generated: Run ONE prompt at session start to generate acceptance criteria,
    /// then inject that same AC message every time ReAct stops
    /// Requires task_complete tool to terminate
    Generated {
        /// Prompt used to generate AC at session start
        generator_prompt: String,
        /// The generated AC (populated after first generation)
        #[serde(skip)]
        acceptance_criteria: Option<String>,
    },

    /// Coagent: Full dual-agent system
    /// Coagent is a complete Agent with its own config, tools, and ReAct loop
    /// Coagent config determines who has task_complete
    Coagent,
}

impl ControlFlow {
    /// Create ControlFlow from autonomy level (0-4)
    pub fn from_level(level: i8) -> Self {
        match level {
            0 => ControlFlow::Passthrough,
            1 => ControlFlow::Loop,
            2 => ControlFlow::Static {
                message: "Continue with the task. Call task_complete when done.".into(),
            },
            3 => ControlFlow::Generated {
                generator_prompt: "Based on the conversation so far, what are the acceptance criteria for this task? Be specific and concise.".into(),
                acceptance_criteria: None,
            },
            4 => ControlFlow::Coagent,
            _ => ControlFlow::Loop, // Default to loop for invalid levels
        }
    }

    /// Does this control flow use a coagent?
    pub fn has_coagent(&self) -> bool {
        matches!(self, ControlFlow::Coagent)
    }

    /// Does this control flow require task_complete tool?
    pub fn requires_task_complete(&self) -> bool {
        !matches!(self, ControlFlow::Passthrough)
    }
}

/// Result of running an Agent
#[derive(Debug)]
pub enum RunResult {
    /// Task completed (task_complete was called)
    Complete { summary: String, total_turns: usize },
    /// HITL mode - needs user input to continue
    NeedsInput {
        last_result: TurnResult,
        turns_so_far: usize,
    },
    /// Max turns reached
    MaxTurns { turns: usize },
    /// Cancelled by user
    Cancelled,
    /// Error occurred
    Error(String),
}

/// Orchestration agent that manages control flow between turns
///
/// This wraps one or two BaseAgents (primary + optional coagent)
/// and manages the turn-by-turn execution based on ControlFlow.
///
/// ALL external code (CLI, ACP server) should use this, never BaseAgent directly.
pub struct Agent {
    /// Agent name (for display/identification)
    pub name: String,
    /// Description
    pub description: Option<String>,
    /// Primary agent that does the work
    primary: BaseAgent,
    /// Optional coagent for supervision/verification
    coagent: Option<BaseAgent>,
    /// How/when coagent intercedes
    control_flow: ControlFlow,
    /// Max turns before giving up (across all iterations)
    max_turns: usize,
    /// Working directory
    working_dir: PathBuf,
    /// Primary agent config (kept for reference)
    pub config: AgentConfig,
    /// Primary session ID (for TodoStore sharing)
    primary_session_id: Option<String>,
    /// Coagent session ID (for TodoStore sharing)
    coagent_session_id: Option<String>,
    /// Shared TodoStore (for linking primary and coagent)
    todo_store: Option<TodoStore>,
}

impl Agent {
    /// Create a new Agent with just a primary agent (no coagent)
    pub fn new(
        name: impl Into<String>,
        primary_config: AgentConfig,
        provider: Arc<ProviderClient>,
        working_dir: PathBuf,
        control_flow: ControlFlow,
    ) -> Self {
        let name = name.into();
        Self {
            name: name.clone(),
            description: primary_config.description.clone(),
            config: primary_config.clone(),
            primary: BaseAgent::new(primary_config, provider, working_dir.clone()),
            coagent: None,
            control_flow,
            max_turns: 100,
            working_dir,
            primary_session_id: None,
            coagent_session_id: None,
            todo_store: None,
        }
    }

    /// Create a new Agent with telemetry
    pub fn with_telemetry(
        name: impl Into<String>,
        primary_config: AgentConfig,
        provider: Arc<ProviderClient>,
        working_dir: PathBuf,
        control_flow: ControlFlow,
        telemetry: Arc<Telemetry>,
    ) -> Self {
        let name = name.into();
        Self {
            name: name.clone(),
            description: primary_config.description.clone(),
            config: primary_config.clone(),
            primary: BaseAgent::with_telemetry(primary_config, provider, working_dir.clone(), telemetry),
            coagent: None,
            control_flow,
            max_turns: 100,
            working_dir,
            primary_session_id: None,
            coagent_session_id: None,
            todo_store: None,
        }
    }

    /// Create an Agent with both primary and coagent
    pub fn with_coagent(
        name: impl Into<String>,
        primary_config: AgentConfig,
        coagent_config: AgentConfig,
        provider: Arc<ProviderClient>,
        working_dir: PathBuf,
        control_flow: ControlFlow,
    ) -> Self {
        let name = name.into();
        let wd = working_dir.clone();
        Self {
            name: name.clone(),
            description: primary_config.description.clone(),
            config: primary_config.clone(),
            primary: BaseAgent::new(primary_config, provider.clone(), working_dir.clone()),
            coagent: Some(BaseAgent::new(coagent_config, provider, wd)),
            control_flow,
            max_turns: 100,
            working_dir,
            primary_session_id: None,
            coagent_session_id: None,
            todo_store: None,
        }
    }

    /// Create an Agent with both primary and coagent, with telemetry
    pub fn with_coagent_and_telemetry(
        name: impl Into<String>,
        primary_config: AgentConfig,
        coagent_config: AgentConfig,
        provider: Arc<ProviderClient>,
        working_dir: PathBuf,
        control_flow: ControlFlow,
        telemetry: Arc<Telemetry>,
    ) -> Self {
        let name = name.into();
        let wd = working_dir.clone();
        Self {
            name: name.clone(),
            description: primary_config.description.clone(),
            config: primary_config.clone(),
            primary: BaseAgent::with_telemetry(primary_config, provider.clone(), working_dir.clone(), telemetry.clone()),
            coagent: Some(BaseAgent::with_telemetry(coagent_config, provider, wd, telemetry)),
            control_flow,
            max_turns: 100,
            working_dir,
            primary_session_id: None,
            coagent_session_id: None,
            todo_store: None,
        }
    }

    /// Set up TodoStore sharing between primary and coagent
    ///
    /// This links the two session IDs so they share the same todo state.
    /// Must be called before running the agent if you want shared todos.
    ///
    /// # Arguments
    /// * `todo_store` - The shared TodoStore instance
    /// * `primary_session_id` - Session ID for the primary agent's todo tools
    /// * `coagent_session_id` - Session ID for the coagent's todo tools (usually same as primary)
    pub fn with_shared_todos(
        mut self,
        todo_store: TodoStore,
        primary_session_id: impl Into<String>,
        coagent_session_id: impl Into<String>,
    ) -> Self {
        let primary_id = primary_session_id.into();
        let coagent_id = coagent_session_id.into();

        // Link the sessions so they share the same underlying todo storage
        todo_store.share_sessions(&primary_id, &coagent_id);

        self.todo_store = Some(todo_store);
        self.primary_session_id = Some(primary_id);
        self.coagent_session_id = Some(coagent_id);
        self
    }

    /// Get the coagent session ID (for creating coagent tool registry)
    pub fn coagent_session_id(&self) -> Option<&str> {
        self.coagent_session_id.as_deref()
    }

    /// Get the shared TodoStore (for creating coagent tool registry)
    pub fn todo_store(&self) -> Option<&TodoStore> {
        self.todo_store.as_ref()
    }

    /// Set max turns
    pub fn max_turns(mut self, max: usize) -> Self {
        self.max_turns = max;
        self
    }

    /// Get working directory
    pub fn working_dir(&self) -> &PathBuf {
        &self.working_dir
    }

    /// Get control flow
    pub fn control_flow(&self) -> &ControlFlow {
        &self.control_flow
    }

    /// Run the agent until completion, passthrough break, or max turns
    ///
    /// This is the main orchestration loop that:
    /// 1. Runs primary.execute_turn()
    /// 2. Checks if done (task_complete)
    /// 3. Applies control flow (passthrough, static message, coagent, etc)
    /// 4. Repeats
    pub async fn run(
        &mut self,
        primary_messages: &mut Vec<ChatCompletionRequestMessage>,
        primary_tools: &[ChatCompletionTool],
        coagent_messages: &mut Option<Vec<ChatCompletionRequestMessage>>,
        coagent_tools: &[ChatCompletionTool],
        tool_executor: &dyn ToolExecutor,
        event_tx: mpsc::UnboundedSender<AgentEvent>,  // Takes ownership so channel closes when we return
        cancellation: CancellationToken,
    ) -> RunResult {
        let mut turns = 0;

        loop {
            if turns >= self.max_turns {
                return RunResult::MaxTurns { turns };
            }

            if cancellation.is_cancelled() {
                return RunResult::Cancelled;
            }

            turns += 1;

            // Primary agent takes a turn
            let result = match self
                .primary
                .execute_turn(
                    primary_messages,
                    primary_tools,
                    tool_executor,
                    &event_tx,
                    cancellation.clone(),
                )
                .await
            {
                Ok(r) => r,
                Err(e) => return RunResult::Error(e),
            };

            // Check if task_complete was called (handled inside ReAct loop)
            if let Some(summary) = result.task_complete_summary() {
                return RunResult::Complete {
                    summary: summary.to_string(),
                    total_turns: turns,
                };
            }

            // Check if cancelled
            if matches!(result.reason, TurnCompleteReason::Cancelled) {
                return RunResult::Cancelled;
            }

            // Apply control flow
            match &mut self.control_flow {
                ControlFlow::Passthrough => {
                    // Return to user
                    return RunResult::NeedsInput {
                        last_result: result,
                        turns_so_far: turns,
                    };
                }

                ControlFlow::Loop => {
                    // Just continue to next turn
                }

                ControlFlow::Static { message } => {
                    // Inject static message as user
                    primary_messages.push(
                        async_openai::types::ChatCompletionRequestUserMessageArgs::default()
                            .content(message.clone())
                            .build()
                            .unwrap()
                            .into(),
                    );
                }

                ControlFlow::Generated {
                    generator_prompt,
                    acceptance_criteria,
                } => {
                    // Get or generate acceptance criteria
                    let ac = match acceptance_criteria {
                        Some(ac) => ac.clone(),
                        None => {
                            // TODO: Run a single LLM call to generate AC from generator_prompt
                            // For now, use a default message
                            let generated = format!(
                                "Continue with the task. Acceptance criteria: {}",
                                generator_prompt
                            );
                            *acceptance_criteria = Some(generated.clone());
                            generated
                        }
                    };

                    primary_messages.push(
                        async_openai::types::ChatCompletionRequestUserMessageArgs::default()
                            .content(ac)
                            .build()
                            .unwrap()
                            .into(),
                    );
                }

                ControlFlow::Coagent => {
                    // Run coagent
                    if let Some(ref coagent) = self.coagent {
                        if let Some(ref mut coagent_msgs) = coagent_messages {
                            // Emit coagent start
                            let _ = event_tx.send(AgentEvent::CoagentStart {
                                primary: self.primary.name.clone(),
                                coagent: coagent.name.clone(),
                            });

                            // Compress primary's turn and add as user message to coagent
                            let primary_summary = humanize_turn(&result);
                            if !primary_summary.is_empty() {
                                coagent_msgs.push(
                                    async_openai::types::ChatCompletionRequestUserMessageArgs::default()
                                        .content(primary_summary)
                                        .build()
                                        .unwrap()
                                        .into(),
                                );
                            }

                            // Coagent takes its turn
                            let coagent_result = match coagent
                                .execute_turn(
                                    coagent_msgs,
                                    coagent_tools,
                                    tool_executor,
                                    &event_tx,
                                    cancellation.clone(),
                                )
                                .await
                            {
                                Ok(r) => r,
                                Err(e) => return RunResult::Error(e),
                            };

                            // Emit coagent end
                            let _ = event_tx.send(AgentEvent::CoagentEnd {
                                primary: self.primary.name.clone(),
                                coagent: coagent.name.clone(),
                            });

                            // Check if coagent called task_complete
                            if let Some(summary) = coagent_result.task_complete_summary() {
                                return RunResult::Complete {
                                    summary: summary.to_string(),
                                    total_turns: turns,
                                };
                            }

                            // Coagent's turn becomes humanized feedback to primary
                            let feedback = humanize_turn(&coagent_result);
                            if !feedback.is_empty() {
                                primary_messages.push(
                                    async_openai::types::ChatCompletionRequestUserMessageArgs::default()
                                        .content(feedback)
                                        .build()
                                        .unwrap()
                                        .into(),
                                );
                            }
                        }
                    }
                }
            }

            // Continue to next turn
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_openai::types::{
        ChatCompletionRequestAssistantMessageArgs, ChatCompletionRequestSystemMessageArgs,
        ChatCompletionRequestUserMessageArgs,
    };

    #[test]
    fn test_init_coagent_session_empty() {
        let primary: Vec<ChatCompletionRequestMessage> = vec![];
        let coagent = init_coagent_session(&primary);

        // Should have just the opening user message
        assert_eq!(coagent.len(), 1);
        assert!(matches!(
            &coagent[0],
            ChatCompletionRequestMessage::User(_)
        ));
    }

    #[test]
    fn test_init_coagent_session_inversion() {
        // Simulate a primary session:
        // system: "You are a coding agent"
        // user: "fix the bug in auth.rs"
        // assistant: "I'll look at auth.rs now"

        let primary: Vec<ChatCompletionRequestMessage> = vec![
            ChatCompletionRequestMessage::System(
                ChatCompletionRequestSystemMessageArgs::default()
                    .content("You are a coding agent")
                    .build()
                    .unwrap(),
            ),
            ChatCompletionRequestMessage::User(
                ChatCompletionRequestUserMessageArgs::default()
                    .content("fix the bug in auth.rs")
                    .build()
                    .unwrap(),
            ),
            ChatCompletionRequestMessage::Assistant(
                ChatCompletionRequestAssistantMessageArgs::default()
                    .content("I'll look at auth.rs now")
                    .build()
                    .unwrap(),
            ),
        ];

        let coagent = init_coagent_session(&primary);

        // Expected coagent session:
        // user: "How can I help you today?"
        // assistant: "fix the bug in auth.rs"  (was user msg, now coagent "said" it)
        // user: "I'll look at auth.rs now"     (was assistant msg, now coagent reviews it)

        assert_eq!(coagent.len(), 3);

        // First: user asks "How can I help you today?"
        match &coagent[0] {
            ChatCompletionRequestMessage::User(msg) => {
                let content = extract_user_content(msg);
                assert!(content.contains("How can I help you"));
            }
            _ => panic!("Expected user message first"),
        }

        // Second: coagent's "response" is the original user instruction
        match &coagent[1] {
            ChatCompletionRequestMessage::Assistant(msg) => {
                let content = extract_content_text(msg.content.as_ref().unwrap());
                assert!(content.contains("fix the bug"));
            }
            _ => panic!("Expected assistant message (inverted from user)"),
        }

        // Third: primary's work becomes user message for coagent to review
        match &coagent[2] {
            ChatCompletionRequestMessage::User(msg) => {
                let content = extract_user_content(msg);
                assert!(content.contains("auth.rs"));
            }
            _ => panic!("Expected user message (inverted from assistant)"),
        }
    }

    #[test]
    fn test_init_coagent_session_skips_system() {
        let primary: Vec<ChatCompletionRequestMessage> = vec![
            ChatCompletionRequestMessage::System(
                ChatCompletionRequestSystemMessageArgs::default()
                    .content("System prompt should be skipped")
                    .build()
                    .unwrap(),
            ),
        ];

        let coagent = init_coagent_session(&primary);

        // Only the opening user message, system was skipped
        assert_eq!(coagent.len(), 1);
        assert!(matches!(
            &coagent[0],
            ChatCompletionRequestMessage::User(_)
        ));
    }

    #[test]
    fn test_control_flow_requires_task_complete() {
        assert!(!ControlFlow::Passthrough.requires_task_complete());
        assert!(ControlFlow::Loop.requires_task_complete());
        assert!(ControlFlow::Static {
            message: "continue".into()
        }
        .requires_task_complete());
        assert!(ControlFlow::Coagent.requires_task_complete());
    }
}
