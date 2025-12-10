//! Agent system
//!
//! Two-layer architecture:
//! - `BaseAgent` (internal): ReAct loop, tool execution, emits events
//! - `Agent` (external): Control flow orchestration, coagent handoff
//!
//! Agent configuration:
//! - `AgentConfig`: Full agent configuration with permissions
//! - `AgentRegistry`: Loads agents from builtins and config files
//! - `AgentPermissions`: Bash patterns, edit permissions, etc.
//!
//! Config loading:
//! - Global: ~/.config/crow/agents/*.yaml
//! - Project: .crow/agents/*.yaml

mod base;
mod builtins;
mod config;
mod config_loader;
mod control_flow;
mod fixtures;
pub mod prompt;
mod registry;

pub use base::*;
pub use builtins::get_builtin_agents;
pub use config::*;
pub use config_loader::{load_agent_configs, save_config_file, project_config_path, global_config_path};
pub use control_flow::*;
pub use fixtures::*;
pub use prompt::{build_system_prompt, get_base_prompt};
pub use registry::AgentRegistry;
