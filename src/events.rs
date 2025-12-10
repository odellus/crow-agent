//! Agent event types
//!
//! These events are emitted by agents during execution and consumed by
//! output adapters (CLI, ACP, etc). This is the ONLY way agents communicate
//! with the outside world.

use futures::Stream;
use serde::Serialize;
use std::path::PathBuf;
use std::pin::Pin;

/// Events emitted during agent execution
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentEvent {
    // === Text Output ===
    /// Streaming text delta from the LLM
    TextDelta {
        agent: String,
        delta: String,
    },
    /// Complete text block
    TextComplete {
        agent: String,
        text: String,
    },

    // === Reasoning/Thinking ===
    /// Streaming reasoning delta (extended thinking)
    ThinkingDelta {
        agent: String,
        delta: String,
    },
    /// Complete thinking block
    ThinkingComplete {
        agent: String,
        text: String,
    },

    // === Tool Execution ===
    /// Tool call started
    ToolCallStart {
        agent: String,
        call_id: String,
        tool: String,
        arguments: serde_json::Value,
    },
    /// Tool call completed
    ToolCallEnd {
        agent: String,
        call_id: String,
        tool: String,
        arguments: serde_json::Value,
        output: String,
        is_error: bool,
        duration_ms: u64,
    },

    // === File Changes ===
    /// Files were modified
    FilesChanged {
        agent: String,
        files: Vec<PathBuf>,
        snapshot_hash: String,
    },

    // === Turn Lifecycle ===
    /// Turn started (entering ReAct loop)
    TurnStart {
        agent: String,
    },
    /// Turn completed (exiting ReAct loop)
    TurnComplete {
        agent: String,
        reason: TurnCompleteReason,
    },

    // === Control Flow (emitted by Agent, not BaseAgent) ===
    /// Coagent starting
    CoagentStart {
        primary: String,
        coagent: String,
    },
    /// Coagent finished
    CoagentEnd {
        primary: String,
        coagent: String,
    },
    /// Subagent spawned (via Task tool)
    SubagentStart {
        parent: String,
        subagent: String,
        task: String,
    },
    /// Subagent completed
    SubagentEnd {
        parent: String,
        subagent: String,
    },

    // === Errors & Cancellation ===
    Error {
        agent: String,
        error: String,
    },
    Cancelled {
        agent: String,
    },

    // === Telemetry ===
    /// Token usage
    Usage {
        agent: String,
        input_tokens: u64,
        output_tokens: u64,
        reasoning_tokens: Option<u64>,
    },

    /// Full LLM call trace (for telemetry)
    Trace {
        agent: String,
        model: String,
        request_messages: String,
        request_tools: Option<String>,
        response_content: Option<String>,
        response_tool_calls: Option<String>,
        input_tokens: Option<u64>,
        output_tokens: Option<u64>,
        latency_ms: u64,
        error: Option<String>,
    },
}

/// Reason the turn completed
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnCompleteReason {
    /// LLM responded with text only (no tool calls)
    TextResponse,
    /// task_complete tool was called
    TaskComplete { summary: String },
    /// Max iterations reached
    MaxIterations,
    /// Cancelled by user
    Cancelled,
}

/// Stream of agent events
pub type AgentEventStream = Pin<Box<dyn Stream<Item = AgentEvent> + Send>>;

/// Result of a single turn (returned by BaseAgent.execute_turn)
#[derive(Debug, Clone)]
pub struct TurnResult {
    /// Text output from this turn (if any)
    pub text: Option<String>,
    /// Thinking/reasoning output (if any)
    pub thinking: Option<String>,
    /// Tool calls that were executed
    pub tool_calls: Vec<ExecutedToolCall>,
    /// Why the turn ended
    pub reason: TurnCompleteReason,
    /// Token usage
    pub usage: TokenUsage,
    /// Files modified during this turn
    pub files_changed: Vec<PathBuf>,
}

impl TurnResult {
    /// Check if task_complete was called
    pub fn is_task_complete(&self) -> bool {
        matches!(self.reason, TurnCompleteReason::TaskComplete { .. })
    }

    /// Get task_complete summary if present
    pub fn task_complete_summary(&self) -> Option<&str> {
        match &self.reason {
            TurnCompleteReason::TaskComplete { summary } => Some(summary),
            _ => None,
        }
    }
}

/// A tool call that was executed
#[derive(Debug, Clone)]
pub struct ExecutedToolCall {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
    pub output: String,
    pub is_error: bool,
    pub duration_ms: u64,
}

/// Token usage for a turn
#[derive(Debug, Clone, Default)]
pub struct TokenUsage {
    pub input: u64,
    pub output: u64,
    pub reasoning: Option<u64>,
}
