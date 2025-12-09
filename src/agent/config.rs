//! Agent configuration types

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Configuration for an agent
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    /// Agent name (unique identifier)
    pub name: String,

    /// Description of what this agent does
    pub description: Option<String>,

    /// Where this agent can be used
    pub mode: AgentMode,

    /// Model override (uses provider default if None)
    pub model: Option<String>,

    /// Temperature (0.0 - 2.0)
    pub temperature: Option<f32>,

    /// Top-p sampling
    pub top_p: Option<f32>,

    /// Max tokens in response
    pub max_tokens: Option<u32>,

    /// Max ReAct iterations before giving up
    pub max_iterations: Option<usize>,

    /// Custom system prompt (overrides default)
    pub system_prompt: Option<String>,

    /// Which tools this agent can use
    pub tools: ToolPermissions,

    /// Is this a built-in agent?
    pub built_in: bool,

    /// Display color (hex)
    pub color: Option<String>,
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
            built_in: false,
            color: None,
        }
    }

    /// Check if a tool is enabled for this agent
    pub fn is_tool_enabled(&self, tool_name: &str) -> bool {
        self.tools.is_enabled(tool_name)
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
}

/// Tool permissions for an agent
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
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
