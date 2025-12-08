//! Agent assembly - Creates the rig agent with all tools

use crate::config::{Config, LlmProvider};
use crate::hooks::TelemetryHook;
use crate::telemetry::Telemetry;
use crate::templates::{ProjectContext, SystemPromptTemplate, Templates, WorktreeContext};
use crate::tools::*;
use anyhow::Result;
use rig::agent::{Agent, StreamingPromptRequest};
use rig::client::{CompletionClient, ProviderClient};
use rig::completion::Prompt;
use rig::message::Message;
use rig::providers::openrouter;
use rig::streaming::StreamingPrompt;

/// Maximum number of tool-call rounds before stopping
const MAX_TOOL_TURNS: usize = 20000000;
use std::path::PathBuf;
use std::sync::Arc;

/// The main Crow agent
pub struct CrowAgent {
    config: Config,
    telemetry: Arc<Telemetry>,
    working_dir: PathBuf,
    todo_store: TodoStore,
    session_id: String,
    templates: Arc<Templates>,
}

impl CrowAgent {
    /// Create a new CrowAgent with the given configuration
    pub fn new(config: Config, telemetry: Arc<Telemetry>) -> Self {
        let working_dir = config.working_dir.clone();
        let session_id = telemetry.session_id().to_string();
        Self {
            config,
            telemetry,
            working_dir,
            todo_store: TodoStore::new(),
            session_id,
            templates: Templates::new(),
        }
    }

    /// Create a new CrowAgent with a shared TodoStore (for multi-agent scenarios)
    pub fn with_todo_store(config: Config, telemetry: Arc<Telemetry>, todo_store: TodoStore) -> Self {
        let working_dir = config.working_dir.clone();
        let session_id = telemetry.session_id().to_string();
        Self {
            config,
            telemetry,
            working_dir,
            todo_store,
            session_id,
            templates: Templates::new(),
        }
    }

    /// Get the list of available tool names
    fn available_tools(&self) -> Vec<String> {
        vec![
            "read_file".to_string(),
            "edit_file".to_string(),
            "list_directory".to_string(),
            "grep".to_string(),
            "find_path".to_string(),
            "terminal".to_string(),
            "thinking".to_string(),
            "now".to_string(),
            "todo_write".to_string(),
            "todo_read".to_string(),
            "fetch".to_string(),
            "web_search".to_string(),
            "diagnostics".to_string(),
            "task_complete".to_string(),
        ]
    }

    /// Get the system prompt rendered from the template
    fn system_prompt(&self) -> String {
        let project = ProjectContext {
            worktrees: vec![WorktreeContext {
                abs_path: self.working_dir.display().to_string(),
                root_name: self.working_dir
                    .file_name()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_else(|| "root".to_string()),
                rules_file: None,
            }],
            os: std::env::consts::OS.to_string(),
            shell: std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string()),
            has_rules: false,
            has_user_rules: false,
            user_rules: vec![],
        };

        let template = SystemPromptTemplate {
            project: &project,
            available_tools: self.available_tools(),
            model_name: Some(self.config.llm.model.clone()),
        };

        template.render(&self.templates).unwrap_or_else(|e| {
            tracing::error!("Failed to render system prompt: {}", e);
            "You are a helpful software engineering assistant.".to_string()
        })
    }

    /// Create the rig agent based on configuration
    pub fn build_agent(&self) -> Result<Agent<openrouter::CompletionModel>> {
        let wd = self.working_dir.clone();

        match self.config.llm.provider {
            LlmProvider::OpenRouter => {
                let client = if let Some(ref key) = self.config.llm.api_key {
                    openrouter::Client::new(key)?
                } else {
                    openrouter::Client::from_env()
                };

                let mut builder = client
                    .agent(&self.config.llm.model)
                    .preamble(&self.system_prompt())
                    .tool(ReadFile::new(wd.clone()))
                    .tool(EditFile::new(wd.clone()))
                    .tool(ListDirectory::new(wd.clone()))
                    .tool(Grep::new(wd.clone()))
                    .tool(FindPath::new(wd.clone()))
                    .tool(Terminal::new(wd.clone()))
                    .tool(Thinking)
                    .tool(Now)
                    .tool(TodoWrite::new(self.todo_store.clone(), self.session_id.clone()))
                    .tool(TodoRead::new(self.todo_store.clone(), self.session_id.clone()))
                    .tool(Fetch::new())
                    .tool(WebSearch::new())
                    .tool(Diagnostics::new(wd.clone()))
                    .tool(TaskComplete::new());

                if let Some(max_tokens) = self.config.llm.max_tokens {
                    builder = builder.max_tokens(max_tokens as u64);
                }

                Ok(builder.build())
            }
            LlmProvider::Custom => {
                // For LM Studio or other OpenAI-compatible endpoints
                let base_url = self.config.llm.base_url.as_ref()
                    .ok_or_else(|| anyhow::anyhow!("base_url required for custom provider"))?;

                let api_key = self.config.llm.api_key.as_ref()
                    .map(|s| s.as_str())
                    .unwrap_or("no-key");

                let client = openrouter::Client::builder()
                    .api_key(api_key)
                    .base_url(base_url)
                    .build()?;

                let mut builder = client
                    .agent(&self.config.llm.model)
                    .preamble(&self.system_prompt())
                    .tool(ReadFile::new(wd.clone()))
                    .tool(EditFile::new(wd.clone()))
                    .tool(ListDirectory::new(wd.clone()))
                    .tool(Grep::new(wd.clone()))
                    .tool(FindPath::new(wd.clone()))
                    .tool(Terminal::new(wd.clone()))
                    .tool(Thinking)
                    .tool(Now)
                    .tool(TodoWrite::new(self.todo_store.clone(), self.session_id.clone()))
                    .tool(TodoRead::new(self.todo_store.clone(), self.session_id.clone()))
                    .tool(Fetch::new())
                    .tool(WebSearch::new())
                    .tool(Diagnostics::new(wd.clone()))
                    .tool(TaskComplete::new());

                if let Some(max_tokens) = self.config.llm.max_tokens {
                    builder = builder.max_tokens(max_tokens as u64);
                }

                Ok(builder.build())
            }
            _ => {
                // For now, default to OpenRouter for other providers
                // TODO: Add proper support for OpenAI, Anthropic
                anyhow::bail!("Provider {:?} not yet implemented, use OpenRouter or Custom",
                    self.config.llm.provider)
            }
        }
    }

    /// Run a single prompt and get response
    pub async fn prompt(&self, message: &str) -> Result<String> {
        self.telemetry.log_user_message(message).await;

        let start = std::time::Instant::now();
        let agent = self.build_agent()?;
        let hook = TelemetryHook::new(self.telemetry.clone());

        let response = agent
            .prompt(message)
            .with_hook(hook)
            .multi_turn(MAX_TOOL_TURNS)
            .await
            .map_err(|e| anyhow::anyhow!("Chat error: {}", e))?;

        let duration = start.elapsed().as_millis() as u64;
        self.telemetry.log_response(&response, None, duration, Some(&self.config.llm.model), None).await;

        Ok(response)
    }

    /// Run a multi-turn chat session
    pub async fn chat(&self, message: &str, history: &mut Vec<Message>) -> Result<String> {
        self.telemetry.log_user_message(message).await;

        let start = std::time::Instant::now();
        let agent = self.build_agent()?;
        let hook = TelemetryHook::new(self.telemetry.clone());

        let response = agent
            .prompt(message)
            .with_history(history)
            .with_hook(hook)
            .multi_turn(MAX_TOOL_TURNS)
            .await
            .map_err(|e| anyhow::anyhow!("Chat error: {}", e))?;

        // History is updated by with_history

        let duration = start.elapsed().as_millis() as u64;
        self.telemetry.log_response(&response, None, duration, Some(&self.config.llm.model), None).await;

        Ok(response)
    }

    /// Get reference to telemetry
    pub fn telemetry(&self) -> &Arc<Telemetry> {
        &self.telemetry
    }

    /// Get working directory
    pub fn working_dir(&self) -> &PathBuf {
        &self.working_dir
    }

    /// Run a streaming multi-turn chat session
    /// Returns a tuple of (stream request, hook) - the hook can be used for cancellation
    pub async fn chat_stream(
        &self,
        message: &str,
        history: Vec<Message>,
    ) -> Result<(StreamingPromptRequest<openrouter::CompletionModel, TelemetryHook>, TelemetryHook)> {
        self.telemetry.log_user_message(message).await;

        let agent = self.build_agent()?;
        let hook = TelemetryHook::new(self.telemetry.clone());
        let hook_clone = hook.clone();

        let request = agent
            .stream_prompt(message)
            .with_history(history)
            .with_hook(hook)
            .multi_turn(MAX_TOOL_TURNS);

        Ok((request, hook_clone))
    }
}

/// Re-export streaming types for use in ACP layer
pub use rig::agent::MultiTurnStreamItem;
pub use rig::streaming::StreamedAssistantContent;
