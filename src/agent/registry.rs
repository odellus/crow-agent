//! Agent registry for managing and loading agents
//!
//! Loads agents from:
//! 1. Built-in agents (general, build, plan, executor, arbiter, planner, architect)
//! 2. Project config: `.crow/agent/*.md` files
//! 3. Global config: `~/.config/crow/agent/*.md` files

use super::builtins::get_builtin_agents;
use super::config::{AgentConfig, AgentMode, AgentPermissions, Permission, ToolPermissions};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, warn};

/// Agent registry - manages all available agents
pub struct AgentRegistry {
    agents: Arc<RwLock<HashMap<String, AgentConfig>>>,
}

impl AgentRegistry {
    /// Create a new agent registry with built-in agents only
    pub fn new() -> Self {
        let agents = get_builtin_agents();
        Self {
            agents: Arc::new(RwLock::new(agents)),
        }
    }

    /// Create registry and load agents from config directories
    pub async fn new_with_config(working_dir: &Path) -> Self {
        let mut agents = get_builtin_agents();

        // Load from global config: ~/.config/crow/agent/*.md
        if let Some(config_dir) = dirs::config_dir() {
            let global_agent_dir = config_dir.join("crow").join("agent");
            if let Ok(loaded) = Self::load_agents_from_dir(&global_agent_dir).await {
                for agent in loaded {
                    debug!("Loaded global agent: {}", agent.name);
                    agents.insert(agent.name.clone(), agent);
                }
            }
        }

        // Load from project config: .crow/agent/*.md (higher priority)
        let project_agent_dir = working_dir.join(".crow").join("agent");
        if let Ok(loaded) = Self::load_agents_from_dir(&project_agent_dir).await {
            for agent in loaded {
                debug!("Loaded project agent: {}", agent.name);
                agents.insert(agent.name.clone(), agent);
            }
        }

        Self {
            agents: Arc::new(RwLock::new(agents)),
        }
    }

    /// Load agents from markdown files in a directory
    async fn load_agents_from_dir(dir: &Path) -> Result<Vec<AgentConfig>, String> {
        let mut agents = Vec::new();

        if !dir.exists() {
            return Ok(agents);
        }

        let entries =
            std::fs::read_dir(dir).map_err(|e| format!("Failed to read agent directory: {}", e))?;

        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().map(|e| e == "md").unwrap_or(false) {
                match Self::load_agent_from_file(&path).await {
                    Ok(agent) => agents.push(agent),
                    Err(e) => warn!("Failed to load agent from {:?}: {}", path, e),
                }
            }
        }

        Ok(agents)
    }

    /// Load a single agent from a markdown file
    ///
    /// Format:
    /// ```markdown
    /// ---
    /// description: When to use this agent
    /// mode: subagent | primary | all
    /// temperature: 0.7
    /// top_p: 1.0
    /// tools:
    ///   bash: true
    ///   edit: false
    /// permission:
    ///   edit: allow | deny | ask
    ///   bash:
    ///     "*": allow
    ///     "rm *": deny
    /// ---
    ///
    /// Custom system prompt content here
    /// ```
    async fn load_agent_from_file(path: &Path) -> Result<AgentConfig, String> {
        let content = tokio::fs::read_to_string(path)
            .await
            .map_err(|e| format!("Failed to read file: {}", e))?;

        // Extract agent name from filename (without .md extension)
        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or("Invalid filename")?
            .to_string();

        // Parse frontmatter and content
        let (frontmatter, prompt) = Self::parse_markdown(&content)?;

        // Build agent from frontmatter
        let mut agent = AgentConfig::new(&name);

        if let Some(desc) = frontmatter.get("description").and_then(|v| v.as_str()) {
            agent.description = Some(desc.to_string());
        }

        if let Some(mode) = frontmatter.get("mode").and_then(|v| v.as_str()) {
            agent.mode = match mode {
                "subagent" => AgentMode::Subagent,
                "primary" => AgentMode::Primary,
                _ => AgentMode::All,
            };
        }

        if let Some(temp) = frontmatter.get("temperature").and_then(|v| v.as_f64()) {
            agent.temperature = Some(temp as f32);
        }

        if let Some(top_p) = frontmatter.get("top_p").and_then(|v| v.as_f64()) {
            agent.top_p = Some(top_p as f32);
        }

        if let Some(color) = frontmatter.get("color").and_then(|v| v.as_str()) {
            agent.color = Some(color.to_string());
        }

        if let Some(model) = frontmatter.get("model").and_then(|v| v.as_str()) {
            agent.model = Some(model.to_string());
        }

        if let Some(max_iter) = frontmatter.get("max_iterations").and_then(|v| v.as_u64()) {
            agent.max_iterations = Some(max_iter as usize);
        }

        // Parse tools map
        if let Some(tools) = frontmatter.get("tools").and_then(|v| v.as_object()) {
            let mut tool_perms = ToolPermissions::all_enabled();
            for (tool_name, enabled) in tools {
                if let Some(enabled) = enabled.as_bool() {
                    if enabled {
                        tool_perms = tool_perms.enable(tool_name.clone());
                    } else {
                        tool_perms = tool_perms.disable(tool_name.clone());
                    }
                }
            }
            agent.tools = tool_perms;
        }

        // Parse permissions
        if let Some(perms) = frontmatter.get("permission").and_then(|v| v.as_object()) {
            agent.permissions = Self::parse_permissions(perms);
        }

        // Set custom prompt if content exists (after frontmatter)
        if !prompt.trim().is_empty() {
            agent.system_prompt = Some(prompt);
        }

        Ok(agent)
    }

    /// Parse markdown with YAML frontmatter
    fn parse_markdown(
        content: &str,
    ) -> Result<(serde_json::Map<String, serde_json::Value>, String), String> {
        let content = content.trim();

        if !content.starts_with("---") {
            // No frontmatter, entire content is prompt
            return Ok((serde_json::Map::new(), content.to_string()));
        }

        // Find end of frontmatter
        let rest = &content[3..];
        let end = rest.find("---").ok_or("Unclosed frontmatter")?;
        let yaml_content = rest[..end].trim();
        let prompt = rest[end + 3..].trim().to_string();

        // Parse YAML as JSON (serde_yaml -> serde_json)
        let yaml_value: serde_yaml::Value = serde_yaml::from_str(yaml_content)
            .map_err(|e| format!("Invalid YAML frontmatter: {}", e))?;

        let json_value: serde_json::Value = serde_json::to_value(yaml_value)
            .map_err(|e| format!("Failed to convert YAML to JSON: {}", e))?;

        let map = json_value
            .as_object()
            .ok_or("Frontmatter must be a YAML object")?
            .clone();

        Ok((map, prompt))
    }

    /// Parse permissions from frontmatter
    fn parse_permissions(perms: &serde_json::Map<String, serde_json::Value>) -> AgentPermissions {
        let mut result = AgentPermissions::default();

        if let Some(edit) = perms.get("edit").and_then(|v| v.as_str()) {
            result.edit = Self::parse_permission(edit);
        }

        if let Some(bash) = perms.get("bash") {
            if let Some(bash_str) = bash.as_str() {
                // Simple permission for all bash
                result.bash.clear();
                result
                    .bash
                    .insert("*".to_string(), Self::parse_permission(bash_str));
            } else if let Some(bash_map) = bash.as_object() {
                // Pattern-based permissions
                result.bash.clear();
                for (pattern, perm) in bash_map {
                    if let Some(perm_str) = perm.as_str() {
                        result
                            .bash
                            .insert(pattern.clone(), Self::parse_permission(perm_str));
                    }
                }
            }
        }

        if let Some(webfetch) = perms.get("webfetch").and_then(|v| v.as_str()) {
            result.webfetch = Some(Self::parse_permission(webfetch));
        }

        if let Some(doom_loop) = perms.get("doom_loop").and_then(|v| v.as_str()) {
            result.doom_loop = Some(Self::parse_permission(doom_loop));
        }

        if let Some(external) = perms.get("external_directory").and_then(|v| v.as_str()) {
            result.external_directory = Some(Self::parse_permission(external));
        }

        result
    }

    fn parse_permission(s: &str) -> Permission {
        match s.to_lowercase().as_str() {
            "allow" => Permission::Allow,
            "deny" => Permission::Deny,
            _ => Permission::Ask,
        }
    }

    /// Get an agent by ID
    pub async fn get(&self, agent_id: &str) -> Option<AgentConfig> {
        let agents = self.agents.read().await;
        agents.get(agent_id).cloned()
    }

    /// Get all agents
    pub async fn get_all(&self) -> Vec<AgentConfig> {
        let agents = self.agents.read().await;
        agents.values().cloned().collect()
    }

    /// Get all primary agents (can be used as top-level agents)
    pub async fn get_primary_agents(&self) -> Vec<AgentConfig> {
        let agents = self.agents.read().await;
        agents
            .values()
            .filter(|a| a.is_primary())
            .cloned()
            .collect()
    }

    /// Get all subagents (can be delegated to)
    pub async fn get_subagents(&self) -> Vec<AgentConfig> {
        let agents = self.agents.read().await;
        agents
            .values()
            .filter(|a| a.is_subagent())
            .cloned()
            .collect()
    }

    /// Get agents by mode
    pub async fn get_by_mode(&self, mode: AgentMode) -> Vec<AgentConfig> {
        let agents = self.agents.read().await;
        agents
            .values()
            .filter(|a| match mode {
                AgentMode::Primary => a.is_primary(),
                AgentMode::Subagent => a.is_subagent(),
                AgentMode::All => true,
            })
            .cloned()
            .collect()
    }

    /// Register a new custom agent
    pub async fn register(&self, agent: AgentConfig) {
        let mut agents = self.agents.write().await;
        agents.insert(agent.name.clone(), agent);
    }

    /// Remove an agent (built-in agents cannot be removed)
    pub async fn unregister(&self, agent_id: &str) -> Result<(), String> {
        let mut agents = self.agents.write().await;

        if let Some(agent) = agents.get(agent_id) {
            if agent.built_in {
                return Err(format!("Cannot remove built-in agent: {}", agent_id));
            }
        }

        agents.remove(agent_id);
        Ok(())
    }

    /// Check if an agent exists
    pub async fn exists(&self, agent_id: &str) -> bool {
        let agents = self.agents.read().await;
        agents.contains_key(agent_id)
    }

    /// Get agent count
    pub async fn count(&self) -> usize {
        let agents = self.agents.read().await;
        agents.len()
    }

    /// List all agent IDs
    pub async fn list_ids(&self) -> Vec<String> {
        let agents = self.agents.read().await;
        agents.keys().cloned().collect()
    }

    /// Generate description text for Task tool
    ///
    /// Lists all subagents with their descriptions for the Task tool prompt.
    pub async fn subagent_descriptions(&self) -> String {
        let agents = self.agents.read().await;
        let mut lines = Vec::new();

        for agent in agents.values() {
            if agent.is_subagent() {
                let desc = agent
                    .description
                    .as_deref()
                    .unwrap_or("No description available");
                lines.push(format!("- {}: {}", agent.name, desc));
            }
        }

        lines.sort();
        lines.join("\n")
    }
}

impl Default for AgentRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for AgentRegistry {
    fn clone(&self) -> Self {
        Self {
            agents: self.agents.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_registry_creation() {
        let registry = AgentRegistry::new();
        let count = registry.count().await;
        assert_eq!(count, 7); // 7 built-in agents
    }

    #[tokio::test]
    async fn test_get_agent() {
        let registry = AgentRegistry::new();
        let agent = registry.get("build").await;
        assert!(agent.is_some());
        assert_eq!(agent.unwrap().name, "build");
    }

    #[tokio::test]
    async fn test_get_nonexistent_agent() {
        let registry = AgentRegistry::new();
        let agent = registry.get("nonexistent").await;
        assert!(agent.is_none());
    }

    #[tokio::test]
    async fn test_get_primary_agents() {
        let registry = AgentRegistry::new();
        let primary = registry.get_primary_agents().await;

        // build, plan, planner, architect
        assert_eq!(primary.len(), 4);

        let names: Vec<String> = primary.iter().map(|a| a.name.clone()).collect();
        assert!(names.contains(&"build".to_string()));
        assert!(names.contains(&"plan".to_string()));
    }

    #[tokio::test]
    async fn test_get_subagents() {
        let registry = AgentRegistry::new();
        let subagents = registry.get_subagents().await;

        // general, executor, arbiter
        assert_eq!(subagents.len(), 3);

        let names: Vec<String> = subagents.iter().map(|a| a.name.clone()).collect();
        assert!(names.contains(&"general".to_string()));
        assert!(names.contains(&"arbiter".to_string()));
    }

    #[tokio::test]
    async fn test_register_custom_agent() {
        let registry = AgentRegistry::new();

        let custom = AgentConfig::new("custom-agent");
        registry.register(custom).await;

        let count = registry.count().await;
        assert_eq!(count, 8); // 7 built-in + 1 custom

        let agent = registry.get("custom-agent").await;
        assert!(agent.is_some());
    }

    #[tokio::test]
    async fn test_cannot_remove_builtin() {
        let registry = AgentRegistry::new();
        let result = registry.unregister("build").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_can_remove_custom() {
        let registry = AgentRegistry::new();

        let custom = AgentConfig::new("custom");
        registry.register(custom).await;

        let result = registry.unregister("custom").await;
        assert!(result.is_ok());

        let exists = registry.exists("custom").await;
        assert!(!exists);
    }

    #[tokio::test]
    async fn test_list_ids() {
        let registry = AgentRegistry::new();
        let ids = registry.list_ids().await;

        assert_eq!(ids.len(), 7);
        assert!(ids.contains(&"general".to_string()));
        assert!(ids.contains(&"build".to_string()));
        assert!(ids.contains(&"plan".to_string()));
        assert!(ids.contains(&"arbiter".to_string()));
    }

    #[tokio::test]
    async fn test_subagent_descriptions() {
        let registry = AgentRegistry::new();
        let desc = registry.subagent_descriptions().await;

        assert!(desc.contains("general"));
        assert!(desc.contains("executor"));
        assert!(desc.contains("arbiter"));
        // Primary agents should NOT be in subagent descriptions
        assert!(!desc.contains("- build:"));
        assert!(!desc.contains("- plan:"));
    }

    #[test]
    fn test_parse_markdown_with_frontmatter() {
        let content = r#"---
description: Test agent
mode: subagent
temperature: 0.5
---

This is the custom prompt.
"#;
        let (fm, prompt) = AgentRegistry::parse_markdown(content).unwrap();
        assert_eq!(fm.get("description").unwrap().as_str().unwrap(), "Test agent");
        assert_eq!(fm.get("mode").unwrap().as_str().unwrap(), "subagent");
        assert_eq!(fm.get("temperature").unwrap().as_f64().unwrap(), 0.5);
        assert_eq!(prompt.trim(), "This is the custom prompt.");
    }

    #[test]
    fn test_parse_markdown_no_frontmatter() {
        let content = "Just a prompt with no frontmatter";
        let (fm, prompt) = AgentRegistry::parse_markdown(content).unwrap();
        assert!(fm.is_empty());
        assert_eq!(prompt, "Just a prompt with no frontmatter");
    }
}
