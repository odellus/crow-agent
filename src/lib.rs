//! Crow Agent - A standalone LLM agent with tools
//!
//! This crate provides:
//! - A set of filesystem and shell tools for LLM agents
//! - CLI/REPL interface for testing and debugging
//! - ACP server mode for Zed integration

pub mod acp;
pub mod auth;
pub mod config;
pub mod lsp;
pub mod message;
pub mod snapshot;
pub mod telemetry;
pub mod templates;
pub mod trace_layer;

// Agent system
pub mod agent;
pub mod events;
pub mod provider;
pub mod tool;
pub mod tools;

pub use acp::run_stdio_server;
pub use auth::AuthConfig;
pub use config::Config;
pub use telemetry::{InteractionGuard, Telemetry, TraceGuard};

pub use agent::{Agent, AgentConfig, BaseAgent, ControlFlow};
pub use events::{AgentEvent, AgentEventStream, TurnResult};
pub use message::{
    AgentMessage, AgentMessageContent, Message, Thread, ToolResult, ToolUse, UserMessage,
};
pub use provider::{ProviderClient, ProviderConfig, StreamDelta};
