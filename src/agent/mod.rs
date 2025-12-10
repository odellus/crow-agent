//! Agent system
//!
//! Two-layer architecture:
//! - `BaseAgent` (internal): ReAct loop, tool execution, emits events
//! - `ACPAgent` (external): Control flow orchestration, coagent handoff
//!
//! Agent configuration:
//! - `AgentConfig`: Full agent configuration with permissions
//! - `AgentRegistry`: Loads agents from builtins and config files
//! - `AgentPermissions`: Bash patterns, edit permissions, etc.

mod base;
mod builtins;
mod config;
mod control_flow;
mod fixtures;
pub mod prompt;
mod registry;

pub use base::*;
pub use builtins::get_builtin_agents;
pub use config::*;
pub use control_flow::*;
pub use fixtures::*;
pub use prompt::{build_system_prompt, get_base_prompt};
pub use registry::AgentRegistry;
