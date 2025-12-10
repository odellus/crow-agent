//! Agent configuration types
//!
//! Full permissions model ported from crow-old, supporting:
//! - Tool enable/disable
//! - Bash command patterns (wildcards)
//! - Edit permissions
//! - Web fetch permissions
//! - Doom loop detection override

use crate::agent::ControlFlow;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Permission level for tool execution
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Permission {
    /// Always allow this action
    Allow,
    /// Always deny this action
    Deny,
    /// Ask user for permission before executing
    #[default]
    Ask,
}

/// Agent permissions configuration
///
/// Controls what actions an agent can take. Supports:
/// - File editing (allow/deny/ask)
/// - Bash command patterns with wildcards
/// - Web fetching
/// - Doom loop override
/// - External directory access
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AgentPermissions {
    /// Permission for file editing
    #[serde(default = "default_allow")]
    pub edit: Permission,

    /// Bash command permissions (pattern -> permission)
    /// Patterns support wildcards: "git *" matches all git commands
    /// Patterns are matched in order; first match wins
    /// Use "*" as catch-all
    #[serde(default = "default_bash_permissions")]
    pub bash: HashMap<String, Permission>,

    /// Permission for web fetching
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub webfetch: Option<Permission>,

    /// Permission for doom loop detection override
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub doom_loop: Option<Permission>,

    /// Permission for accessing directories outside project
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_directory: Option<Permission>,
}

fn default_allow() -> Permission {
    Permission::Allow
}

fn default_bash_permissions() -> HashMap<String, Permission> {
    let mut bash = HashMap::new();
    bash.insert("*".to_string(), Permission::Allow);
    bash
}

impl Default for AgentPermissions {
    fn default() -> Self {
        Self {
            edit: Permission::Allow,
            bash: default_bash_permissions(),
            webfetch: Some(Permission::Allow),
            doom_loop: Some(Permission::Ask),
            external_directory: Some(Permission::Ask),
        }
    }
}

impl AgentPermissions {
    /// Create permissions that allow everything
    pub fn allow_all() -> Self {
        Self::default()
    }

    /// Create read-only permissions (Plan agent style)
    ///
    /// - Denies file editing
    /// - Allows read-only bash commands
    /// - Asks for anything else
    pub fn read_only() -> Self {
        let mut bash = HashMap::new();

        // Read-only commands (matches OpenCode exactly)
        bash.insert("cut*".to_string(), Permission::Allow);
        bash.insert("diff*".to_string(), Permission::Allow);
        bash.insert("du*".to_string(), Permission::Allow);
        bash.insert("file *".to_string(), Permission::Allow);

        // Find with dangerous options requires ask
        bash.insert("find * -delete*".to_string(), Permission::Ask);
        bash.insert("find * -exec*".to_string(), Permission::Ask);
        bash.insert("find * -fprint*".to_string(), Permission::Ask);
        bash.insert("find * -fls*".to_string(), Permission::Ask);
        bash.insert("find * -fprintf*".to_string(), Permission::Ask);
        bash.insert("find * -ok*".to_string(), Permission::Ask);
        bash.insert("find *".to_string(), Permission::Allow);

        // Git read-only
        bash.insert("git diff*".to_string(), Permission::Allow);
        bash.insert("git log*".to_string(), Permission::Allow);
        bash.insert("git show*".to_string(), Permission::Allow);
        bash.insert("git status*".to_string(), Permission::Allow);
        bash.insert("git branch".to_string(), Permission::Allow);
        bash.insert("git branch -v".to_string(), Permission::Allow);

        // Text processing
        bash.insert("grep*".to_string(), Permission::Allow);
        bash.insert("head*".to_string(), Permission::Allow);
        bash.insert("less*".to_string(), Permission::Allow);
        bash.insert("ls*".to_string(), Permission::Allow);
        bash.insert("more*".to_string(), Permission::Allow);
        bash.insert("pwd*".to_string(), Permission::Allow);
        bash.insert("rg*".to_string(), Permission::Allow);

        // Sort with output redirection requires ask
        bash.insert("sort --output=*".to_string(), Permission::Ask);
        bash.insert("sort -o *".to_string(), Permission::Ask);
        bash.insert("sort*".to_string(), Permission::Allow);

        bash.insert("stat*".to_string(), Permission::Allow);
        bash.insert("tail*".to_string(), Permission::Allow);

        // Tree with output redirection requires ask
        bash.insert("tree -o *".to_string(), Permission::Ask);
        bash.insert("tree*".to_string(), Permission::Allow);

        bash.insert("uniq*".to_string(), Permission::Allow);
        bash.insert("wc*".to_string(), Permission::Allow);
        bash.insert("whereis*".to_string(), Permission::Allow);
        bash.insert("which*".to_string(), Permission::Allow);

        // Ask for anything else (default catch-all)
        bash.insert("*".to_string(), Permission::Ask);

        Self {
            edit: Permission::Deny,
            bash,
            webfetch: Some(Permission::Allow),
            doom_loop: Some(Permission::Ask),
            external_directory: Some(Permission::Ask),
        }
    }

    /// Check permission for a bash command
    ///
    /// Matches patterns using glob-style matching:
    /// - `*` matches any sequence of characters
    /// - More specific patterns (longer) are checked first
    /// - Falls back to `*` catch-all
    pub fn check_bash(&self, command: &str) -> Permission {
        // Sort patterns by specificity (longer = more specific)
        let mut patterns: Vec<_> = self.bash.iter().collect();
        patterns.sort_by(|a, b| b.0.len().cmp(&a.0.len()));

        for (pattern, permission) in patterns {
            if Self::glob_match(pattern, command) {
                return *permission;
            }
        }

        // Default: ask
        Permission::Ask
    }

    /// Simple glob matching with `*` wildcards
    fn glob_match(pattern: &str, text: &str) -> bool {
        if pattern == "*" {
            return true;
        }

        // Split pattern by * and match each part
        let parts: Vec<&str> = pattern.split('*').collect();

        if parts.len() == 1 {
            // No wildcards - exact match
            return pattern == text;
        }

        let mut pos = 0;

        // First part must match at start (if non-empty)
        if !parts[0].is_empty() {
            if !text.starts_with(parts[0]) {
                return false;
            }
            pos = parts[0].len();
        }

        // Middle parts can match anywhere after current position
        for part in &parts[1..parts.len() - 1] {
            if part.is_empty() {
                continue;
            }
            if let Some(idx) = text[pos..].find(part) {
                pos += idx + part.len();
            } else {
                return false;
            }
        }

        // Last part must match at end (if non-empty)
        let last = parts[parts.len() - 1];
        if !last.is_empty() {
            if !text[pos..].ends_with(last) {
                return false;
            }
        }

        true
    }
}

/// Configuration for an agent
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    /// Agent name (unique identifier)
    /// When loading from YAML files, this is set from the filename.
    #[serde(default)]
    pub name: String,

    /// Description of what this agent does
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Where this agent can be used
    #[serde(default)]
    pub mode: AgentMode,

    /// Model override (uses provider default if None)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,

    /// Temperature (0.0 - 2.0)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,

    /// Top-p sampling
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,

    /// Max tokens in response
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,

    /// Max ReAct iterations before giving up
    #[serde(default = "default_max_iterations", skip_serializing_if = "Option::is_none")]
    pub max_iterations: Option<usize>,

    /// Custom system prompt (overrides default)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,

    /// Which tools this agent can use (enable/disable by name)
    #[serde(default)]
    pub tools: ToolPermissions,

    /// Full permission configuration (bash patterns, edit, etc.)
    #[serde(default)]
    pub permissions: AgentPermissions,

    /// Is this a built-in agent?
    #[serde(default)]
    pub built_in: bool,

    /// Display color (hex)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,

    /// Control flow mode - what happens after each ReAct turn
    /// - passthrough: Return to user after each turn (HITL)
    /// - loop: Keep going until task_complete (default)
    /// - static: Inject canned message between turns
    /// - generated: Generate acceptance criteria once, inject each turn
    /// - coagent: Dual-agent system with arbiter oversight
    #[serde(default)]
    pub control_flow: ControlFlowConfig,

    /// Static message to inject between turns (for control_flow: static)
    /// Can be inline string or path to prompt file
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub static_message: Option<String>,

    /// Prompt for generating acceptance criteria (for control_flow: generated)
    /// Can be inline string or path to prompt file
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generate_prompt: Option<String>,

    /// Coagent config name (required when control_flow is coagent)
    /// Specifies which coagent agent to use for oversight
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coagent: Option<String>,
}

/// Control flow configuration for YAML
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ControlFlowConfig {
    /// Return to user after each turn (HITL)
    Passthrough,
    /// Keep going until task_complete
    #[default]
    Loop,
    /// Inject canned message between turns
    Static,
    /// Generate acceptance criteria once, inject each turn
    Generated,
    /// Dual-agent with coagent oversight
    Coagent,
}

fn default_max_iterations() -> Option<usize> {
    Some(50)
}

impl AgentConfig {
    /// Create a new agent config with defaults
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: None,
            mode: AgentMode::Primary,
            model: None,
            temperature: None,
            top_p: None,
            max_tokens: None,
            max_iterations: Some(50),
            system_prompt: None,
            tools: ToolPermissions::default(),
            permissions: AgentPermissions::default(),
            built_in: false,
            color: None,
            control_flow: ControlFlowConfig::Loop,
            static_message: None,
            generate_prompt: None,
            coagent: None,
        }
    }

    /// Create a new built-in agent
    pub fn builtin(name: impl Into<String>) -> Self {
        let mut config = Self::new(name);
        config.built_in = true;
        config
    }

    /// Check if a tool is enabled for this agent
    pub fn is_tool_enabled(&self, tool_name: &str) -> bool {
        self.tools.is_enabled(tool_name)
    }

    /// Check if this agent can be used as primary
    pub fn is_primary(&self) -> bool {
        self.mode.is_primary()
    }

    /// Check if this agent can be used as subagent
    pub fn is_subagent(&self) -> bool {
        self.mode.is_subagent()
    }

    /// Get effective temperature (with default fallback)
    pub fn get_temperature(&self) -> f32 {
        self.temperature.unwrap_or(0.7)
    }

    /// Get effective top_p (with default fallback)
    pub fn get_top_p(&self) -> f32 {
        self.top_p.unwrap_or(1.0)
    }

    /// Get control flow for runtime
    pub fn get_control_flow(&self) -> ControlFlow {
        match self.control_flow {
            ControlFlowConfig::Passthrough => ControlFlow::Passthrough,
            ControlFlowConfig::Loop => ControlFlow::Loop,
            ControlFlowConfig::Static => ControlFlow::Static {
                message: self.static_message.clone()
                    .unwrap_or_else(|| "Continue with the task. Call task_complete when done.".into()),
            },
            ControlFlowConfig::Generated => ControlFlow::Generated {
                generator_prompt: self.generate_prompt.clone()
                    .unwrap_or_else(|| "Based on the conversation so far, what are the acceptance criteria for this task? Be specific and concise.".into()),
                acceptance_criteria: None,
            },
            ControlFlowConfig::Coagent => ControlFlow::Coagent,
        }
    }

    /// Does this agent use a coagent?
    pub fn has_coagent(&self) -> bool {
        self.control_flow == ControlFlowConfig::Coagent
    }

    /// Get coagent name (defaults to "judge")
    pub fn coagent_name(&self) -> &str {
        self.coagent.as_deref().unwrap_or("judge")
    }

    /// Builder: set description
    pub fn with_description(mut self, desc: impl Into<String>) -> Self {
        self.description = Some(desc.into());
        self
    }

    /// Builder: set mode
    pub fn with_mode(mut self, mode: AgentMode) -> Self {
        self.mode = mode;
        self
    }

    /// Builder: set temperature
    pub fn with_temperature(mut self, temp: f32) -> Self {
        self.temperature = Some(temp);
        self
    }

    /// Builder: set color
    pub fn with_color(mut self, color: impl Into<String>) -> Self {
        self.color = Some(color.into());
        self
    }

    /// Builder: set system prompt
    pub fn with_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt = Some(prompt.into());
        self
    }

    /// Builder: set permissions
    pub fn with_permissions(mut self, permissions: AgentPermissions) -> Self {
        self.permissions = permissions;
        self
    }

    /// Builder: set control flow
    pub fn with_control_flow(mut self, control_flow: ControlFlowConfig) -> Self {
        self.control_flow = control_flow;
        self
    }

    /// Builder: set coagent name
    pub fn with_coagent(mut self, coagent: impl Into<String>) -> Self {
        self.coagent = Some(coagent.into());
        self
    }

    /// Builder: set tool permissions
    pub fn with_tools(mut self, tools: ToolPermissions) -> Self {
        self.tools = tools;
        self
    }

    /// Builder: disable a specific tool
    pub fn disable_tool(mut self, tool: impl Into<String>) -> Self {
        self.tools = self.tools.disable(tool);
        self
    }

    /// Builder: enable a specific tool
    pub fn enable_tool(mut self, tool: impl Into<String>) -> Self {
        self.tools = self.tools.enable(tool);
        self
    }
}

/// Where an agent can be used
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum AgentMode {
    /// Can be selected by user as primary agent
    #[default]
    Primary,
    /// Only usable as subagent (spawned by Task tool)
    Subagent,
    /// Only usable as coagent (dual-agent orchestration, autonomy levels 3-5)
    Coagent,
    /// Both primary and subagent
    All,
}

impl AgentMode {
    pub fn is_primary(&self) -> bool {
        matches!(self, AgentMode::Primary | AgentMode::All)
    }

    pub fn is_subagent(&self) -> bool {
        matches!(self, AgentMode::Subagent | AgentMode::All)
    }

    pub fn is_coagent(&self) -> bool {
        matches!(self, AgentMode::Coagent)
    }
}

/// Tool permissions for an agent
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolPermissions {
    /// Explicit tool enable/disable overrides
    /// true = enabled, false = disabled
    /// Tools not listed use the default (enabled)
    #[serde(default)]
    pub overrides: HashMap<String, bool>,

    /// Default for tools not explicitly listed
    #[serde(default = "default_true")]
    pub default_enabled: bool,
}

fn default_true() -> bool {
    true
}

impl Default for ToolPermissions {
    fn default() -> Self {
        Self {
            overrides: HashMap::new(),
            default_enabled: true,
        }
    }
}

impl ToolPermissions {
    /// Create permissions with all tools enabled
    pub fn all_enabled() -> Self {
        Self {
            overrides: HashMap::new(),
            default_enabled: true,
        }
    }

    /// Create permissions with all tools disabled
    pub fn all_disabled() -> Self {
        Self {
            overrides: HashMap::new(),
            default_enabled: false,
        }
    }

    /// Enable a specific tool
    pub fn enable(mut self, tool: impl Into<String>) -> Self {
        self.overrides.insert(tool.into(), true);
        self
    }

    /// Disable a specific tool
    pub fn disable(mut self, tool: impl Into<String>) -> Self {
        self.overrides.insert(tool.into(), false);
        self
    }

    /// Check if a tool is enabled
    pub fn is_enabled(&self, tool_name: &str) -> bool {
        self.overrides
            .get(tool_name)
            .copied()
            .unwrap_or(self.default_enabled)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_permission_default() {
        assert_eq!(Permission::default(), Permission::Ask);
    }

    #[test]
    fn test_agent_permissions_default() {
        let perms = AgentPermissions::default();
        assert_eq!(perms.edit, Permission::Allow);
        assert_eq!(perms.check_bash("anything"), Permission::Allow);
    }

    #[test]
    fn test_agent_permissions_read_only() {
        let perms = AgentPermissions::read_only();
        assert_eq!(perms.edit, Permission::Deny);

        // Allowed commands
        assert_eq!(perms.check_bash("ls -la"), Permission::Allow);
        assert_eq!(perms.check_bash("grep foo"), Permission::Allow);
        assert_eq!(perms.check_bash("git status"), Permission::Allow);
        assert_eq!(perms.check_bash("find . -name foo"), Permission::Allow);

        // Dangerous find commands need ask
        assert_eq!(perms.check_bash("find . -delete"), Permission::Ask);
        assert_eq!(perms.check_bash("find . -exec rm {} \\;"), Permission::Ask);

        // Unknown commands need ask
        assert_eq!(perms.check_bash("rm -rf /"), Permission::Ask);
        assert_eq!(perms.check_bash("cargo build"), Permission::Ask);
    }

    #[test]
    fn test_bash_pattern_matching() {
        let mut perms = AgentPermissions::default();
        perms.bash.clear();
        perms.bash.insert("git *".to_string(), Permission::Allow);
        perms.bash.insert("git push*".to_string(), Permission::Ask);
        perms.bash.insert("*".to_string(), Permission::Deny);

        // More specific pattern wins
        assert_eq!(perms.check_bash("git push origin main"), Permission::Ask);
        assert_eq!(perms.check_bash("git status"), Permission::Allow);
        assert_eq!(perms.check_bash("rm -rf"), Permission::Deny);
    }

    #[test]
    fn test_agent_config_creation() {
        let config = AgentConfig::new("test");
        assert_eq!(config.name, "test");
        assert!(!config.built_in);
        assert_eq!(config.mode, AgentMode::Primary);
    }

    #[test]
    fn test_agent_config_builder() {
        let config = AgentConfig::builtin("build")
            .with_description("Build agent")
            .with_mode(AgentMode::Primary)
            .with_temperature(0.3)
            .with_color("#3B82F6")
            .disable_tool("task_complete");

        assert!(config.built_in);
        assert_eq!(config.description, Some("Build agent".to_string()));
        assert_eq!(config.temperature, Some(0.3));
        assert!(!config.is_tool_enabled("task_complete"));
        assert!(config.is_tool_enabled("bash")); // default enabled
    }

    #[test]
    fn test_agent_modes() {
        assert!(AgentMode::Primary.is_primary());
        assert!(!AgentMode::Primary.is_subagent());

        assert!(!AgentMode::Subagent.is_primary());
        assert!(AgentMode::Subagent.is_subagent());

        assert!(AgentMode::All.is_primary());
        assert!(AgentMode::All.is_subagent());
    }

    #[test]
    fn test_tool_permissions() {
        let tools = ToolPermissions::all_enabled()
            .disable("task")
            .disable("task_complete");

        assert!(!tools.is_enabled("task"));
        assert!(!tools.is_enabled("task_complete"));
        assert!(tools.is_enabled("bash"));
        assert!(tools.is_enabled("edit"));
    }
}
