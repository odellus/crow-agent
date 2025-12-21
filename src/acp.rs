//! ACP (Agent Client Protocol) server implementation
//!
//! This module implements the ACP Agent trait to allow crow_agent to be used
//! as an external agent server with Zed or other ACP-compatible clients.
//!
//! The server communicates over stdio using JSON-RPC 2.0.

use agent_client_protocol::{
    self as acp, AgentCapabilities, AuthenticateRequest, AuthenticateResponse, CancelNotification,
    ContentBlock, ContentChunk, ExtNotification, ExtRequest, ExtResponse, Implementation,
    InitializeRequest, InitializeResponse, LoadSessionRequest, LoadSessionResponse, ModelId,
    ModelInfo, NewSessionRequest, NewSessionResponse, Plan, PlanEntry, PlanEntryPriority,
    PlanEntryStatus, PromptCapabilities, PromptRequest, PromptResponse, ProtocolVersion, SessionId,
    SessionMode, SessionModeId, SessionModeState, SessionModelState, SessionNotification,
    SessionUpdate, SetSessionModeRequest, SetSessionModeResponse, StopReason, TextContent,
    ToolCall, ToolCallId, ToolCallStatus, ToolCallUpdate, ToolCallUpdateFields, ToolKind,
};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info};

use async_openai::types::{
    ChatCompletionRequestMessage, ChatCompletionRequestSystemMessageArgs,
    ChatCompletionRequestUserMessageArgs, ChatCompletionTool,
};

use crate::agent::{build_system_prompt, Agent, AgentRegistry};
use crate::config::Config;
use crate::events::AgentEvent;
use crate::provider::{ProviderClient, ProviderConfig};
use crate::snapshot::{Patch, SnapshotManager};
use crate::telemetry::Telemetry;
use crate::tool::ToolRegistry;
use crate::tools::{self, TodoStore};

/// Outgoing notifications from agent to client
pub enum OutgoingNotification {
    Session(SessionNotification),
    Extension(ExtNotification),
}

/// Session state for an active ACP session
struct Session {
    /// The full agent with control flow and coagent support
    /// Stored in Option so we can take() it during run without needing Default
    agent: RefCell<Option<Agent>>,
    /// Current agent name (for mode switching)
    agent_name: RefCell<String>,
    /// Agent registry (for mode switching)
    agent_registry: AgentRegistry,
    /// Primary tool registry
    registry: ToolRegistry,
    /// OpenAI-format tools for LLM (primary)
    tools: Vec<ChatCompletionTool>,
    /// Coagent tool registry (if using coagent mode)
    coagent_registry: Option<ToolRegistry>,
    /// Coagent tools (if using coagent mode)
    coagent_tools: Vec<ChatCompletionTool>,
    /// Shared TodoStore - persists across mode switches
    todo_store: TodoStore,
    /// Conversation history
    history: RefCell<Vec<ChatCompletionRequestMessage>>,
    /// Cancellation token for current operation
    cancellation: RefCell<Option<CancellationToken>>,
    /// Snapshot manager for tracking file changes
    snapshot_manager: SnapshotManager,
    /// Current snapshot hash (taken before each prompt)
    current_snapshot: RefCell<Option<String>>,
    /// Accumulated patches from file-modifying tools (for undo)
    patches: RefCell<Vec<Patch>>,
    /// Working directory for this session
    working_dir: std::path::PathBuf,
}

/// Crow's ACP Agent implementation
pub struct CrowAcpAgent {
    /// Channel for sending notifications back to the client
    notification_tx: mpsc::UnboundedSender<(OutgoingNotification, oneshot::Sender<()>)>,
    /// Counter for generating session IDs
    next_session_id: Cell<u64>,
    /// Active sessions
    sessions: RefCell<HashMap<String, Session>>,
    /// Base configuration (working dir, model, etc.)
    config: Config,
    /// Shared telemetry instance
    telemetry: Arc<Telemetry>,
    /// Provider for LLM calls
    provider: Arc<ProviderClient>,
}

impl CrowAcpAgent {
    /// Create a new CrowAcpAgent
    pub fn new(
        notification_tx: mpsc::UnboundedSender<(OutgoingNotification, oneshot::Sender<()>)>,
        config: Config,
        telemetry: Arc<Telemetry>,
    ) -> Result<Self, String> {
        // Build provider from config
        let base_url = config
            .llm
            .base_url
            .as_deref()
            .unwrap_or("http://localhost:1234/v1");
        let provider_config = ProviderConfig::custom(
            "lm-studio", // Use lm-studio as provider name for auth.json lookup
            base_url,
            "LM_STUDIO_API_KEY", // Env var fallback
            &config.llm.model,
        );
        let provider = Arc::new(ProviderClient::new(provider_config)?);

        Ok(Self {
            notification_tx,
            next_session_id: Cell::new(0),
            sessions: RefCell::new(HashMap::new()),
            config,
            telemetry,
            provider,
        })
    }

    /// Send a session update notification and wait for acknowledgment
    async fn send_update(&self, notification: SessionNotification) -> acp::Result<()> {
        let (tx, rx) = oneshot::channel();
        self.notification_tx
            .send((OutgoingNotification::Session(notification), tx))
            .map_err(|_| acp::Error::internal_error())?;
        rx.await.map_err(|_| acp::Error::internal_error())?;
        Ok(())
    }

    /// Send an extension notification and wait for acknowledgment
    async fn send_ext_notification(&self, notification: ExtNotification) -> acp::Result<()> {
        let (tx, rx) = oneshot::channel();
        self.notification_tx
            .send((OutgoingNotification::Extension(notification), tx))
            .map_err(|_| acp::Error::internal_error())?;
        rx.await.map_err(|_| acp::Error::internal_error())?;
        Ok(())
    }
}

#[async_trait::async_trait(?Send)]
impl acp::Agent for CrowAcpAgent {
    async fn initialize(&self, args: InitializeRequest) -> acp::Result<InitializeResponse> {
        info!(
            "ACP initialize: protocol_version={:?}",
            args.protocol_version
        );

        Ok(InitializeResponse::new(ProtocolVersion::V1)
            .agent_capabilities(
                AgentCapabilities::new()
                    .load_session(false)
                    .prompt_capabilities(PromptCapabilities::default()),
            )
            .agent_info(Implementation::new("crow-agent", env!("CARGO_PKG_VERSION"))))
    }

    async fn authenticate(&self, _args: AuthenticateRequest) -> acp::Result<AuthenticateResponse> {
        // No authentication required
        Ok(AuthenticateResponse::default())
    }

    async fn new_session(&self, args: NewSessionRequest) -> acp::Result<NewSessionResponse> {
        info!("ACP new_session: cwd={:?}", args.cwd);

        // Generate a new session ID
        let session_id = self.next_session_id.get();
        self.next_session_id.set(session_id + 1);
        let session_id_str = session_id.to_string();

        // Use the provided cwd for this session
        let working_dir = args.cwd.clone();
        let model = self.config.llm.model.clone();

        // Load agent registry
        let agent_registry = AgentRegistry::new_with_config(&working_dir).await;

        // Build available modes from agent registry
        // Only include agents that can be used as primary (not subagents or coagents)
        let all_agents = agent_registry.get_all().await;
        let available_modes: Vec<SessionMode> = all_agents
            .iter()
            .filter(|config| config.mode.is_primary())
            .map(|config| {
                let mode = SessionMode::new(
                    SessionModeId::new(config.name.as_str()),
                    config.name.clone(),
                );
                if let Some(desc) = &config.description {
                    mode.description(desc.clone())
                } else {
                    mode
                }
            })
            .collect();

        // Use "build" agent by default (matches CLI default)
        let agent_name = "build".to_string();
        let agent_config = agent_registry.get(&agent_name).await.ok_or_else(|| {
            error!("Agent '{}' not found", agent_name);
            acp::Error::internal_error()
        })?;

        // Get control flow from agent config
        let control_flow = agent_config.get_control_flow();

        // Create shared TodoStore
        let todo_store = TodoStore::new();
        let telemetry_session_id = self.telemetry.session_id().to_string();

        // Check if we need a coagent
        let (agent, coagent_registry, coagent_tools) = if agent_config.has_coagent() {
            // Load coagent config
            let coagent_name = agent_config.coagent_name();
            let coagent_config = agent_registry.get(coagent_name).await.ok_or_else(|| {
                error!("Coagent '{}' not found", coagent_name);
                acp::Error::internal_error()
            })?;

            // Coagent uses same session_id as primary for trace linking
            let coagent_session_id = telemetry_session_id.clone();

            // Create agent with coagent and shared todos
            let agent = Agent::with_coagent_and_telemetry(
                &agent_name,
                agent_config.clone(),
                coagent_config.clone(),
                self.provider.clone(),
                working_dir.clone(),
                control_flow,
                self.telemetry.clone(),
            )
            .with_shared_todos(
                todo_store.clone(),
                &telemetry_session_id,
                &coagent_session_id,
            );

            // Create coagent tool registry with shared TodoStore
            let coagent_reg = tools::create_coagent_registry(
                working_dir.clone(),
                coagent_session_id,
                todo_store.clone(),
                false,
            );

            // Filter coagent tools based on coagent config permissions
            let coagent_tools: Vec<_> = coagent_reg
                .to_openai_tools()
                .into_iter()
                .filter(|t| coagent_config.is_tool_enabled(&t.function.name))
                .collect();

            (agent, Some(coagent_reg), coagent_tools)
        } else {
            // No coagent - create simple agent
            let agent = Agent::with_telemetry(
                &agent_name,
                agent_config.clone(),
                self.provider.clone(),
                working_dir.clone(),
                control_flow,
                self.telemetry.clone(),
            );
            (agent, None, vec![])
        };

        // Create primary tool registry with task tool for subagent spawning
        // Clone agent_registry since create_full_registry_async takes ownership
        let registry = tools::create_full_registry_async(
            working_dir.clone(),
            telemetry_session_id,
            todo_store.clone(),
            agent_registry.clone(),
            self.provider.clone(),
        )
        .await;

        // Filter tools based on agent permissions
        let tools: Vec<_> = registry
            .to_openai_tools()
            .into_iter()
            .filter(|t| agent_config.is_tool_enabled(&t.function.name))
            .collect();

        // Build system prompt
        let system_prompt =
            build_system_prompt(&model, &working_dir, agent_config.system_prompt.as_deref());

        // Create snapshot manager for this session's working directory
        let snapshot_manager = SnapshotManager::for_directory(working_dir.clone());

        // Initialize history with system prompt
        let history = vec![ChatCompletionRequestSystemMessageArgs::default()
            .content(system_prompt.clone())
            .build()
            .map_err(|_| acp::Error::internal_error())?
            .into()];

        // Build mode state for response
        let modes = SessionModeState::new(SessionModeId::new(agent_name.as_str()), available_modes);

        // Build model state for response
        // For now, expose the configured model as the only available model
        let current_model = model.clone();
        let available_models = vec![ModelInfo::new(
            ModelId::new(current_model.as_str()),
            current_model.clone(),
        )];
        let models = SessionModelState::new(ModelId::new(current_model.as_str()), available_models);

        // Store the session
        self.sessions.borrow_mut().insert(
            session_id_str.clone(),
            Session {
                agent: RefCell::new(Some(agent)),
                agent_name: RefCell::new(agent_name),
                agent_registry,
                registry,
                tools,
                coagent_registry,
                coagent_tools,
                todo_store,
                history: RefCell::new(history),
                cancellation: RefCell::new(None),
                snapshot_manager,
                current_snapshot: RefCell::new(None),
                patches: RefCell::new(Vec::new()),
                working_dir,
            },
        );

        Ok(NewSessionResponse::new(SessionId::new(session_id_str))
            .modes(modes)
            .models(models))
    }

    async fn load_session(&self, _args: LoadSessionRequest) -> acp::Result<LoadSessionResponse> {
        // Session persistence not yet implemented
        Err(acp::Error::method_not_found())
    }

    async fn set_session_mode(
        &self,
        args: SetSessionModeRequest,
    ) -> acp::Result<SetSessionModeResponse> {
        let session_id = args.session_id.0.to_string();
        let new_mode_id = args.mode_id.0.to_string();
        info!(
            "ACP set_session_mode: session_id={}, mode_id={}",
            session_id, new_mode_id
        );

        // Get session data we need (including existing TodoStore to preserve state)
        let (agent_registry, working_dir, todo_store) = {
            let sessions = self.sessions.borrow();
            let session = sessions
                .get(&session_id)
                .ok_or_else(acp::Error::invalid_params)?;
            (
                session.agent_registry.clone(),
                session.working_dir.clone(),
                session.todo_store.clone(),
            )
        };

        // Load the new agent config
        let agent_config = agent_registry.get(&new_mode_id).await.ok_or_else(|| {
            error!("Agent '{}' not found", new_mode_id);
            acp::Error::invalid_params()
        })?;

        // Get control flow from agent config
        let control_flow = agent_config.get_control_flow();

        // Reuse existing TodoStore from session (preserves todos across mode switches)
        let telemetry_session_id = self.telemetry.session_id().to_string();

        // Check if we need a coagent
        let (new_agent, new_coagent_registry, new_coagent_tools) = if agent_config.has_coagent() {
            let coagent_name = agent_config.coagent_name();
            let coagent_config = agent_registry.get(coagent_name).await.ok_or_else(|| {
                error!("Coagent '{}' not found", coagent_name);
                acp::Error::internal_error()
            })?;

            let coagent_session_id = telemetry_session_id.clone();

            let agent = Agent::with_coagent_and_telemetry(
                &new_mode_id,
                agent_config.clone(),
                coagent_config.clone(),
                self.provider.clone(),
                working_dir.clone(),
                control_flow,
                self.telemetry.clone(),
            )
            .with_shared_todos(
                todo_store.clone(),
                &telemetry_session_id,
                &coagent_session_id,
            );

            let coagent_reg = tools::create_coagent_registry(
                working_dir.clone(),
                coagent_session_id,
                todo_store.clone(),
                false,
            );

            let coagent_tools: Vec<_> = coagent_reg
                .to_openai_tools()
                .into_iter()
                .filter(|t| coagent_config.is_tool_enabled(&t.function.name))
                .collect();

            (agent, Some(coagent_reg), coagent_tools)
        } else {
            let agent = Agent::with_telemetry(
                &new_mode_id,
                agent_config.clone(),
                self.provider.clone(),
                working_dir.clone(),
                control_flow,
                self.telemetry.clone(),
            );
            (agent, None, vec![])
        };

        // Create new tool registry
        let new_registry = tools::create_full_registry_async(
            working_dir.clone(),
            telemetry_session_id,
            todo_store.clone(),
            agent_registry.clone(),
            self.provider.clone(),
        )
        .await;

        // Filter tools based on agent permissions
        let new_tools: Vec<_> = new_registry
            .to_openai_tools()
            .into_iter()
            .filter(|t| agent_config.is_tool_enabled(&t.function.name))
            .collect();

        // Build new system prompt
        let model = self.config.llm.model.clone();
        let system_prompt =
            build_system_prompt(&model, &working_dir, agent_config.system_prompt.as_deref());

        // Update session with new agent
        {
            let sessions = self.sessions.borrow();
            if let Some(session) = sessions.get(&session_id) {
                *session.agent.borrow_mut() = Some(new_agent);
                *session.agent_name.borrow_mut() = new_mode_id.clone();

                // Update history with new system prompt (keep user messages)
                let mut history = session.history.borrow_mut();
                if !history.is_empty() {
                    history[0] = ChatCompletionRequestSystemMessageArgs::default()
                        .content(system_prompt)
                        .build()
                        .unwrap()
                        .into();
                }
            }
        }

        // Update mutable fields outside the borrow
        {
            let mut sessions = self.sessions.borrow_mut();
            if let Some(session) = sessions.get_mut(&session_id) {
                session.registry = new_registry;
                session.tools = new_tools;
                session.coagent_registry = new_coagent_registry;
                session.coagent_tools = new_coagent_tools;
            }
        }

        Ok(SetSessionModeResponse::new())
    }

    async fn prompt(&self, args: PromptRequest) -> acp::Result<PromptResponse> {
        let session_id = args.session_id.0.to_string();
        info!("ACP prompt: session_id={}", session_id);

        // Convert prompt content to a single string
        let prompt_text = args
            .prompt
            .iter()
            .filter_map(|content| match content {
                ContentBlock::Text(text) => Some(text.text.clone()),
                _ => None, // Skip images, audio, etc. for now
            })
            .collect::<Vec<_>>()
            .join("\n");

        if prompt_text.is_empty() {
            return Err(acp::Error::invalid_params());
        }

        // Take a snapshot before processing (for undo support)
        let snapshot_hash = {
            let sessions = self.sessions.borrow();
            if let Some(session) = sessions.get(&session_id) {
                match session.snapshot_manager.track().await {
                    Ok(hash) => {
                        if let Some(ref h) = hash {
                            info!("Snapshot tracked: {}", &h[..12.min(h.len())]);
                        }
                        *session.current_snapshot.borrow_mut() = hash.clone();
                        hash
                    }
                    Err(e) => {
                        debug!("Failed to track snapshot: {}", e);
                        None
                    }
                }
            } else {
                None
            }
        };

        // Create cancellation token for this prompt
        let cancellation = CancellationToken::new();
        {
            let sessions = self.sessions.borrow();
            if let Some(session) = sessions.get(&session_id) {
                *session.cancellation.borrow_mut() = Some(cancellation.clone());
            }
        }

        // Add user message to history
        {
            let sessions = self.sessions.borrow();
            if let Some(session) = sessions.get(&session_id) {
                let user_msg = ChatCompletionRequestUserMessageArgs::default()
                    .content(prompt_text.clone())
                    .build()
                    .map_err(|_| acp::Error::internal_error())?;
                session.history.borrow_mut().push(user_msg.into());
            }
        }

        // Run the agent turn and stream events
        let result = self
            .run_agent_turn(&session_id, &args.session_id, cancellation, snapshot_hash)
            .await;

        // Clear cancellation token
        {
            let sessions = self.sessions.borrow();
            if let Some(session) = sessions.get(&session_id) {
                *session.cancellation.borrow_mut() = None;
            }
        }

        result
    }

    async fn cancel(&self, args: CancelNotification) -> acp::Result<()> {
        let session_id = args.session_id.0.to_string();
        info!("ACP cancel: session_id={}", session_id);

        let sessions = self.sessions.borrow();
        if let Some(session) = sessions.get(&session_id) {
            if let Some(ref token) = *session.cancellation.borrow() {
                info!("Cancelling session {}", session_id);
                token.cancel();
            }
        }

        Ok(())
    }

    async fn ext_method(&self, args: ExtRequest) -> acp::Result<ExtResponse> {
        info!("ACP ext_method: {}", args.method);

        match &*args.method {
            "session/revert" => {
                // Revert to a previous snapshot
                let params = serde_json::from_str::<serde_json::Value>(args.params.get())
                    .unwrap_or(serde_json::Value::Null);

                let session_id = params
                    .get("session_id")
                    .and_then(|v| v.as_str())
                    .ok_or_else(acp::Error::invalid_params)?;

                let sessions = self.sessions.borrow();
                let session = sessions
                    .get(session_id)
                    .ok_or_else(acp::Error::invalid_params)?;

                let patches = session.patches.borrow();

                if patches.is_empty() {
                    return Ok(ExtResponse::new(
                        serde_json::value::RawValue::from_string(
                            r#"{"reverted": false, "reason": "no patches available"}"#.to_string(),
                        )
                        .unwrap()
                        .into(),
                    ));
                }

                let revert_all = params.get("all").and_then(|v| v.as_bool()).unwrap_or(false);
                let patch_index = params.get("patch_index").and_then(|v| v.as_u64());

                let patches_to_revert: Vec<Patch> = if revert_all {
                    patches.clone()
                } else if let Some(idx) = patch_index {
                    if (idx as usize) < patches.len() {
                        vec![patches[idx as usize].clone()]
                    } else {
                        return Ok(ExtResponse::new(
                            serde_json::value::RawValue::from_string(
                                r#"{"reverted": false, "reason": "invalid patch index"}"#
                                    .to_string(),
                            )
                            .unwrap()
                            .into(),
                        ));
                    }
                } else {
                    vec![patches.last().unwrap().clone()]
                };

                drop(patches);

                match session.snapshot_manager.revert(&patches_to_revert).await {
                    Ok(()) => {
                        let reverted_count = patches_to_revert.len();
                        let files_reverted: Vec<String> = patches_to_revert
                            .iter()
                            .flat_map(|p| p.files.iter().map(|f| f.display().to_string()))
                            .collect();

                        let mut patches = session.patches.borrow_mut();
                        if revert_all {
                            patches.clear();
                        } else if let Some(idx) = patch_index {
                            patches.remove(idx as usize);
                        } else {
                            patches.pop();
                        }

                        Ok(ExtResponse::new(
                            serde_json::value::RawValue::from_string(
                                serde_json::json!({
                                    "reverted": true,
                                    "patches_reverted": reverted_count,
                                    "files": files_reverted
                                })
                                .to_string(),
                            )
                            .unwrap()
                            .into(),
                        ))
                    }
                    Err(e) => {
                        error!("Failed to revert: {}", e);
                        Ok(ExtResponse::new(
                            serde_json::value::RawValue::from_string(
                                serde_json::json!({
                                    "reverted": false,
                                    "reason": e
                                })
                                .to_string(),
                            )
                            .unwrap()
                            .into(),
                        ))
                    }
                }
            }

            "session/patches" => {
                let params = serde_json::from_str::<serde_json::Value>(args.params.get())
                    .unwrap_or(serde_json::Value::Null);

                let session_id = params
                    .get("session_id")
                    .and_then(|v| v.as_str())
                    .ok_or_else(acp::Error::invalid_params)?;

                let sessions = self.sessions.borrow();
                let session = sessions
                    .get(session_id)
                    .ok_or_else(acp::Error::invalid_params)?;

                let patches = session.patches.borrow();
                let patch_list: Vec<serde_json::Value> = patches.iter().enumerate().map(|(i, p)| {
                    serde_json::json!({
                        "index": i,
                        "hash": &p.hash[..12.min(p.hash.len())],
                        "files": p.files.iter().map(|f| f.display().to_string()).collect::<Vec<_>>()
                    })
                }).collect();

                Ok(ExtResponse::new(
                    serde_json::value::RawValue::from_string(
                        serde_json::json!({ "patches": patch_list }).to_string(),
                    )
                    .unwrap()
                    .into(),
                ))
            }

            _ => Ok(ExtResponse::new(
                serde_json::value::RawValue::from_string("null".to_string())
                    .unwrap()
                    .into(),
            )),
        }
    }

    async fn ext_notification(&self, args: ExtNotification) -> acp::Result<()> {
        info!("ACP ext_notification: {}", args.method);
        Ok(())
    }
}

impl CrowAcpAgent {
    /// Run the agent turn and stream events to ACP client
    async fn run_agent_turn(
        &self,
        session_id: &str,
        acp_session_id: &SessionId,
        cancellation: CancellationToken,
        snapshot_hash: Option<String>,
    ) -> acp::Result<PromptResponse> {
        use crate::agent::{init_coagent_session, RunResult};

        // Get session data we need
        let (mut messages, tools, coagent_tools, coagent_registry, registry, has_coagent) = {
            let sessions = self.sessions.borrow();
            let session = sessions
                .get(session_id)
                .ok_or_else(acp::Error::invalid_params)?;
            let history = session.history.borrow().clone();
            (
                history,
                session.tools.clone(),
                session.coagent_tools.clone(),
                session.coagent_registry.clone(),
                session.registry.clone(),
                session.coagent_registry.is_some(),
            )
        };

        // Initialize coagent messages if we have a coagent
        let mut coagent_messages: Option<Vec<ChatCompletionRequestMessage>> = if has_coagent {
            Some(init_coagent_session(&messages))
        } else {
            None
        };

        // Get coagent registry for tool execution (or fall back to primary registry)
        let coagent_reg = coagent_registry.as_ref().unwrap_or(&registry);

        // Create event channel
        let (event_tx, mut event_rx) = mpsc::unbounded_channel();

        // Take agent out of session for the duration of the run
        let mut agent = {
            let sessions = self.sessions.borrow();
            let session = sessions
                .get(session_id)
                .ok_or_else(acp::Error::invalid_params)?;
            let agent_opt = session.agent.borrow_mut().take();
            agent_opt.ok_or_else(acp::Error::internal_error)?
        };

        // Run agent future
        let agent_fut = agent.run(
            &mut messages,
            &tools,
            &mut coagent_messages,
            &coagent_tools,
            coagent_reg,
            event_tx,
            cancellation.clone(),
        );

        // Event processing future - runs concurrently with agent
        let event_fut = async {
            while let Some(event) = event_rx.recv().await {
                match event {
                    AgentEvent::TextDelta { delta, .. } => {
                        let _ = self
                            .send_update(SessionNotification::new(
                                acp_session_id.clone(),
                                SessionUpdate::AgentMessageChunk(ContentChunk::new(
                                    ContentBlock::Text(TextContent::new(delta)),
                                )),
                            ))
                            .await;
                    }

                    AgentEvent::ThinkingDelta { delta, .. } => {
                        let _ = self
                            .send_update(SessionNotification::new(
                                acp_session_id.clone(),
                                SessionUpdate::AgentThoughtChunk(ContentChunk::new(
                                    ContentBlock::Text(TextContent::new(delta)),
                                )),
                            ))
                            .await;
                    }

                    AgentEvent::ToolCallStart {
                        call_id,
                        tool,
                        arguments,
                        ..
                    } => {
                        let tool_call_id = ToolCallId::from(call_id);
                        let kind = tool_name_to_kind(&tool);
                        let title = format!("Calling {}", tool);

                        let _ = self
                            .send_update(SessionNotification::new(
                                acp_session_id.clone(),
                                SessionUpdate::ToolCall(
                                    ToolCall::new(tool_call_id, title)
                                        .kind(kind)
                                        .status(ToolCallStatus::InProgress)
                                        .raw_input(arguments.clone()),
                                ),
                            ))
                            .await;

                        // Handle todo_write specially
                        if tool == "todo_write" {
                            if let Some(plan) = parse_todo_write_to_plan(&arguments) {
                                let _ = self
                                    .send_update(SessionNotification::new(
                                        acp_session_id.clone(),
                                        SessionUpdate::Plan(plan),
                                    ))
                                    .await;
                            }
                        }
                    }

                    AgentEvent::ToolCallEnd {
                        call_id,
                        tool,
                        output,
                        is_error,
                        ..
                    } => {
                        let tool_call_id = ToolCallId::from(call_id);
                        let status = if is_error {
                            ToolCallStatus::Failed
                        } else {
                            ToolCallStatus::Completed
                        };

                        let _ = self
                            .send_update(SessionNotification::new(
                                acp_session_id.clone(),
                                SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(
                                    tool_call_id,
                                    ToolCallUpdateFields::new()
                                        .status(status)
                                        .content(vec![output.clone().into()]),
                                )),
                            ))
                            .await;

                        // Track file changes for undo
                        if let Some(ref hash) = snapshot_hash {
                            let is_file_modifying =
                                matches!(tool.as_str(), "edit" | "write" | "bash");

                            if is_file_modifying {
                                let sessions = self.sessions.borrow();
                                if let Some(session) = sessions.get(session_id) {
                                    if let Ok(patch) = session.snapshot_manager.patch(hash).await {
                                        if !patch.files.is_empty() {
                                            let patch_index = session.patches.borrow().len();
                                            let files: Vec<String> = patch
                                                .files
                                                .iter()
                                                .map(|f| f.display().to_string())
                                                .collect();
                                            session.patches.borrow_mut().push(patch);

                                            let patch_data = serde_json::json!({
                                                "session_id": session_id,
                                                "patch_index": patch_index,
                                                "files": files,
                                                "tool": tool
                                            });
                                            if let Ok(raw) =
                                                serde_json::value::to_raw_value(&patch_data)
                                            {
                                                let _ = self
                                                    .send_ext_notification(ExtNotification::new(
                                                        "session/patch",
                                                        raw.into(),
                                                    ))
                                                    .await;
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }

                    AgentEvent::Error { error, .. } => {
                        let _ = self
                            .send_update(SessionNotification::new(
                                acp_session_id.clone(),
                                SessionUpdate::AgentMessageChunk(ContentChunk::new(
                                    ContentBlock::Text(TextContent::new(format!(
                                        "Error: {}",
                                        error
                                    ))),
                                )),
                            ))
                            .await;
                    }

                    AgentEvent::CoagentStart { coagent, .. } => {
                        debug!("Coagent {} started", coagent);
                    }

                    AgentEvent::CoagentEnd { coagent, .. } => {
                        debug!("Coagent {} ended", coagent);
                    }

                    AgentEvent::Usage {
                        input_tokens,
                        output_tokens,
                        ..
                    } => {
                        debug!("Token usage: {} in, {} out", input_tokens, output_tokens);
                    }

                    _ => {}
                }
            }
        };

        // Run both concurrently - agent produces events, event_fut consumes them
        let (result, _): (RunResult, ()) = tokio::join!(agent_fut, event_fut);

        // Put agent back into session
        {
            let sessions = self.sessions.borrow();
            if let Some(session) = sessions.get(session_id) {
                *session.agent.borrow_mut() = Some(agent);
                // Update history with final messages
                *session.history.borrow_mut() = messages;
            }
        }

        // Map result to ACP stop reason
        let stop_reason = match result {
            RunResult::Complete { .. } => StopReason::EndTurn,
            RunResult::NeedsInput { .. } => StopReason::EndTurn,
            RunResult::MaxTurns { .. } => StopReason::EndTurn,
            RunResult::Cancelled => StopReason::Cancelled,
            RunResult::Error(_) => StopReason::EndTurn,
        };

        Ok(PromptResponse::new(stop_reason))
    }
}

/// Map tool names to ACP ToolKind for appropriate UI treatment
fn tool_name_to_kind(name: &str) -> ToolKind {
    match name {
        "read_file" => ToolKind::Read,
        "edit_file" => ToolKind::Edit,
        "list_directory" => ToolKind::Read,
        "grep" | "find_path" => ToolKind::Search,
        "terminal" => ToolKind::Execute,
        "thinking" => ToolKind::Think,
        "fetch" | "web_search" => ToolKind::Fetch,
        _ => ToolKind::Other,
    }
}

/// Parse todo_write tool arguments into an ACP Plan
fn parse_todo_write_to_plan(args: &serde_json::Value) -> Option<Plan> {
    let todos = args.get("todos")?.as_array()?;

    let entries: Vec<PlanEntry> = todos
        .iter()
        .filter_map(|todo| {
            let content = todo.get("content")?.as_str()?.to_string();
            let status_str = todo.get("status")?.as_str()?;

            let status = match status_str {
                "pending" => PlanEntryStatus::Pending,
                "in_progress" => PlanEntryStatus::InProgress,
                "completed" => PlanEntryStatus::Completed,
                _ => PlanEntryStatus::Pending,
            };

            Some(PlanEntry::new(content, PlanEntryPriority::Medium, status))
        })
        .collect();

    if entries.is_empty() {
        None
    } else {
        Some(Plan::new(entries))
    }
}

/// Run the ACP server on stdio
pub async fn run_stdio_server(config: Config, telemetry: Arc<Telemetry>) -> acp::Result<()> {
    use acp::Client as _;
    use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

    let outgoing = tokio::io::stdout().compat_write();
    let incoming = tokio::io::stdin().compat();

    // Channel for session notifications
    let (tx, mut rx) = mpsc::unbounded_channel();

    // Create the agent
    let agent = CrowAcpAgent::new(tx, config, telemetry).map_err(|e| {
        error!("Failed to create agent: {}", e);
        acp::Error::internal_error()
    })?;

    // Start the connection
    let (conn, handle_io) = acp::AgentSideConnection::new(agent, outgoing, incoming, |fut| {
        tokio::task::spawn_local(fut);
    });

    // Background task to send notifications
    tokio::task::spawn_local(async move {
        while let Some((notification, tx)) = rx.recv().await {
            let result = match notification {
                OutgoingNotification::Session(session_notif) => {
                    conn.session_notification(session_notif).await
                }
                OutgoingNotification::Extension(ext_notif) => {
                    conn.ext_notification(ext_notif).await
                }
            };
            if let Err(e) = result {
                error!("Failed to send notification: {}", e);
                break;
            }
            tx.send(()).ok();
        }
    });

    // Run until stdin/stdout are closed
    handle_io.await
}
