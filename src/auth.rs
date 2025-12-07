//! Authentication configuration loading
//!
//! Loads API keys and provider settings from:
//! - $XDG_DATA_HOME/crow/auth.json (preferred)
//! - ~/.local/share/crow/auth.json (fallback)
//! - ~/.crow_agent/auth.json (legacy)

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// Authentication entry for a provider
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum AuthEntry {
    /// Simple API key authentication
    #[serde(rename = "api")]
    Api {
        key: String,
        #[serde(default)]
        base_url: Option<String>,
    },
}

impl AuthEntry {
    /// Get the API key
    pub fn api_key(&self) -> &str {
        match self {
            AuthEntry::Api { key, .. } => key,
        }
    }

    /// Get the base URL if configured
    pub fn base_url(&self) -> Option<&str> {
        match self {
            AuthEntry::Api { base_url, .. } => base_url.as_deref(),
        }
    }
}

/// Authentication configuration file
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AuthConfig {
    pub providers: HashMap<String, AuthEntry>,
}

impl AuthConfig {
    /// Load auth config from the default location
    pub fn load() -> anyhow::Result<Self> {
        let path = Self::config_path()?;
        Self::load_from(&path)
    }

    /// Load auth config from a specific path
    pub fn load_from(path: &PathBuf) -> anyhow::Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }

        let content = std::fs::read_to_string(path)?;
        let config: AuthConfig = serde_json::from_str(&content)?;
        Ok(config)
    }

    /// Get the default config file path
    pub fn config_path() -> anyhow::Result<PathBuf> {
        // Try XDG_DATA_HOME first
        if let Ok(xdg_data) = std::env::var("XDG_DATA_HOME") {
            let path = PathBuf::from(xdg_data).join("crow/auth.json");
            if path.exists() {
                return Ok(path);
            }
        }

        // Try ~/.local/share/crow/auth.json
        if let Ok(home) = std::env::var("HOME") {
            let path = PathBuf::from(&home).join(".local/share/crow/auth.json");
            if path.exists() {
                return Ok(path);
            }

            // Try legacy ~/.crow_agent/auth.json
            let legacy_path = PathBuf::from(&home).join(".crow_agent/auth.json");
            if legacy_path.exists() {
                return Ok(legacy_path);
            }

            // Default to XDG location even if it doesn't exist yet
            return Ok(PathBuf::from(home).join(".local/share/crow/auth.json"));
        }

        Ok(PathBuf::from(".crow_agent/auth.json"))
    }

    /// Get auth entry for a provider
    pub fn get(&self, provider: &str) -> Option<&AuthEntry> {
        self.providers.get(provider)
    }

    /// Get API key for a provider
    pub fn api_key(&self, provider: &str) -> Option<&str> {
        self.providers.get(provider).map(|e| e.api_key())
    }

    /// Get base URL for a provider
    pub fn base_url(&self, provider: &str) -> Option<&str> {
        self.providers.get(provider).and_then(|e| e.base_url())
    }

    /// List all configured providers
    pub fn providers(&self) -> impl Iterator<Item = &String> {
        self.providers.keys()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_auth_config() {
        let json = r#"{
            "openrouter": {"type": "api", "key": "sk-xxx"},
            "lm-studio": {"type": "api", "key": "lm-studio", "base_url": "http://localhost:1234/v1"}
        }"#;

        let config: AuthConfig = serde_json::from_str(json).unwrap();

        assert_eq!(config.api_key("openrouter"), Some("sk-xxx"));
        assert_eq!(config.base_url("openrouter"), None);

        assert_eq!(config.api_key("lm-studio"), Some("lm-studio"));
        assert_eq!(
            config.base_url("lm-studio"),
            Some("http://localhost:1234/v1")
        );
    }
}
