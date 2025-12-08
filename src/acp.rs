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
use futures::StreamExt;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;

use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};
use tracing::{debug, error, info};

use rig::agent::MultiTurnStreamItem;
use rig::message::{AssistantContent, Message, Text, UserContent};
use rig::streaming::StreamedAssistantContent;
use rig::OneOrMany;
use crate::config::Config;
use crate::hooks::TelemetryHook;
use crate::snapshot::{Patch, SnapshotManager};
use crate::telemetry::Telemetry;
use crate::CrowAgent;

/// Outgoing notifications from agent to client
pub enum OutgoingNotification {
    Session(SessionNotification),
    Extension(ExtNotification),
}

/// Session state for an active ACP session
struct Session {
    agent: CrowAgent,
    history: RefCell<Vec<rig::message::Message>>,
    /// Active hook for the current operation - used for cancellation
    active_hook: RefCell<Option<TelemetryHook>>,
    /// Flag to indicate the session should be cancelled
    cancelled: Cell<bool>,
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
}

impl CrowAcpAgent {
    /// Create a new CrowAcpAgent
    pub fn new(
        notification_tx: mpsc::UnboundedSender<(OutgoingNotification, oneshot::Sender<()>)>,
        config: Config,
        telemetry: Arc<Telemetry>,
    ) -> Self {
        Self {
            notification_tx,
            next_session_id: Cell::new(0),
            sessions: RefCell::new(HashMap::new()),
            config,
            telemetry,
        }
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

        let session_config = Config {
            working_dir,
            ..self.config.clone()
        };

        // Create a new agent for this session
        let agent = CrowAgent::new(session_config.clone(), self.telemetry.clone());

        // Create snapshot manager for this session's working directory
        let snapshot_manager = SnapshotManager::for_directory(session_config.working_dir.clone());

        // Store the session
        self.sessions.borrow_mut().insert(
            session_id_str.clone(),
            Session {
                agent,
                history: RefCell::new(Vec::new()),
                active_hook: RefCell::new(None),
                cancelled: Cell::new(false),
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

        // Get the session and start streaming
        let stream_result = {
            let sessions = self.sessions.borrow();
            let session = sessions
                .get(&session_id)
                .ok_or_else(acp::Error::invalid_params)?;

            // Reset cancelled flag at start of new prompt
            session.cancelled.set(false);

            let history = session.history.borrow().clone();
            session.agent.chat_stream(&prompt_text, history).await
        };

        match stream_result {
            Ok((stream_request, hook)) => {
                // Store the hook for potential cancellation
                {
                    let sessions = self.sessions.borrow();
                    if let Some(session) = sessions.get(&session_id) {
                        *session.active_hook.borrow_mut() = Some(hook.clone());
                    }
                }

                // Await the request to get the actual stream
                let mut stream = stream_request.await;

                // Accumulate EVERYTHING for history - on cancel we need to save all of this
                // This mirrors what rig does internally during multi_turn
                let mut accumulated_text = String::new();
                let mut accumulated_tool_calls: Vec<rig::message::ToolCall> = Vec::new();
                let mut accumulated_tool_results: Vec<rig::message::ToolResult> = Vec::new();

                // Helper to save accumulated state to history
                // ALWAYS saves the user message, even if assistant hadn't responded yet
                let save_history = |sessions: &RefCell<HashMap<String, Session>>,
                                   session_id: &str,
                                   prompt_text: &str,
                                   text: &str,
                                   tool_calls: &[rig::message::ToolCall],
                                   tool_results: &[rig::message::ToolResult]| {
                    let sessions_ref = sessions.borrow();
                    if let Some(session) = sessions_ref.get(session_id) {
                        let mut history = session.history.borrow_mut();

                        // ALWAYS add user message - even if cancelled before assistant responded
                        history.push(Message::User {
                            content: OneOrMany::one(UserContent::text(prompt_text)),
                        });

                        // Build assistant content - can have both text and tool calls
                        let mut assistant_content: Vec<AssistantContent> = Vec::new();

                        if !text.is_empty() {
                            assistant_content.push(AssistantContent::Text(Text {
                                text: text.to_string(),
                            }));
                        }

                        for tc in tool_calls {
                            assistant_content.push(AssistantContent::ToolCall(tc.clone()));
                        }

                        // Only add assistant message if there's content
                        if !assistant_content.is_empty() {
                            history.push(Message::Assistant {
                                id: None,
                                content: OneOrMany::many(assistant_content)
                                    .expect("we checked it's not empty"),
                            });
                        }

                        // Add tool results as user messages
                        for result in tool_results {
                            history.push(Message::User {
                                content: OneOrMany::one(UserContent::ToolResult(result.clone())),
                            });
                        }

                        info!("Saved history: {} chars text, {} tool calls, {} results, now {} messages",
                              text.len(), tool_calls.len(), tool_results.len(), history.len());
                    }
                };

                // Process stream items and send updates
                while let Some(item) = stream.next().await {
                    // Check if cancelled
                    {
                        let sessions = self.sessions.borrow();
                        if let Some(session) = sessions.get(&session_id) {
                            if session.cancelled.get() {
                                info!("Session {} was cancelled, breaking stream loop", session_id);
                                *session.active_hook.borrow_mut() = None;
                                drop(sessions);

                                // Save everything we accumulated before cancellation
                                save_history(
                                    &self.sessions, &session_id, &prompt_text,
                                    &accumulated_text, &accumulated_tool_calls, &accumulated_tool_results
                                );

                                return Ok(PromptResponse::new(StopReason::Cancelled));
                            }
                        }
                    }

                    match item {
                        Ok(MultiTurnStreamItem::StreamAssistantItem(content)) => {
                            match content {
                                StreamedAssistantContent::Text(text) => {
                                    // Accumulate text for history
                                    accumulated_text.push_str(&text.text);
                                    debug!("Accumulated {} chars (total: {})", text.text.len(), accumulated_text.len());

                                    // Stream text chunks immediately
                                    self.send_update(SessionNotification::new(
                                        args.session_id.clone(),
                                        SessionUpdate::AgentMessageChunk(ContentChunk::new(
                                            ContentBlock::Text(TextContent::new(text.text)),
                                        )),
                                    ))
                                    .await?;
                                }
                                StreamedAssistantContent::ToolCall(tool_call) => {
                                    // Accumulate tool call for history
                                    accumulated_tool_calls.push(tool_call.clone());

                                    // Send tool call notification
                                    let tool_call_id = ToolCallId::from(tool_call.id.clone());
                                    let kind = tool_name_to_kind(&tool_call.function.name);
                                    let title = format!("Calling {}", tool_call.function.name);

                                    self.send_update(SessionNotification::new(
                                        args.session_id.clone(),
                                        SessionUpdate::ToolCall(
                                            ToolCall::new(tool_call_id, title)
                                                .kind(kind)
                                                .status(ToolCallStatus::InProgress)
                                                .raw_input(tool_call.function.arguments.clone()),
                                        ),
                                    ))
                                    .await?;

                                    // If this is todo_write, also send a Plan update
                                    if tool_call.function.name == "todo_write" {
                                        if let Some(plan) =
                                            parse_todo_write_to_plan(&tool_call.function.arguments)
                                        {
                                            self.send_update(SessionNotification::new(
                                                args.session_id.clone(),
                                                SessionUpdate::Plan(plan),
                                            ))
                                            .await?;
                                        }
                                    }
                                }
                                StreamedAssistantContent::Reasoning(reasoning) => {
                                    // Send reasoning as thought chunk
                                    let text = reasoning.reasoning.join("");
                                    self.send_update(SessionNotification::new(
                                        args.session_id.clone(),
                                        SessionUpdate::AgentThoughtChunk(ContentChunk::new(
                                            ContentBlock::Text(TextContent::new(text)),
                                        )),
                                    ))
                                    .await?;
                                }
                                StreamedAssistantContent::ToolCallDelta { .. } => {
                                    // Skip tool call deltas - we handle complete tool calls
                                }
                                StreamedAssistantContent::Final(_) => {
                                    // Final response handled separately
                                }
                            }
                        }
                        Ok(MultiTurnStreamItem::StreamUserItem(user_content)) => {
                            // Tool results - accumulate AND send as tool call update
                            let rig::streaming::StreamedUserContent::ToolResult(result) =
                                user_content;

                            // Find the tool call that corresponds to this result
                            let tool_name = accumulated_tool_calls
                                .iter()
                                .find(|tc| tc.id == result.id)
                                .map(|tc| tc.function.name.clone());

                            // Accumulate for history
                            accumulated_tool_results.push(result.clone());

                            let tool_call_id = ToolCallId::from(result.id.clone());
                            let result_text = result
                                .content
                                .iter()
                                .filter_map(|c| {
                                    if let rig::message::ToolResultContent::Text(t) = c {
                                        Some(t.text.clone())
                                    } else {
                                        None
                                    }
                                })
                                .collect::<Vec<_>>()
                                .join("\n");

                            self.send_update(SessionNotification::new(
                                args.session_id.clone(),
                                SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(
                                    tool_call_id,
                                    ToolCallUpdateFields::new()
                                        .status(ToolCallStatus::Completed)
                                        .content(vec![result_text.into()]),
                                )),
                            ))
                            .await?;

                            // Check if this was a file-modifying tool and track changes
                            if let Some(ref name) = tool_name {
                                let is_file_modifying = matches!(
                                    name.as_str(),
                                    "edit_file" | "write" | "terminal" | "bash"
                                );

                                if is_file_modifying {
                                    if let Some(ref hash) = snapshot_hash {
                                        // Get patch info and store it
                                        let patch_info = {
                                            let sessions = self.sessions.borrow();
                                            if let Some(session) = sessions.get(&session_id) {
                                                if let Ok(patch) = session.snapshot_manager.patch(hash).await {
                                                    if !patch.files.is_empty() {
                                                        info!(
                                                            "Tool {} modified {} files",
                                                            name,
                                                            patch.files.len()
                                                        );
                                                        let patch_index = session.patches.borrow().len();
                                                        let files: Vec<String> = patch.files.iter()
                                                            .map(|f| f.display().to_string())
                                                            .collect();
                                                        // Store patch for potential undo
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

                                        // Send extension notification about the patch (outside the borrow)
                                        if let Some((patch_index, files)) = patch_info {
                                            let patch_data = serde_json::json!({
                                                "session_id": session_id,
                                                "patch_index": patch_index,
                                                "files": files,
                                                "tool": name
                                            });
                                            if let Ok(raw) = serde_json::value::to_raw_value(&patch_data) {
                                                let ext_notif = ExtNotification::new(
                                                    "session/patch",
                                                    raw.into()
                                                );
                                                // Fire and forget - don't block on this
                                                let _ = self.send_ext_notification(ext_notif).await;
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        Ok(MultiTurnStreamItem::FinalResponse(final_resp)) => {
                            // Stream completed successfully - update history
                            info!("Stream completed, updating history");

                            // Use the final response text if available, otherwise use accumulated
                            let response_text = if !final_resp.response().is_empty() {
                                final_resp.response().to_string()
                            } else {
                                accumulated_text.clone()
                            };

                            // Update session history
                            let sessions = self.sessions.borrow();
                            if let Some(session) = sessions.get(&session_id) {
                                let mut history = session.history.borrow_mut();

                                // Add user message
                                history.push(Message::User {
                                    content: OneOrMany::one(UserContent::text(&prompt_text)),
                                });

                                // Add assistant response (final response is just text, tool calls already handled in multi-turn)
                                history.push(Message::Assistant {
                                    id: None,
                                    content: OneOrMany::one(AssistantContent::Text(Text {
                                        text: response_text,
                                    })),
                                });

                                info!("History updated, now has {} messages", history.len());
                                *session.active_hook.borrow_mut() = None;
                            }
                        }
                        Ok(_) => {
                            // Handle any other MultiTurnStreamItem variants
                        }
                        Err(e) => {
                            // Check if this was a cancellation
                            let is_cancelled = e.to_string().contains("PromptCancelled");

                            if is_cancelled {
                                info!("Stream was cancelled via error");
                                {
                                    let sessions = self.sessions.borrow();
                                    if let Some(session) = sessions.get(&session_id) {
                                        *session.active_hook.borrow_mut() = None;
                                    }
                                }

                                // Save everything we accumulated
                                save_history(
                                    &self.sessions, &session_id, &prompt_text,
                                    &accumulated_text, &accumulated_tool_calls, &accumulated_tool_results
                                );

                                return Ok(PromptResponse::new(StopReason::Cancelled));
                            }

                            error!("Stream error: {}", e);
                            self.send_update(SessionNotification::new(
                                args.session_id.clone(),
                                SessionUpdate::AgentMessageChunk(ContentChunk::new(
                                    ContentBlock::Text(TextContent::new(format!("Error: {}", e))),
                                )),
                            ))
                            .await?;

                            {
                                let sessions = self.sessions.borrow();
                                if let Some(session) = sessions.get(&session_id) {
                                    *session.active_hook.borrow_mut() = None;
                                }
                            }

                            // Save everything we accumulated before error
                            save_history(
                                &self.sessions, &session_id, &prompt_text,
                                &accumulated_text, &accumulated_tool_calls, &accumulated_tool_results
                            );

                            return Ok(PromptResponse::new(StopReason::Refusal));
                        }
                    }
                }

                Ok(PromptResponse::new(StopReason::EndTurn))
            }
            Err(e) => {
                error!("Prompt error: {}", e);

                let sessions = self.sessions.borrow();
                if let Some(session) = sessions.get(&session_id) {
                    *session.active_hook.borrow_mut() = None;
                }
                drop(sessions);

                self.send_update(SessionNotification::new(
                    args.session_id.clone(),
                    SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(
                        TextContent::new(format!("Error: {}", e)),
                    ))),
                ))
                .await?;

                Ok(PromptResponse::new(StopReason::Refusal))
            }
        }
    }

    async fn cancel(&self, args: CancelNotification) -> acp::Result<()> {
        let session_id = args.session_id.0.to_string();
        info!("ACP cancel: session_id={}", session_id);

        // Set cancelled flag and trigger hook cancel
        let sessions = self.sessions.borrow();
        if let Some(session) = sessions.get(&session_id) {
            // Set the cancelled flag - this will be checked in the stream loop
            session.cancelled.set(true);
            info!("Set cancelled flag for session {}", session_id);

            // Also trigger the hook's cancel signal for interrupting tool execution
            let hook = session.active_hook.borrow();
            if let Some(ref h) = *hook {
                info!("Triggering cancel signal for session {}", session_id);
                h.cancel().await;
            }
        } else {
            info!("Session {} not found for cancel", session_id);
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
                // Expected params: { "session_id": "...", "patch_index": N } or { "session_id": "...", "all": true }
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
                    info!("No patches to revert for session {}", session_id);
                    return Ok(ExtResponse::new(
                        serde_json::value::RawValue::from_string(
                            r#"{"reverted": false, "reason": "no patches available"}"#.to_string()
                        ).unwrap().into(),
                    ));
                }

                // Check if we're reverting all or just the last one
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
                    // Default: revert the last patch
                    vec![patches.last().unwrap().clone()]
                };

                drop(patches);

                // Perform the revert
                match session.snapshot_manager.revert(&patches_to_revert).await {
                    Ok(()) => {
                        let reverted_count = patches_to_revert.len();
                        let files_reverted: Vec<String> = patches_to_revert
                            .iter()
                            .flat_map(|p| p.files.iter().map(|f| f.display().to_string()))
                            .collect();

                        info!("Reverted {} patches ({} files) for session {}",
                              reverted_count, files_reverted.len(), session_id);

                        // Remove reverted patches from the list
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
                // List available patches for a session
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
                // Unknown extension method
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
    let agent = CrowAcpAgent::new(tx, config, telemetry);

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
