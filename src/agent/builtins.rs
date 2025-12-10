//! Built-in agent definitions
//!
//! Built-in agents:
//! - `build`: Primary implementation agent (default)
//! - `plan`: Primary read-only planning agent
//! - `general`: Subagent for research and exploration

use super::config::{AgentConfig, AgentMode, AgentPermissions, ControlFlowConfig, ToolPermissions};
use std::collections::HashMap;

/// Get all built-in agents
pub fn get_builtin_agents() -> HashMap<String, AgentConfig> {
    let mut agents = HashMap::new();

    // Build agent - primary implementation agent (default)
    // Loop mode - keeps going until task_complete
    let build = AgentConfig::builtin("build")
        .with_description("Implementation agent for executing code and build tasks.")
        .with_mode(AgentMode::Primary)
        .with_control_flow(ControlFlowConfig::Loop);
    agents.insert("build".to_string(), build);

    // Plan agent - read-only analysis
    // Passthrough mode - returns to user after each turn
    let plan = AgentConfig::builtin("plan")
        .with_description("Planning and analysis agent with restricted permissions.")
        .with_mode(AgentMode::Primary)
        .with_permissions(AgentPermissions::read_only())
        .with_control_flow(ControlFlowConfig::Passthrough);
    agents.insert("plan".to_string(), plan);

    // General agent - for research and exploration (subagent only)
    // Loop mode - keeps going until task_complete
    let general = AgentConfig::builtin("general")
        .with_description(
            "General-purpose agent for researching complex questions, \
             searching for code, and executing multi-step tasks.",
        )
        .with_mode(AgentMode::Subagent)
        .with_control_flow(ControlFlowConfig::Loop)
        .with_tools(
            ToolPermissions::all_enabled()
                .disable("todoread")
                .disable("todowrite"),
        );
    agents.insert("general".to_string(), general);

    agents
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::config::Permission;

    #[test]
    fn test_builtin_agents_count() {
        let agents = get_builtin_agents();
        assert_eq!(agents.len(), 3); // build, plan, general
    }

    #[test]
    fn test_build_agent() {
        let agents = get_builtin_agents();
        let build = agents.get("build").unwrap();

        assert!(build.built_in);
        assert_eq!(build.mode, AgentMode::Primary);
        assert!(build.is_primary());
        assert_eq!(build.control_flow, ControlFlowConfig::Loop);
    }

    #[test]
    fn test_plan_agent() {
        let agents = get_builtin_agents();
        let plan = agents.get("plan").unwrap();

        assert!(plan.built_in);
        assert_eq!(plan.mode, AgentMode::Primary);
        assert_eq!(plan.permissions.edit, Permission::Deny);
        assert_eq!(plan.control_flow, ControlFlowConfig::Passthrough);

        // Read-only bash commands allowed
        assert_eq!(plan.permissions.check_bash("ls -la"), Permission::Allow);
        assert_eq!(plan.permissions.check_bash("git status"), Permission::Allow);

        // Write commands need ask
        assert_eq!(plan.permissions.check_bash("cargo build"), Permission::Ask);
    }

    #[test]
    fn test_general_agent() {
        let agents = get_builtin_agents();
        let general = agents.get("general").unwrap();

        assert!(general.built_in);
        assert_eq!(general.mode, AgentMode::Subagent);
        assert_eq!(general.control_flow, ControlFlowConfig::Loop);
        assert!(!general.is_tool_enabled("todowrite"));
        assert!(!general.is_tool_enabled("todoread"));
        assert!(general.is_tool_enabled("bash"));
    }
}
