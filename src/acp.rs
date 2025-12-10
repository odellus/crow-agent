//! ACP (Agent Client Protocol) server implementation
//!
//! This module implements the ACP Agent trait to allow crow_agent to be used
//! as an external agent server with Zed or other ACP-compatible clients.
//!
//! The server communicates over stdio using JSON-RPC 2.0.

use agent_client_protocol::{
    self as acp, AgentCapabilities, AuthenticateRequest, AuthenticateResponse,
    CancelNotification, ContentBlock, ContentChunk, ExtNotification, ExtRequest, ExtResponse,
    Implementation, InitializeRequest, InitializeResponse, LoadSessionRequest, LoadSessionResponse,
    NewSessionRequest, NewSessionResponse, Plan, PlanEntry, PlanEntryPriority, PlanEntryStatus,
    PromptCapabilities, PromptRequest, PromptResponse, ProtocolVersion, SessionId,
    SessionNotification, SessionUpdate, SetSessionModeRequest, SetSessionModeResponse, StopReason,
    TextContent, ToolCall, ToolCallId, ToolCallStatus, ToolCallUpdate, ToolCallUpdateFields,
    ToolKind,
};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info};

use async_openai::types::{
    ChatCompletionRequestMessage, ChatCompletionRequestSystemMessageArgs,
    ChatCompletionRequestUserMessageArgs, ChatCompletionRequestAssistantMessageArgs,
    ChatCompletionRequestToolMessageArgs, ChatCompletionTool,
};

use crate::agent::{AgentConfig, BaseAgent};
use crate::config::Config;
use crate::events::{AgentEvent, TurnCompleteReason};
use crate::provider::{ProviderClient, ProviderConfig};
use crate::snapshot::{Patch, SnapshotManager};
use crate::telemetry::Telemetry;
use crate::tool::ToolRegistry;
use crate::tools2;

/// Outgoing notifications from agent to client
pub enum OutgoingNotification {
    Session(SessionNotification),
    Extension(ExtNotification),
}

/// Session state for an active ACP session
struct Session {
    /// The base agent for this session
    agent: BaseAgent,
    /// Tool registry
    registry: ToolRegistry,
    /// OpenAI-format tools for LLM
    tools: Vec<ChatCompletionTool>,
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
        let base_url = config.llm.base_url.as_deref().unwrap_or("http://localhost:1234/v1");
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

    /// Get default system prompt
    fn system_prompt(&self) -> String {
        r#"You are Crow, a helpful software engineering assistant.

You have access to tools to help accomplish tasks. When you're done with a task, call task_complete with a summary.

Be concise and direct. Focus on solving the user's problem efficiently."#
            .to_string()
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
            .agent_info(Implementation::new(
                "crow-agent",
                env!("CARGO_PKG_VERSION"),
            )))
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

        // Create agent config
        let mut agent_config = AgentConfig::new("crow");
        agent_config.model = Some(self.config.llm.model.clone());

        // Create the base agent
        let agent = BaseAgent::with_telemetry(
            agent_config,
            self.provider.clone(),
            working_dir.clone(),
            self.telemetry.clone(),
        );

        // Create tool registry
        let registry = tools2::create_registry(working_dir.clone());
        let tools = registry.to_openai_tools();

        // Create snapshot manager for this session's working directory
        let snapshot_manager = SnapshotManager::for_directory(working_dir);

        // Initialize history with system prompt
        let history = vec![
            ChatCompletionRequestSystemMessageArgs::default()
                .content(self.system_prompt())
                .build()
                .map_err(|_| acp::Error::internal_error())?
                .into(),
        ];

        // Store the session
        self.sessions.borrow_mut().insert(
            session_id_str.clone(),
            Session {
                agent,
                registry,
                tools,
                history: RefCell::new(history),
                cancellation: RefCell::new(None),
                snapshot_manager,
                current_snapshot: RefCell::new(None),
                patches: RefCell::new(Vec::new()),
            },
        );

        Ok(NewSessionResponse::new(SessionId::new(session_id_str)))
    }

    async fn load_session(&self, _args: LoadSessionRequest) -> acp::Result<LoadSessionResponse> {
        // Session persistence not yet implemented
        Err(acp::Error::method_not_found())
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
        let result = self.run_agent_turn(&session_id, &args.session_id, cancellation, snapshot_hash).await;

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

    async fn set_session_mode(
        &self,
        _args: SetSessionModeRequest,
    ) -> acp::Result<SetSessionModeResponse> {
        // No modes supported yet
        Err(acp::Error::method_not_found())
    }

    async fn ext_method(&self, args: ExtRequest) -> acp::Result<ExtResponse> {
        info!("ACP ext_method: {}", args.method);

        match &*args.method {
            "session/revert" => {
                // Revert to a previous snapshot
                let params = serde_json::from_str::<serde_json::Value>(args.params.get())
                    .unwrap_or(serde_json::Value::Null);

                let session_id = params.get("session_id")
                    .and_then(|v| v.as_str())
                    .ok_or_else(acp::Error::invalid_params)?;

                let sessions = self.sessions.borrow();
                let session = sessions.get(session_id)
                    .ok_or_else(acp::Error::invalid_params)?;

                let patches = session.patches.borrow();

                if patches.is_empty() {
                    return Ok(ExtResponse::new(
                        serde_json::value::RawValue::from_string(
                            r#"{"reverted": false, "reason": "no patches available"}"#.to_string()
                        ).unwrap().into(),
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
                                r#"{"reverted": false, "reason": "invalid patch index"}"#.to_string()
                            ).unwrap().into(),
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
                                }).to_string()
                            ).unwrap().into(),
                        ))
                    }
                    Err(e) => {
                        error!("Failed to revert: {}", e);
                        Ok(ExtResponse::new(
                            serde_json::value::RawValue::from_string(
                                serde_json::json!({
                                    "reverted": false,
                                    "reason": e
                                }).to_string()
                            ).unwrap().into(),
                        ))
                    }
                }
            }

            "session/patches" => {
                let params = serde_json::from_str::<serde_json::Value>(args.params.get())
                    .unwrap_or(serde_json::Value::Null);

                let session_id = params.get("session_id")
                    .and_then(|v| v.as_str())
                    .ok_or_else(acp::Error::invalid_params)?;

                let sessions = self.sessions.borrow();
                let session = sessions.get(session_id)
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
                        serde_json::json!({ "patches": patch_list }).to_string()
                    ).unwrap().into(),
                ))
            }

            _ => {
                Ok(ExtResponse::new(
                    serde_json::value::RawValue::from_string("null".to_string())
                        .unwrap()
                        .into(),
                ))
            }
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
        // Get session data we need
        let (agent, registry, tools, messages) = {
            let sessions = self.sessions.borrow();
            let session = sessions.get(session_id)
                .ok_or_else(acp::Error::invalid_params)?;
            let history = session.history.borrow().clone();
            (
                session.agent.clone(),
                session.registry.clone(),
                session.tools.clone(),
                history,
            )
        };

        // Create event channel
        let (event_tx, mut event_rx) = mpsc::unbounded_channel();

        // Run agent turn in background
        let agent_clone = agent.clone();
        let registry_clone = registry.clone();
        let tools_clone = tools.clone();
        let cancel_clone = cancellation.clone();

        let turn_handle = tokio::task::spawn_local(async move {
            agent_clone.execute_turn(
                &mut messages.clone(),
                &tools_clone,
                &registry_clone,
                &event_tx,
                cancel_clone,
            ).await
        });

        // Process events and send to ACP client
        let mut final_reason = TurnCompleteReason::TextResponse;
        let mut accumulated_text = String::new();

        while let Some(event) = event_rx.recv().await {
            match event {
                AgentEvent::TextDelta { delta, .. } => {
                    accumulated_text.push_str(&delta);
                    self.send_update(SessionNotification::new(
                        acp_session_id.clone(),
                        SessionUpdate::AgentMessageChunk(ContentChunk::new(
                            ContentBlock::Text(TextContent::new(delta)),
                        )),
                    )).await?;
                }

                AgentEvent::ThinkingDelta { delta, .. } => {
                    self.send_update(SessionNotification::new(
                        acp_session_id.clone(),
                        SessionUpdate::AgentThoughtChunk(ContentChunk::new(
                            ContentBlock::Text(TextContent::new(delta)),
                        )),
                    )).await?;
                }

                AgentEvent::ToolCallStart { call_id, tool, arguments, .. } => {
                    let tool_call_id = ToolCallId::from(call_id);
                    let kind = tool_name_to_kind(&tool);
                    let title = format!("Calling {}", tool);

                    self.send_update(SessionNotification::new(
                        acp_session_id.clone(),
                        SessionUpdate::ToolCall(
                            ToolCall::new(tool_call_id, title)
                                .kind(kind)
                                .status(ToolCallStatus::InProgress)
                                .raw_input(arguments.clone()),
                        ),
                    )).await?;

                    // Handle todo_write specially
                    if tool == "todo_write" {
                        if let Some(plan) = parse_todo_write_to_plan(&arguments) {
                            self.send_update(SessionNotification::new(
                                acp_session_id.clone(),
                                SessionUpdate::Plan(plan),
                            )).await?;
                        }
                    }
                }

                AgentEvent::ToolCallEnd { call_id, tool, output, is_error, .. } => {
                    let tool_call_id = ToolCallId::from(call_id);
                    let status = if is_error {
                        ToolCallStatus::Failed
                    } else {
                        ToolCallStatus::Completed
                    };

                    self.send_update(SessionNotification::new(
                        acp_session_id.clone(),
                        SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(
                            tool_call_id,
                            ToolCallUpdateFields::new()
                                .status(status)
                                .content(vec![output.clone().into()]),
                        )),
                    )).await?;

                    // Track file changes for undo
                    if let Some(ref hash) = snapshot_hash {
                        let is_file_modifying = matches!(
                            tool.as_str(),
                            "edit_file" | "write" | "terminal" | "bash"
                        );

                        if is_file_modifying {
                            let patch_info = {
                                let sessions = self.sessions.borrow();
                                if let Some(session) = sessions.get(session_id) {
                                    if let Ok(patch) = session.snapshot_manager.patch(hash).await {
                                        if !patch.files.is_empty() {
                                            let patch_index = session.patches.borrow().len();
                                            let files: Vec<String> = patch.files.iter()
                                                .map(|f| f.display().to_string())
                                                .collect();
                                            session.patches.borrow_mut().push(patch);
                                            Some((patch_index, files))
                                        } else {
                                            None
                                        }
                                    } else {
                                        None
                                    }
                                } else {
                                    None
                                }
                            };

                            if let Some((patch_index, files)) = patch_info {
                                let patch_data = serde_json::json!({
                                    "session_id": session_id,
                                    "patch_index": patch_index,
                                    "files": files,
                                    "tool": tool
                                });
                                if let Ok(raw) = serde_json::value::to_raw_value(&patch_data) {
                                    let _ = self.send_ext_notification(
                                        ExtNotification::new("session/patch", raw.into())
                                    ).await;
                                }
                            }
                        }
                    }
                }

                AgentEvent::TurnComplete { reason, .. } => {
                    final_reason = reason;
                }

                AgentEvent::Cancelled { .. } => {
                    final_reason = TurnCompleteReason::Cancelled;
                }

                AgentEvent::Error { error, .. } => {
                    self.send_update(SessionNotification::new(
                        acp_session_id.clone(),
                        SessionUpdate::AgentMessageChunk(ContentChunk::new(
                            ContentBlock::Text(TextContent::new(format!("Error: {}", error))),
                        )),
                    )).await?;
                }

                AgentEvent::Usage { input_tokens, output_tokens, .. } => {
                    debug!("Token usage: {} in, {} out", input_tokens, output_tokens);
                }

                _ => {}
            }
        }

        // Wait for turn to complete
        let turn_result = turn_handle.await
            .map_err(|_| acp::Error::internal_error())?
            .map_err(|e| {
                error!("Turn failed: {}", e);
                acp::Error::internal_error()
            })?;

        // Update history with the turn result
        {
            let sessions = self.sessions.borrow();
            if let Some(session) = sessions.get(session_id) {
                let mut history = session.history.borrow_mut();

                // Add assistant message with text and/or tool calls
                if let Some(ref text) = turn_result.text {
                    let assistant_msg = ChatCompletionRequestAssistantMessageArgs::default()
                        .content(text.clone())
                        .build()
                        .map_err(|_| acp::Error::internal_error())?;
                    history.push(assistant_msg.into());
                }

                // Add tool results
                for tc in &turn_result.tool_calls {
                    let tool_msg = ChatCompletionRequestToolMessageArgs::default()
                        .tool_call_id(&tc.id)
                        .content(tc.output.clone())
                        .build()
                        .map_err(|_| acp::Error::internal_error())?;
                    history.push(tool_msg.into());
                }

                info!("History updated, now has {} messages", history.len());
            }
        }

        // Map to ACP stop reason
        let stop_reason = match final_reason {
            TurnCompleteReason::TextResponse => StopReason::EndTurn,
            TurnCompleteReason::TaskComplete { .. } => StopReason::EndTurn,
            TurnCompleteReason::MaxIterations => StopReason::EndTurn,
            TurnCompleteReason::Cancelled => StopReason::Cancelled,
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
    let agent = CrowAcpAgent::new(tx, config, telemetry)
        .map_err(|e| {
            error!("Failed to create agent: {}", e);
            acp::Error::internal_error()
        })?;

    // Start the connection
    let (conn, handle_io) =
        acp::AgentSideConnection::new(agent, outgoing, incoming, |fut| {
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
