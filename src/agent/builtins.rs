//! Built-in agent definitions
//!
//! Ported from crow-old, matching OpenCode's agent system.
//!
//! Built-in agents:
//! - `build`: Primary implementation agent (default)
//! - `plan`: Primary read-only planning agent
//! - `general`: Subagent for research and exploration
//! - `executor`: Subagent for dual-agent verified tasks
//! - `arbiter`: Subagent that verifies executor's work
//! - `planner`: Primary agent in dual-agent mode
//! - `architect`: Primary verifier in dual-agent mode

use super::config::{AgentConfig, AgentMode, AgentPermissions, ToolPermissions};
use std::collections::HashMap;

/// Get all built-in agents
pub fn get_builtin_agents() -> HashMap<String, AgentConfig> {
    let mut agents = HashMap::new();

    // General agent - for research and exploration
    let general = AgentConfig::builtin("general")
        .with_description(
            "General-purpose agent for researching complex questions, \
             searching for code, and executing multi-step tasks.",
        )
        .with_mode(AgentMode::Subagent)
        .with_tools(
            ToolPermissions::all_enabled()
                .disable("todoread")
                .disable("todowrite"),
        );
    agents.insert("general".to_string(), general);

    // Build agent - for implementation (matches OpenCode)
    let build = AgentConfig::builtin("build")
        .with_description("Implementation agent for executing code and build tasks.")
        .with_mode(AgentMode::Primary)
        .disable_tool("task_complete"); // Only arbiter should use this
    agents.insert("build".to_string(), build);

    // Plan agent - read-only analysis (matches OpenCode)
    let plan = AgentConfig::builtin("plan")
        .with_description("Planning and analysis agent with restricted permissions.")
        .with_mode(AgentMode::Primary)
        .with_permissions(AgentPermissions::read_only());
    agents.insert("plan".to_string(), plan);

    // Executor agent - implementation agent for dual-agent system
    let executor = AgentConfig::builtin("executor")
        .with_description("Executor agent for dual-agent verified tasks. Does the implementation work.")
        .with_mode(AgentMode::Subagent)
        .with_color("#3B82F6") // Blue
        .with_tools(
            ToolPermissions::all_enabled()
                .disable("task") // Prevent infinite subagent nesting
                .disable("task_complete"), // Only arbiter can complete
        );
    agents.insert("executor".to_string(), executor);

    // Arbiter agent - verification agent for dual-agent system
    let arbiter = AgentConfig::builtin("arbiter")
        .with_description("Verification agent that reviews work and calls task_complete when satisfied.")
        .with_mode(AgentMode::Subagent)
        .with_temperature(0.3) // Lower temperature for more deterministic verification
        .with_color("#10B981") // Green
        .with_prompt(ARBITER_PROMPT)
        .with_tools(
            ToolPermissions::all_enabled()
                .disable("task") // Prevent infinite subagent nesting
                .enable("task_complete"), // Only arbiter can complete
        );
    agents.insert("arbiter".to_string(), arbiter);

    // Planner agent - primary agent for dual-agent mode
    let planner = AgentConfig::builtin("planner")
        .with_description("Primary planning agent that executes tasks. Works with Architect in dual-agent mode.")
        .with_mode(AgentMode::Primary)
        .with_color("#3B82F6") // Blue
        .with_tools(
            ToolPermissions::all_enabled()
                .disable("task") // No subagents for primary dual mode
                .disable("task_complete"), // Only architect can complete
        );
    agents.insert("planner".to_string(), planner);

    // Architect agent - verifier for primary dual-agent mode
    let architect = AgentConfig::builtin("architect")
        .with_description("Verification agent that reviews Planner's work. Calls task_complete when satisfied.")
        .with_mode(AgentMode::Primary)
        .with_temperature(0.3) // Lower temperature for more deterministic verification
        .with_color("#10B981") // Green
        .with_prompt(ARCHITECT_PROMPT)
        .with_tools(
            ToolPermissions::all_enabled()
                .disable("task") // No subagents for primary dual mode
                .enable("task_complete"), // Only architect can complete
        );
    agents.insert("architect".to_string(), architect);

    agents
}

/// System prompt for the architect agent in primary dual-agent mode
const ARCHITECT_PROMPT: &str = r#"You are the Architect agent in a dual-agent system.

You review the Planner's work. Either call task_complete OR provide feedback. Nothing else.

## CRITICAL RULES

1. When you call `task_complete`, STOP IMMEDIATELY. Do not generate any more text or tool calls after it.
2. You get ONE response per turn. Make your decision and execute it.
3. Do NOT explain what you're going to do. Just do it.

## Your job

Verify the Planner's work:
- Read the code/files if needed
- Run tests if applicable (cargo test, npm test, etc.)
- Check if requirements are met

## Decision

**If work is complete and correct:** Call `task_complete` with summary and verification. STOP.

**If work has issues:** Provide brief, specific feedback. The Planner will fix it.

## Example good responses

GOOD (complete):
```
<calls task_complete with summary>
```

GOOD (needs work):
```
Tests fail: `cargo test` shows 2 failures in auth module. Fix the token validation.
```

BAD:
```
Let me verify the work... I'll check the files... Now I'll run tests... The task is complete, let me call task_complete... <calls task_complete> Great, I've verified everything!
```

Be terse. One action per turn.
"#;

/// System prompt for the arbiter agent in subagent dual-agent mode
const ARBITER_PROMPT: &str = r#"You are the Arbiter agent in a dual-agent verification system.

You receive the Executor's full session showing everything it did - all thinking, tool calls, and outputs.

Your job is to VERIFY the work:

1. **Read carefully** - Understand what the Executor did and why
2. **Run tests** - Execute test commands (cargo test, npm test, pytest, etc.)
3. **Check the code** - Read modified files to verify correctness
4. **Verify requirements** - Ensure the original task requirements are met

## When to call task_complete

Call `task_complete` with summary and verification when ALL of these are true:
- Tests pass (if applicable)
- Code compiles/runs without errors
- The original requirements are satisfied
- No obvious bugs or issues

## When NOT to call task_complete

If there are problems:
- Explain clearly what's wrong and why
- Provide specific feedback the Executor can act on
- Your full response will be sent back to the Executor

## Important

- You have ALL the same tools as the Executor (bash, read, edit, etc.)
- Actually run tests - don't just assume they pass
- Be thorough but efficient - verify the critical paths
- If you find issues, be constructive in your feedback
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::config::Permission;

    #[test]
    fn test_builtin_agents_count() {
        let agents = get_builtin_agents();
        assert_eq!(agents.len(), 7); // general, build, plan, executor, arbiter, planner, architect
    }

    #[test]
    fn test_general_agent() {
        let agents = get_builtin_agents();
        let general = agents.get("general").unwrap();

        assert!(general.built_in);
        assert_eq!(general.mode, AgentMode::Subagent);
        assert!(!general.is_tool_enabled("todowrite"));
        assert!(!general.is_tool_enabled("todoread"));
        assert!(general.is_tool_enabled("bash")); // Other tools enabled
    }

    #[test]
    fn test_build_agent() {
        let agents = get_builtin_agents();
        let build = agents.get("build").unwrap();

        assert!(build.built_in);
        assert_eq!(build.mode, AgentMode::Primary);
        assert!(build.is_primary());
        assert!(!build.is_tool_enabled("task_complete"));
    }

    #[test]
    fn test_plan_agent_permissions() {
        let agents = get_builtin_agents();
        let plan = agents.get("plan").unwrap();

        assert_eq!(plan.permissions.edit, Permission::Deny);
        assert_eq!(plan.mode, AgentMode::Primary);

        // Read-only bash commands allowed
        assert_eq!(plan.permissions.check_bash("ls -la"), Permission::Allow);
        assert_eq!(plan.permissions.check_bash("git status"), Permission::Allow);

        // Write commands need ask
        assert_eq!(plan.permissions.check_bash("cargo build"), Permission::Ask);
    }

    #[test]
    fn test_executor_agent() {
        let agents = get_builtin_agents();
        let executor = agents.get("executor").unwrap();

        assert!(executor.built_in);
        assert_eq!(executor.mode, AgentMode::Subagent);
        // Executor cannot spawn subagents or complete tasks
        assert!(!executor.is_tool_enabled("task"));
        assert!(!executor.is_tool_enabled("task_complete"));
        // But can use todos (shared with arbiter)
        assert!(executor.is_tool_enabled("todowrite"));
        assert!(executor.is_tool_enabled("todoread"));
    }

    #[test]
    fn test_arbiter_agent() {
        let agents = get_builtin_agents();
        let arbiter = agents.get("arbiter").unwrap();

        assert!(arbiter.built_in);
        assert_eq!(arbiter.mode, AgentMode::Subagent);
        // Arbiter cannot spawn subagents but CAN complete tasks
        assert!(!arbiter.is_tool_enabled("task"));
        assert!(arbiter.is_tool_enabled("task_complete"));
        // Has custom prompt
        assert!(arbiter.system_prompt.is_some());
        assert_eq!(arbiter.temperature, Some(0.3));
    }

    #[test]
    fn test_planner_agent() {
        let agents = get_builtin_agents();
        let planner = agents.get("planner").unwrap();

        assert!(planner.built_in);
        assert_eq!(planner.mode, AgentMode::Primary);
        // Planner cannot spawn subagents or complete tasks
        assert!(!planner.is_tool_enabled("task"));
        assert!(!planner.is_tool_enabled("task_complete"));
    }

    #[test]
    fn test_architect_agent() {
        let agents = get_builtin_agents();
        let architect = agents.get("architect").unwrap();

        assert!(architect.built_in);
        assert_eq!(architect.mode, AgentMode::Primary);
        // Architect cannot spawn subagents but CAN complete tasks
        assert!(!architect.is_tool_enabled("task"));
        assert!(architect.is_tool_enabled("task_complete"));
        assert!(architect.system_prompt.is_some());
        assert_eq!(architect.temperature, Some(0.3));
    }
}
