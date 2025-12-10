//! Agent configuration loader
//!
//! Loads agent configs from YAML files:
//! - Global: ~/.config/crow/agents/*.yaml (XDG_CONFIG_HOME)
//! - Project: .crow/agents/*.yaml
//!
//! Project configs override global configs with the same name.

use super::config::AgentConfig;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Load all agent configs from global and project directories
///
/// Priority: project configs override global configs
pub fn load_agent_configs(working_dir: &Path) -> HashMap<String, AgentConfig> {
    let mut configs = HashMap::new();

    // Load global configs first (lower priority)
    if let Some(global_dir) = global_config_dir() {
        load_configs_from_dir(&global_dir, &mut configs);
    }

    // Load project configs (higher priority, overrides global)
    let project_dir = working_dir.join(".crow").join("agents");
    load_configs_from_dir(&project_dir, &mut configs);

    configs
}

/// Get the global agent config directory
/// Uses XDG_CONFIG_HOME or falls back to ~/.config
fn global_config_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("crow").join("agents"))
}

/// Load all .yaml files from a directory into the configs map
fn load_configs_from_dir(dir: &Path, configs: &mut HashMap<String, AgentConfig>) {
    if !dir.is_dir() {
        return;
    }

    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.filter_map(|e| e.ok()) {
        let path = entry.path();

        // Only process .yaml and .yml files
        let ext = path.extension().and_then(|e| e.to_str());
        if !matches!(ext, Some("yaml") | Some("yml")) {
            continue;
        }

        // Agent name is the file stem
        let name = match path.file_stem().and_then(|s| s.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };

        match load_config_file(&path) {
            Ok(mut config) => {
                // Ensure the config name matches the filename
                config.name = name.clone();
                configs.insert(name, config);
            }
            Err(e) => {
                eprintln!("Warning: Failed to load agent config {}: {}", path.display(), e);
            }
        }
    }
}

/// Load a single agent config from a YAML file
fn load_config_file(path: &Path) -> Result<AgentConfig, String> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| format!("Failed to read file: {}", e))?;

    serde_yaml::from_str(&content)
        .map_err(|e| format!("Failed to parse YAML: {}", e))
}

/// Save an agent config to a YAML file
pub fn save_config_file(path: &Path, config: &AgentConfig) -> Result<(), String> {
    // Ensure parent directory exists
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create directory: {}", e))?;
    }

    let content = serde_yaml::to_string(config)
        .map_err(|e| format!("Failed to serialize config: {}", e))?;

    std::fs::write(path, content)
        .map_err(|e| format!("Failed to write file: {}", e))
}

/// Get the path where a project-level agent config would be saved
pub fn project_config_path(working_dir: &Path, agent_name: &str) -> PathBuf {
    working_dir.join(".crow").join("agents").join(format!("{}.yaml", agent_name))
}

/// Get the path where a global agent config would be saved
pub fn global_config_path(agent_name: &str) -> Option<PathBuf> {
    global_config_dir().map(|d| d.join(format!("{}.yaml", agent_name)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_load_empty_dir() {
        let temp = TempDir::new().unwrap();
        let agents_dir = temp.path().join("agents");
        std::fs::create_dir_all(&agents_dir).unwrap();

        let mut configs = HashMap::new();
        load_configs_from_dir(&agents_dir, &mut configs);
        assert!(configs.is_empty());
    }

    #[test]
    fn test_load_single_config() {
        let temp = TempDir::new().unwrap();
        let agents_dir = temp.path().join("agents");
        std::fs::create_dir_all(&agents_dir).unwrap();

        let config_content = r#"
description: "Test agent"
mode: primary
control_flow: loop
"#;
        std::fs::write(agents_dir.join("test.yaml"), config_content).unwrap();

        let mut configs = HashMap::new();
        load_configs_from_dir(&agents_dir, &mut configs);
        assert_eq!(configs.len(), 1);

        let test = configs.get("test").unwrap();
        assert_eq!(test.name, "test");
        assert_eq!(test.description, Some("Test agent".to_string()));
        assert_eq!(test.control_flow, crate::agent::ControlFlowConfig::Loop);
    }

    #[test]
    fn test_save_and_load_config() {
        use crate::agent::ControlFlowConfig;

        let temp = TempDir::new().unwrap();
        let path = temp.path().join("agent.yaml");

        let config = AgentConfig::new("test")
            .with_description("A test agent")
            .with_control_flow(ControlFlowConfig::Static);

        save_config_file(&path, &config).unwrap();

        let loaded = load_config_file(&path).unwrap();
        assert_eq!(loaded.description, Some("A test agent".to_string()));
        assert_eq!(loaded.control_flow, ControlFlowConfig::Static);
    }
}
