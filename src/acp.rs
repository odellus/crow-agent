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
    NewSessionRequest, NewSessionResponse, PromptCapabilities, PromptRequest, PromptResponse,
    ProtocolVersion, SessionId, SessionNotification, SessionUpdate, SetSessionModeRequest,
    SetSessionModeResponse, StopReason, TextContent,
};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;

use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};
use tracing::{error, info};

use crate::config::Config;
use crate::telemetry::Telemetry;
use crate::CrowAgent;

/// Session state for an active ACP session
struct Session {
    agent: CrowAgent,
    history: RefCell<Vec<rig::message::Message>>,
}

/// Crow's ACP Agent implementation
pub struct CrowAcpAgent {
    /// Channel for sending session notifications back to the client
    session_update_tx: mpsc::UnboundedSender<(SessionNotification, oneshot::Sender<()>)>,
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
        session_update_tx: mpsc::UnboundedSender<(SessionNotification, oneshot::Sender<()>)>,
        config: Config,
        telemetry: Arc<Telemetry>,
    ) -> Self {
        Self {
            session_update_tx,
            next_session_id: Cell::new(0),
            sessions: RefCell::new(HashMap::new()),
            config,
            telemetry,
        }
    }

    /// Send a session update notification and wait for acknowledgment
    async fn send_update(&self, notification: SessionNotification) -> acp::Result<()> {
        let (tx, rx) = oneshot::channel();
        self.session_update_tx
            .send((notification, tx))
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
        let agent = CrowAgent::new(session_config, self.telemetry.clone());

        // Store the session
        self.sessions.borrow_mut().insert(
            session_id_str.clone(),
            Session {
                agent,
                history: RefCell::new(Vec::new()),
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

        // Get the session and run the prompt
        let response = {
            let sessions = self.sessions.borrow();
            let session = sessions.get(&session_id).ok_or_else(acp::Error::invalid_params)?;

            let mut history = session.history.borrow_mut();
            session.agent.chat(&prompt_text, &mut history).await
        };

        match response {
            Ok(response_text) => {
                // Stream the response back as a chunk
                self.send_update(SessionNotification::new(
                    args.session_id.clone(),
                    SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(
                        TextContent::new(response_text),
                    ))),
                ))
                .await?;

                Ok(PromptResponse::new(StopReason::EndTurn))
            }
            Err(e) => {
                error!("Prompt error: {}", e);

                // Send error as a message
                self.send_update(SessionNotification::new(
                    args.session_id.clone(),
                    SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(
                        TextContent::new(format!("Error: {}", e)),
                    ))),
                ))
                .await?;

                // Return Refusal for errors
                Ok(PromptResponse::new(StopReason::Refusal))
            }
        }
    }

    async fn cancel(&self, args: CancelNotification) -> acp::Result<()> {
        info!("ACP cancel: session_id={}", args.session_id.0);
        // TODO: Implement cancellation
        // For now, just acknowledge
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
        // No extension methods supported
        Ok(ExtResponse::new(
            serde_json::value::RawValue::from_string("null".to_string())
                .unwrap()
                .into(),
        ))
    }

    async fn ext_notification(&self, args: ExtNotification) -> acp::Result<()> {
        info!("ACP ext_notification: {}", args.method);
        Ok(())
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

    // Background task to send session notifications
    tokio::task::spawn_local(async move {
        while let Some((notification, tx)) = rx.recv().await {
            if let Err(e) = conn.session_notification(notification).await {
                error!("Failed to send session notification: {}", e);
                break;
            }
            tx.send(()).ok();
        }
    });

    // Run until stdin/stdout are closed
    handle_io.await
}
