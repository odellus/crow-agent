//! Provider configuration

use serde::{Deserialize, Serialize};

/// Configuration for an OpenAI-compatible provider
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    /// Display name for the provider
    pub name: String,
    /// API base URL (e.g., "https://api.openai.com/v1")
    pub base_url: String,
    /// Environment variable name for the API key
    pub api_key_env: String,
    /// Default model to use
    pub default_model: String,
}

impl ProviderConfig {
    /// Create an OpenRouter provider config
    pub fn openrouter() -> Self {
        Self {
            name: "OpenRouter".to_string(),
            base_url: "https://openrouter.ai/api/v1".to_string(),
            api_key_env: "OPENROUTER_API_KEY".to_string(),
            default_model: "anthropic/claude-sonnet-4".to_string(),
        }
    }

    /// Create an OpenAI provider config
    pub fn openai() -> Self {
        Self {
            name: "OpenAI".to_string(),
            base_url: "https://api.openai.com/v1".to_string(),
            api_key_env: "OPENAI_API_KEY".to_string(),
            default_model: "gpt-4o".to_string(),
        }
    }

    /// Create an Anthropic provider config (via their OpenAI-compatible endpoint)
    pub fn anthropic() -> Self {
        Self {
            name: "Anthropic".to_string(),
            base_url: "https://api.anthropic.com/v1".to_string(),
            api_key_env: "ANTHROPIC_API_KEY".to_string(),
            default_model: "claude-sonnet-4-20250514".to_string(),
        }
    }

    /// Create a Moonshot/Kimi provider config
    pub fn moonshot() -> Self {
        Self {
            name: "Moonshot".to_string(),
            base_url: "https://api.moonshot.ai/v1".to_string(),
            api_key_env: "MOONSHOT_API_KEY".to_string(),
            default_model: "kimi-k2-0711-preview".to_string(),
        }
    }

    /// Create a custom provider config (e.g., LM Studio, vLLM)
    pub fn custom(
        name: impl Into<String>,
        base_url: impl Into<String>,
        api_key_env: impl Into<String>,
        default_model: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            base_url: base_url.into(),
            api_key_env: api_key_env.into(),
            default_model: default_model.into(),
        }
    }
}
