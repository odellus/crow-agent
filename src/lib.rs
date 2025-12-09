//! Crow Agent - A standalone LLM agent with tools
//!
//! This crate provides:
//! - A set of filesystem and shell tools for LLM agents
//! - CLI/REPL interface for testing and debugging
//! - ACP server mode for Zed integration (future)
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────┐
//! │                  crow_agent                      │
//! ├─────────────────────────────────────────────────┤
//! │  Frontend Layer                                  │
//! │  ┌──────────────┐  ┌──────────────────────┐    │
//! │  │  CLI/REPL    │  │  ACP Server (future) │    │
//! │  └──────────────┘  └──────────────────────┘    │
//! ├─────────────────────────────────────────────────┤
//! │  Agent Core (rig-based)                         │
//! │  ┌──────────────────────────────────────────┐  │
//! │  │  CrowAgent                                │  │
//! │  │  - Tool dispatch                          │  │
//! │  │  - Conversation management                │  │
//! │  │  - Telemetry logging                      │  │
//! │  └──────────────────────────────────────────┘  │
//! ├─────────────────────────────────────────────────┤
//! │  Tools Layer (rig::Tool implementations)        │
//! │  ┌────────┐ ┌────────┐ ┌────────┐ ┌────────┐  │
//! │  │read_   │ │terminal│ │edit_   │ │grep    │  │
//! │  │file    │ │        │ │file    │ │        │  │
//! │  └────────┘ └────────┘ └────────┘ └────────┘  │
//! │  ┌────────┐ ┌────────┐ ┌────────┐ ┌────────┐  │
//! │  │list_   │ │find_   │ │thinking│ │now     │  │
//! │  │dir     │ │path    │ │        │ │        │  │
//! │  └────────┘ └────────┘ └────────┘ └────────┘  │
//! └─────────────────────────────────────────────────┘
//! ```

pub mod acp;
pub mod auth;
pub mod config;
pub mod hooks;
pub mod lsp;
pub mod snapshot;
pub mod telemetry;
pub mod templates;
pub mod tools;
pub mod trace_layer;

// New architecture (replacing rig-based agent)
pub mod events;
pub mod provider;
pub mod tool;
pub mod tools2;

// Old rig-based agent (to be migrated)
#[path = "agent.rs"]
pub mod agent_old;

// New agent system
#[path = "agent/mod.rs"]
pub mod agent;

pub use acp::run_stdio_server;
pub use agent_old::CrowAgent; // Keep old export for now
pub use auth::AuthConfig;
pub use config::Config;
pub use telemetry::{Telemetry, TraceGuard};

// New exports
pub use agent::{ACPAgent, AgentConfig, BaseAgent, ControlFlow};
pub use events::{AgentEvent, AgentEventStream, TurnResult};
pub use provider::{ProviderClient, ProviderConfig, StreamDelta};
