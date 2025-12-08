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
pub mod agent;
pub mod auth;
pub mod config;
pub mod hooks;
pub mod lsp;
pub mod snapshot;
pub mod telemetry;
pub mod templates;
pub mod tools;
pub mod trace_layer;

pub use acp::run_stdio_server;
pub use agent::CrowAgent;
pub use auth::AuthConfig;
pub use config::Config;
pub use telemetry::Telemetry;
