//! Control Flow Layer - External agent orchestration
//!
//! This is the external layer that:
//! - Orchestrates turns between primary and coagent
//! - Applies autonomy levels (ControlFlow enum)
//! - Manages the session from the CLI/ACP perspective
//!
//! It calls BaseAgent.execute_turn() and decides what happens after each turn.

use crate::agent::{fixtures::humanize_turn, AgentConfig, BaseAgent, ToolExecutor};
use crate::events::{AgentEvent, TurnCompleteReason, TurnResult};
use crate::provider::ProviderClient;
use async_openai::types::{ChatCompletionRequestMessage, ChatCompletionTool};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// Control flow determines what happens after each turn
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ControlFlow {
    /// Level -1: Return to user after each turn (human-in-the-loop)
    HITL,

    /// Level 0: Loop until task_complete, no intervention
    Loop,

    /// Level 1: Inject a static message after each turn
    Static { message: String },

    /// Level 2: Generate a contextual message (acceptance criteria)
    Generated { generator_prompt: String },

    /// Levels 3-5: Coagent intercedes between turns
    Coagent {
        /// Which tools the coagent has access to
        tools: CoagentTools,
        /// Can coagent call task_complete to end the run?
        can_terminate: bool,
    },
}

/// What tools a coagent has access to
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoagentTools {
    /// Level 3: No tools, just chat
    None,
    /// Level 4: Read-only tools (read, grep, list, etc)
    ReadOnly,
    /// Level 5: Full tools (same as primary)
    Full,
}

impl ControlFlow {
    /// Create ControlFlow from autonomy level (-1 to 5)
    pub fn from_level(level: i8) -> Self {
        match level {
            -1 => ControlFlow::HITL,
            0 => ControlFlow::Loop,
            1 => ControlFlow::Static {
                message: "Continue with the task. Call task_complete when done.".into(),
            },
            2 => ControlFlow::Generated {
                generator_prompt: "Based on the task, what are the acceptance criteria?".into(),
            },
            3 => ControlFlow::Coagent {
                tools: CoagentTools::None,
                can_terminate: false,
            },
            4 => ControlFlow::Coagent {
                tools: CoagentTools::ReadOnly,
                can_terminate: true,
            },
            5 => ControlFlow::Coagent {
                tools: CoagentTools::Full,
                can_terminate: true,
            },
            _ => ControlFlow::Loop,
        }
    }

    /// Does this control flow use a coagent?
    pub fn has_coagent(&self) -> bool {
        matches!(self, ControlFlow::Coagent { .. })
    }
}

/// Result of running an ACPAgent
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

/// External agent that orchestrates control flow
///
/// This wraps one or two BaseAgents (primary + optional coagent)
/// and manages the turn-by-turn execution based on ControlFlow.
pub struct ACPAgent {
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
}

impl ACPAgent {
    /// Create a new ACPAgent with just a primary agent (no coagent)
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
            primary: BaseAgent::new(primary_config, provider, working_dir),
            coagent: None,
            control_flow,
            max_turns: 100,
        }
    }

    /// Create an ACPAgent with both primary and coagent
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
            primary: BaseAgent::new(primary_config, provider.clone(), working_dir),
            coagent: Some(BaseAgent::new(coagent_config, provider, wd)),
            control_flow,
            max_turns: 100,
        }
    }

    /// Set max turns
    pub fn max_turns(mut self, max: usize) -> Self {
        self.max_turns = max;
        self
    }

    /// Run the agent until completion, HITL break, or max turns
    ///
    /// This is the main orchestration loop that:
    /// 1. Runs primary.execute_turn()
    /// 2. Checks if done (task_complete)
    /// 3. Applies control flow (HITL, static message, coagent, etc)
    /// 4. Repeats
    pub async fn run(
        &self,
        primary_messages: &mut Vec<ChatCompletionRequestMessage>,
        primary_tools: &[ChatCompletionTool],
        coagent_messages: &mut Option<Vec<ChatCompletionRequestMessage>>,
        coagent_tools: &[ChatCompletionTool],
        tool_executor: &dyn ToolExecutor,
        event_tx: &mpsc::UnboundedSender<AgentEvent>,
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
                    event_tx,
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
            match &self.control_flow {
                ControlFlow::HITL => {
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
                    generator_prompt: _,
                } => {
                    // TODO: Generate AC message using a separate LLM call
                    // For now, fall back to a static message
                    primary_messages.push(
                        async_openai::types::ChatCompletionRequestUserMessageArgs::default()
                            .content("Continue with the task. Are the acceptance criteria met?")
                            .build()
                            .unwrap()
                            .into(),
                    );
                }

                ControlFlow::Coagent { can_terminate, .. } => {
                    // Run coagent
                    if let Some(ref coagent) = self.coagent {
                        if let Some(ref mut coagent_msgs) = coagent_messages {
                            // Emit coagent start
                            let _ = event_tx.send(AgentEvent::CoagentStart {
                                primary: self.primary.name.clone(),
                                coagent: coagent.name.clone(),
                            });

                            // Coagent gets context about what primary did
                            // (In practice, you'd summarize or share the primary's last turn)
                            let coagent_result = match coagent
                                .execute_turn(
                                    coagent_msgs,
                                    coagent_tools,
                                    tool_executor,
                                    event_tx,
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
                            if *can_terminate {
                                if let Some(summary) = coagent_result.task_complete_summary() {
                                    return RunResult::Complete {
                                        summary: summary.to_string(),
                                        total_turns: turns,
                                    };
                                }
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
