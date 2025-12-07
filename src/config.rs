//! Configuration for the Crow Agent

use crate::auth::AuthConfig;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Main configuration for the agent
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// LLM provider configuration
    pub llm: LlmConfig,

    /// Working directory for file operations
    pub working_dir: PathBuf,

    /// Telemetry settings
    pub telemetry: TelemetryConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmConfig {
    /// Provider type (openrouter, openai, anthropic, etc.)
    pub provider: LlmProvider,

    /// API key (can also be set via environment variable)
    pub api_key: Option<String>,

    /// Base URL for the API (for custom endpoints like LM Studio)
    pub base_url: Option<String>,

    /// Model name/ID
    pub model: String,

    /// Maximum tokens for response
    pub max_tokens: Option<u32>,

    /// Temperature for sampling
    pub temperature: Option<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub enum LlmProvider {
    #[default]
    OpenRouter,
    OpenAI,
    Anthropic,
    /// Custom OpenAI-compatible endpoint (e.g., LM Studio, vLLM)
    Custom,
}

impl LlmProvider {
    pub fn as_str(&self) -> &'static str {
        match self {
            LlmProvider::OpenRouter => "openrouter",
            LlmProvider::OpenAI => "openai",
            LlmProvider::Anthropic => "anthropic",
            LlmProvider::Custom => "custom",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelemetryConfig {
    /// Directory for log files
    pub log_dir: PathBuf,

    /// Enable verbose logging
    pub verbose: bool,

    /// Log raw JSON requests/responses
    pub log_raw: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            llm: LlmConfig::default(),
            working_dir: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            telemetry: TelemetryConfig::default(),
        }
    }
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            provider: LlmProvider::OpenRouter,
            api_key: None,
            base_url: None,
            model: "glm-4.5-air@q4_k_m".to_string(),
            max_tokens: Some(4096),
            temperature: Some(0.7),
        }
    }
}

impl Default for TelemetryConfig {
    fn default() -> Self {
        Self {
            log_dir: PathBuf::from(".crow_logs"),
            verbose: false,
            log_raw: false,
        }
    }
}

impl Config {
    /// Create config for local LM Studio instance
    pub fn lm_studio(base_url: &str, model: &str, working_dir: PathBuf) -> Self {
        Self {
            llm: LlmConfig {
                provider: LlmProvider::Custom,
                api_key: Some("lm-studio".to_string()), // LM Studio doesn't need real key
                base_url: Some(base_url.to_string()),
                model: model.to_string(),
                max_tokens: Some(4096),
                temperature: Some(0.7),
            },
            working_dir,
            telemetry: TelemetryConfig::default(),
        }
    }

    /// Create config for OpenRouter
    pub fn openrouter(model: &str, working_dir: PathBuf) -> Self {
        Self {
            llm: LlmConfig {
                provider: LlmProvider::OpenRouter,
                api_key: std::env::var("OPENROUTER_API_KEY").ok(),
                base_url: None,
                model: model.to_string(),
                max_tokens: Some(4096),
                temperature: Some(0.7),
            },
            working_dir,
            telemetry: TelemetryConfig::default(),
        }
    }

    /// Set verbose logging
    pub fn with_verbose(mut self, verbose: bool) -> Self {
        self.telemetry.verbose = verbose;
        self
    }

    /// Set log directory
    pub fn with_log_dir(mut self, log_dir: PathBuf) -> Self {
        self.telemetry.log_dir = log_dir;
        self
    }

    /// Set API key
    pub fn with_api_key(mut self, api_key: String) -> Self {
        self.llm.api_key = Some(api_key);
        self
    }

    /// Create config from a provider name in auth.json
    ///
    /// Looks up the provider in auth.json and uses its API key and base_url.
    /// The model should be specified separately.
    pub fn from_provider(
        provider: &str,
        model: &str,
        working_dir: PathBuf,
    ) -> anyhow::Result<Self> {
        let auth = AuthConfig::load()?;

        let entry = auth
            .get(provider)
            .ok_or_else(|| anyhow::anyhow!("Provider '{}' not found in auth.json", provider))?;

        let llm_provider = if entry.base_url().is_some() {
            LlmProvider::Custom
        } else {
            // Infer provider type from name
            match provider.to_lowercase().as_str() {
                "openrouter" => LlmProvider::OpenRouter,
                "openai" => LlmProvider::OpenAI,
                "anthropic" => LlmProvider::Anthropic,
                _ => LlmProvider::Custom,
            }
        };

        Ok(Self {
            llm: LlmConfig {
                provider: llm_provider,
                api_key: Some(entry.api_key().to_string()),
                base_url: entry.base_url().map(String::from),
                model: model.to_string(),
                max_tokens: Some(4096),
                temperature: Some(0.7),
            },
            working_dir,
            telemetry: TelemetryConfig::default(),
        })
    }

    /// List available providers from auth.json
    pub fn list_providers() -> anyhow::Result<Vec<String>> {
        let auth = AuthConfig::load()?;
        Ok(auth.providers().cloned().collect())
    }
}
