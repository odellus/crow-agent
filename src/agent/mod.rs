//! Agent system
//!
//! Two-layer architecture:
//! - `BaseAgent` (internal): ReAct loop, tool execution, emits events
//! - `ACPAgent` (external): Control flow orchestration, coagent handoff

mod base;
mod config;
mod control_flow;
mod fixtures;

pub use base::*;
pub use config::*;
pub use control_flow::*;
pub use fixtures::*;
