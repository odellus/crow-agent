//! Telemetry and logging infrastructure
//!
//! Provides full observability for LLM interactions:
//! - SQLite storage for all interactions (queryable history)
//! - OpenTelemetry export (Jaeger, Honeycomb, etc.)
//! - Console logging (human-readable)
//! - JSON file logging (for analysis)

use crate::trace_layer::SqliteTraceLayer;
use chrono::{DateTime, Utc};
use opentelemetry::trace::TracerProvider;
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::trace::SdkTracerProvider;
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};
use uuid::Uuid;

/// A single interaction record for telemetry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InteractionRecord {
    pub id: Uuid,
    pub session_id: Uuid,
    pub timestamp: DateTime<Utc>,
    pub interaction_type: InteractionType,
    pub duration_ms: Option<u64>,
    pub prompt_tokens: Option<u32>,
    pub completion_tokens: Option<u32>,
    pub total_tokens: Option<u32>,
    pub model: Option<String>,
    pub content: Option<String>,
    pub raw_request: Option<String>,
    pub raw_response: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum InteractionType {
    UserMessage,
    AssistantMessage,
    ToolCall,
    ToolResult,
    SystemPrompt,
}

impl InteractionType {
    fn as_str(&self) -> &'static str {
        match self {
            InteractionType::UserMessage => "user_message",
            InteractionType::AssistantMessage => "assistant_message",
            InteractionType::ToolCall => "tool_call",
            InteractionType::ToolResult => "tool_result",
            InteractionType::SystemPrompt => "system_prompt",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenCount {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallRecord {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
    pub result: Option<String>,
    pub error: Option<String>,
    pub duration_ms: u64,
}

/// Session state for telemetry
#[derive(Debug)]
pub struct TelemetrySession {
    pub id: Uuid,
    pub started_at: DateTime<Utc>,
    pub interaction_count: AtomicU64,
    pub total_tokens: AtomicU64,
}

impl TelemetrySession {
    pub fn new() -> Self {
        Self {
            id: Uuid::new_v4(),
            started_at: Utc::now(),
            interaction_count: AtomicU64::new(0),
            total_tokens: AtomicU64::new(0),
        }
    }

    pub fn next_interaction(&self) -> u64 {
        self.interaction_count.fetch_add(1, Ordering::SeqCst)
    }

    pub fn add_tokens(&self, count: u64) {
        self.total_tokens.fetch_add(count, Ordering::SeqCst);
    }
}

impl Default for TelemetrySession {
    fn default() -> Self {
        Self::new()
    }
}

/// SQLite-backed telemetry storage
struct TelemetryDb {
    conn: Connection,
}

impl TelemetryDb {
    fn new(path: &PathBuf) -> anyhow::Result<Self> {
        let conn = Connection::open(path)?;

        // Create tables
        conn.execute_batch(r#"
            CREATE TABLE IF NOT EXISTS sessions (
                id TEXT PRIMARY KEY,
                started_at TEXT NOT NULL,
                working_dir TEXT,
                model TEXT,
                provider TEXT
            );

            CREATE TABLE IF NOT EXISTS interactions (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                timestamp TEXT NOT NULL,
                interaction_type TEXT NOT NULL,
                duration_ms INTEGER,
                prompt_tokens INTEGER,
                completion_tokens INTEGER,
                total_tokens INTEGER,
                model TEXT,
                content TEXT,
                raw_request TEXT,
                raw_response TEXT,
                error TEXT,
                FOREIGN KEY (session_id) REFERENCES sessions(id)
            );

            CREATE TABLE IF NOT EXISTS tool_calls (
                id TEXT PRIMARY KEY,
                interaction_id TEXT NOT NULL,
                session_id TEXT NOT NULL,
                timestamp TEXT NOT NULL,
                tool_name TEXT NOT NULL,
                arguments TEXT,
                result TEXT,
                error TEXT,
                duration_ms INTEGER,
                FOREIGN KEY (interaction_id) REFERENCES interactions(id),
                FOREIGN KEY (session_id) REFERENCES sessions(id)
            );

            -- Langfuse-level traces: full LLM call records
            CREATE TABLE IF NOT EXISTS traces (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                started_at TEXT NOT NULL,
                completed_at TEXT,
                model_provider TEXT NOT NULL,
                model_id TEXT NOT NULL,
                -- Full request data
                request_messages TEXT NOT NULL,
                request_tools TEXT,
                -- Full response data
                response_content TEXT,
                response_tool_calls TEXT,
                -- Token usage
                input_tokens INTEGER,
                output_tokens INTEGER,
                total_tokens INTEGER,
                -- Timing
                latency_ms INTEGER,
                -- Error info
                error TEXT,
                FOREIGN KEY (session_id) REFERENCES sessions(id)
            );

            CREATE INDEX IF NOT EXISTS idx_interactions_session ON interactions(session_id);
            CREATE INDEX IF NOT EXISTS idx_interactions_type ON interactions(interaction_type);
            CREATE INDEX IF NOT EXISTS idx_tool_calls_session ON tool_calls(session_id);
            CREATE INDEX IF NOT EXISTS idx_tool_calls_name ON tool_calls(tool_name);
            CREATE INDEX IF NOT EXISTS idx_traces_session ON traces(session_id);
            CREATE INDEX IF NOT EXISTS idx_traces_started ON traces(started_at);
        "#)?;

        Ok(Self { conn })
    }

    fn insert_session(&self, session: &TelemetrySession, working_dir: Option<&str>, model: Option<&str>, provider: Option<&str>) -> anyhow::Result<()> {
        self.conn.execute(
            "INSERT INTO sessions (id, started_at, working_dir, model, provider) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                session.id.to_string(),
                session.started_at.to_rfc3339(),
                working_dir,
                model,
                provider
            ],
        )?;
        Ok(())
    }

    fn insert_interaction(&self, record: &InteractionRecord) -> anyhow::Result<()> {
        self.conn.execute(
            r#"INSERT INTO interactions
               (id, session_id, timestamp, interaction_type, duration_ms,
                prompt_tokens, completion_tokens, total_tokens, model,
                content, raw_request, raw_response, error)
               VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)"#,
            params![
                record.id.to_string(),
                record.session_id.to_string(),
                record.timestamp.to_rfc3339(),
                record.interaction_type.as_str(),
                record.duration_ms,
                record.prompt_tokens,
                record.completion_tokens,
                record.total_tokens,
                record.model,
                record.content,
                record.raw_request,
                record.raw_response,
                record.error
            ],
        )?;
        Ok(())
    }

    fn insert_tool_call(&self, interaction_id: Uuid, session_id: Uuid, record: &ToolCallRecord) -> anyhow::Result<()> {
        self.conn.execute(
            r#"INSERT INTO tool_calls
               (id, interaction_id, session_id, timestamp, tool_name, arguments, result, error, duration_ms)
               VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)"#,
            params![
                record.id,
                interaction_id.to_string(),
                session_id.to_string(),
                Utc::now().to_rfc3339(),
                record.name,
                record.arguments.to_string(),
                record.result,
                record.error,
                record.duration_ms as i64
            ],
        )?;
        Ok(())
    }

    fn insert_trace(&self, trace: &TraceRecord) -> anyhow::Result<()> {
        // First ensure we have the agent_name column (migration)
        let _ = self.conn.execute(
            "ALTER TABLE traces ADD COLUMN agent_name TEXT DEFAULT 'unknown'",
            [],
        );

        // Use INSERT OR REPLACE so flush() can be called multiple times
        self.conn.execute(
            r#"INSERT OR REPLACE INTO traces
               (id, session_id, agent_name, started_at, completed_at, model_provider, model_id,
                request_messages, request_tools, response_content, response_tool_calls,
                input_tokens, output_tokens, total_tokens, latency_ms, error)
               VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)"#,
            params![
                trace.id.to_string(),
                trace.session_id.to_string(),
                trace.agent_name,
                trace.started_at.to_rfc3339(),
                trace.completed_at.map(|t| t.to_rfc3339()),
                trace.model_provider,
                trace.model_id,
                trace.request_messages,
                trace.request_tools,
                trace.response_content,
                trace.response_tool_calls,
                trace.input_tokens.map(|t| t as i64),
                trace.output_tokens.map(|t| t as i64),
                trace.total_tokens.map(|t| t as i64),
                trace.latency_ms.map(|t| t as i64),
                trace.error
            ],
        )?;
        Ok(())
    }
}

/// A full LLM call trace record
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceRecord {
    pub id: Uuid,
    pub session_id: Uuid,
    pub agent_name: String,
    pub started_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub model_provider: String,
    pub model_id: String,
    pub request_messages: String,
    pub request_tools: Option<String>,
    pub response_content: Option<String>,
    pub response_tool_calls: Option<String>,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
    pub latency_ms: Option<u64>,
    pub error: Option<String>,
}

/// Builder for creating traces - start before LLM call, complete after
#[derive(Debug, Clone)]
pub struct TraceBuilder {
    pub id: Uuid,
    pub session_id: Uuid,
    pub agent_name: String,
    pub model_provider: String,
    pub model_id: String,
    pub request_messages: String,
    pub request_tools: Option<String>,
    pub started_at: DateTime<Utc>,
}

/// Guard that saves trace on drop - ensures partial data is saved even on interruption
///
/// NOTE: For SIGTERM/SIGKILL protection, call `flush()` periodically during streaming.
/// Drop only runs on graceful shutdown - hard kills won't trigger it.
pub struct TraceGuard {
    telemetry: Arc<Telemetry>,
    id: Uuid,
    session_id: Uuid,
    agent_name: String,
    model_provider: String,
    model_id: String,
    request_messages: String,
    request_tools: Option<String>,
    started_at: DateTime<Utc>,
    // Accumulated response data - updated as streaming progresses
    pub response_content: String,
    pub response_thinking: String,
    pub response_tool_calls: Vec<serde_json::Value>,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub error: Option<String>,
    // Set to true when explicitly completed (prevents double-save)
    completed: bool,
    // Track if we've saved to DB yet (for upsert logic)
    saved_once: bool,
}

impl TraceGuard {
    pub fn new(telemetry: Arc<Telemetry>, builder: TraceBuilder) -> Self {
        Self {
            telemetry,
            id: builder.id,
            session_id: builder.session_id,
            agent_name: builder.agent_name,
            model_provider: builder.model_provider,
            model_id: builder.model_id,
            request_messages: builder.request_messages,
            request_tools: builder.request_tools,
            started_at: builder.started_at,
            response_content: String::new(),
            response_thinking: String::new(),
            response_tool_calls: Vec::new(),
            input_tokens: None,
            output_tokens: None,
            error: None,
            completed: false,
            saved_once: false,
        }
    }

    /// Append text to the response
    pub fn push_text(&mut self, text: &str) {
        self.response_content.push_str(text);
    }

    /// Append thinking/reasoning tokens
    pub fn push_thinking(&mut self, text: &str) {
        self.response_thinking.push_str(text);
    }

    /// Add a tool call
    pub fn push_tool_call(&mut self, id: &str, name: &str, arguments: &str) {
        self.response_tool_calls.push(serde_json::json!({
            "id": id,
            "name": name,
            "arguments": arguments
        }));
    }

    /// Update token counts
    pub fn set_usage(&mut self, input: u64, output: u64) {
        self.input_tokens = Some(input);
        self.output_tokens = Some(output);
    }

    /// Mark as errored
    pub fn set_error(&mut self, error: impl Into<String>) {
        self.error = Some(error.into());
    }

    /// Flush current state to database - call periodically during streaming
    /// for SIGTERM protection. Safe to call multiple times.
    pub fn flush(&mut self) {
        self.save_impl();
        self.saved_once = true;
    }

    /// Explicitly complete and save (prevents drop from saving again)
    pub fn complete(mut self) {
        self.save_impl();
        self.completed = true;
    }

    fn save_impl(&self) {
        let completed_at = Utc::now();
        let latency_ms = (completed_at - self.started_at).num_milliseconds() as u64;

        let tool_calls_json = if self.response_tool_calls.is_empty() {
            None
        } else {
            serde_json::to_string(&self.response_tool_calls).ok()
        };

        let trace = TraceRecord {
            id: self.id,
            session_id: self.session_id,
            agent_name: self.agent_name.clone(),
            started_at: self.started_at,
            completed_at: Some(completed_at),
            model_provider: self.model_provider.clone(),
            model_id: self.model_id.clone(),
            request_messages: self.request_messages.clone(),
            request_tools: self.request_tools.clone(),
            response_content: {
                // Combine thinking + content for full trace
                let mut full_content = String::new();
                if !self.response_thinking.is_empty() {
                    full_content.push_str("<thinking>\n");
                    full_content.push_str(&self.response_thinking);
                    full_content.push_str("\n</thinking>\n");
                }
                if !self.response_content.is_empty() {
                    full_content.push_str(&self.response_content);
                }
                if full_content.is_empty() {
                    None
                } else {
                    Some(full_content)
                }
            },
            response_tool_calls: tool_calls_json,
            input_tokens: self.input_tokens,
            output_tokens: self.output_tokens,
            total_tokens: match (self.input_tokens, self.output_tokens) {
                (Some(i), Some(o)) => Some(i + o),
                _ => None,
            },
            latency_ms: Some(latency_ms),
            error: self.error.clone(),
        };

        self.telemetry.save_trace(trace);
    }
}

impl Drop for TraceGuard {
    fn drop(&mut self) {
        if !self.completed {
            // Save whatever we accumulated before being dropped
            self.save_impl();
        }
    }
}

impl TraceBuilder {
    pub fn new(
        session_id: Uuid,
        agent_name: impl Into<String>,
        model_provider: impl Into<String>,
        model_id: impl Into<String>,
        request_messages: impl Into<String>,
    ) -> Self {
        Self {
            id: Uuid::new_v4(),
            session_id,
            agent_name: agent_name.into(),
            model_provider: model_provider.into(),
            model_id: model_id.into(),
            request_messages: request_messages.into(),
            request_tools: None,
            started_at: Utc::now(),
        }
    }

    pub fn with_tools(mut self, tools: impl Into<String>) -> Self {
        self.request_tools = Some(tools.into());
        self
    }

    /// Complete the trace with success
    pub fn complete(
        self,
        response_content: Option<String>,
        response_tool_calls: Option<String>,
        input_tokens: Option<u64>,
        output_tokens: Option<u64>,
    ) -> TraceRecord {
        let completed_at = Utc::now();
        let latency_ms = (completed_at - self.started_at).num_milliseconds() as u64;

        TraceRecord {
            id: self.id,
            session_id: self.session_id,
            agent_name: self.agent_name,
            started_at: self.started_at,
            completed_at: Some(completed_at),
            model_provider: self.model_provider,
            model_id: self.model_id,
            request_messages: self.request_messages,
            request_tools: self.request_tools,
            response_content,
            response_tool_calls,
            input_tokens,
            output_tokens,
            total_tokens: match (input_tokens, output_tokens) {
                (Some(i), Some(o)) => Some(i + o),
                _ => None,
            },
            latency_ms: Some(latency_ms),
            error: None,
        }
    }

    /// Complete the trace with an error (discards partial data)
    pub fn fail(self, error: impl Into<String>) -> TraceRecord {
        self.fail_with_partial(error, None, None, None, None)
    }

    /// Complete the trace with an error, preserving any partial data we accumulated
    pub fn fail_with_partial(
        self,
        error: impl Into<String>,
        response_content: Option<String>,
        response_tool_calls: Option<String>,
        input_tokens: Option<u64>,
        output_tokens: Option<u64>,
    ) -> TraceRecord {
        let completed_at = Utc::now();
        let latency_ms = (completed_at - self.started_at).num_milliseconds() as u64;

        TraceRecord {
            id: self.id,
            session_id: self.session_id,
            agent_name: self.agent_name,
            started_at: self.started_at,
            completed_at: Some(completed_at),
            model_provider: self.model_provider,
            model_id: self.model_id,
            request_messages: self.request_messages,
            request_tools: self.request_tools,
            response_content,
            response_tool_calls,
            input_tokens,
            output_tokens,
            total_tokens: match (input_tokens, output_tokens) {
                (Some(i), Some(o)) => Some(i + o),
                _ => None,
            },
            latency_ms: Some(latency_ms),
            error: Some(error.into()),
        }
    }
}

/// Main telemetry handler
pub struct Telemetry {
    session: Arc<TelemetrySession>,
    db: Arc<Mutex<TelemetryDb>>,
    log_dir: PathBuf,
    _file_guard: Option<WorkerGuard>,
    _otel_provider: Option<SdkTracerProvider>,
}

impl Telemetry {
    /// Initialize telemetry with full observability stack
    ///
    /// If `use_stderr` is true, console logs go to stderr instead of stdout.
    /// This is required for ACP serve mode where stdout is used for JSON-RPC.
    pub fn init(
        log_dir: PathBuf,
        verbose: bool,
        otel_endpoint: Option<&str>,
        working_dir: Option<&str>,
        model: Option<&str>,
        provider: Option<&str>,
    ) -> anyhow::Result<Self> {
        Self::init_with_options(log_dir, verbose, otel_endpoint, working_dir, model, provider, false)
    }

    /// Initialize telemetry with stderr output (for ACP serve mode)
    pub fn init_for_serve(
        log_dir: PathBuf,
        verbose: bool,
        otel_endpoint: Option<&str>,
        working_dir: Option<&str>,
        model: Option<&str>,
        provider: Option<&str>,
    ) -> anyhow::Result<Self> {
        Self::init_with_options(log_dir, verbose, otel_endpoint, working_dir, model, provider, true)
    }

    fn init_with_options(
        log_dir: PathBuf,
        verbose: bool,
        otel_endpoint: Option<&str>,
        working_dir: Option<&str>,
        model: Option<&str>,
        provider: Option<&str>,
        use_stderr: bool,
    ) -> anyhow::Result<Self> {
        std::fs::create_dir_all(&log_dir)?;

        let session = Arc::new(TelemetrySession::new());

        // Initialize SQLite
        let db_path = log_dir.join("telemetry.db");
        let db = TelemetryDb::new(&db_path)?;
        db.insert_session(&session, working_dir, model, provider)?;
        let db = Arc::new(Mutex::new(db));

        // Set up file appender for JSON logs
        let file_appender = tracing_appender::rolling::daily(&log_dir, "crow_agent.log");
        let (non_blocking, file_guard) = tracing_appender::non_blocking(file_appender);

        // Helper to create env filter
        let make_env_filter = || {
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| {
                    if verbose {
                        EnvFilter::new("debug,hyper=info,reqwest=info,h2=info,rustls=info")
                    } else {
                        EnvFilter::new("info,hyper=warn,reqwest=warn,h2=warn,rustls=warn")
                    }
                })
        };

        // SQLite trace layer - captures gen_ai spans to database
        let sqlite_layer = SqliteTraceLayer::new(db_path.clone(), session.id.to_string());

        // OpenTelemetry setup (optional)
        let otel_provider = if let Some(endpoint) = otel_endpoint {
            let exporter = opentelemetry_otlp::SpanExporter::builder()
                .with_http()
                .with_endpoint(endpoint)
                .with_protocol(opentelemetry_otlp::Protocol::HttpBinary)
                .build()?;

            let provider = SdkTracerProvider::builder()
                .with_batch_exporter(exporter)
                .with_resource(Resource::builder()
                    .with_service_name("crow-agent")
                    .build())
                .build();

            let tracer = provider.tracer("crow-agent");
            let otel_layer = tracing_opentelemetry::layer().with_tracer(tracer);

            // In serve mode, skip console output (stdout is for JSON-RPC)
            // Logs still go to file and SQLite
            let subscriber = tracing_subscriber::registry()
                .with(make_env_filter())
                .with(fmt::layer().with_target(false).compact())
                .with(fmt::layer().json().with_writer(non_blocking))
                .with(sqlite_layer)
                .with(otel_layer);
            subscriber.try_init().ok();
            Some(provider)
        } else if use_stderr {
            // Serve mode without OTel: skip console output (stdout is for JSON-RPC)
            // Logs still go to file and SQLite
            let subscriber = tracing_subscriber::registry()
                .with(make_env_filter())
                .with(fmt::layer().json().with_writer(non_blocking))
                .with(sqlite_layer);
            subscriber.try_init().ok();
            None
        } else if verbose {
            // Verbose mode: console + file + SQLite
            let subscriber = tracing_subscriber::registry()
                .with(make_env_filter())
                .with(fmt::layer().with_target(false).compact())
                .with(fmt::layer().json().with_writer(non_blocking))
                .with(sqlite_layer);
            subscriber.try_init().ok();
            None
        } else {
            // Normal mode: file + SQLite only (no console noise)
            let subscriber = tracing_subscriber::registry()
                .with(make_env_filter())
                .with(fmt::layer().json().with_writer(non_blocking))
                .with(sqlite_layer);
            subscriber.try_init().ok();
            None
        };

        tracing::info!(
            session_id = %session.id,
            log_dir = %log_dir.display(),
            db_path = %db_path.display(),
            otel_enabled = otel_endpoint.is_some(),
            "Telemetry initialized"
        );

        Ok(Self {
            session,
            db,
            log_dir,
            _file_guard: Some(file_guard),
            _otel_provider: otel_provider,
        })
    }

    /// Create a minimal telemetry instance (for testing)
    pub fn minimal() -> anyhow::Result<Self> {
        let log_dir = PathBuf::from(".crow_logs");
        std::fs::create_dir_all(&log_dir)?;

        let session = Arc::new(TelemetrySession::new());
        let db_path = log_dir.join("telemetry.db");
        let db = TelemetryDb::new(&db_path)?;
        db.insert_session(&session, None, None, None)?;

        Ok(Self {
            session,
            db: Arc::new(Mutex::new(db)),
            log_dir,
            _file_guard: None,
            _otel_provider: None,
        })
    }

    /// Get the current session ID
    pub fn session_id(&self) -> Uuid {
        self.session.id
    }

    /// Log a user message
    pub async fn log_user_message(&self, content: &str) {
        let record = InteractionRecord {
            id: Uuid::new_v4(),
            session_id: self.session.id,
            timestamp: Utc::now(),
            interaction_type: InteractionType::UserMessage,
            duration_ms: None,
            prompt_tokens: None,
            completion_tokens: None,
            total_tokens: None,
            model: None,
            content: Some(content.to_string()),
            raw_request: None,
            raw_response: None,
            error: None,
        };

        self.session.next_interaction();

        tracing::info!(
            interaction_id = %record.id,
            role = "user",
            content_len = content.len(),
            "User message"
        );
        tracing::debug!(content = content, "User message content");

        if let Ok(db) = self.db.lock() {
            let _ = db.insert_interaction(&record);
        }
    }

    /// Log an LLM response
    pub async fn log_response(
        &self,
        content: &str,
        token_count: Option<TokenCount>,
        duration_ms: u64,
        model: Option<&str>,
        raw_response: Option<&str>,
    ) {
        if let Some(ref tc) = token_count {
            self.session.add_tokens(tc.total_tokens as u64);
        }

        let record = InteractionRecord {
            id: Uuid::new_v4(),
            session_id: self.session.id,
            timestamp: Utc::now(),
            interaction_type: InteractionType::AssistantMessage,
            duration_ms: Some(duration_ms),
            prompt_tokens: token_count.as_ref().map(|t| t.prompt_tokens),
            completion_tokens: token_count.as_ref().map(|t| t.completion_tokens),
            total_tokens: token_count.as_ref().map(|t| t.total_tokens),
            model: model.map(String::from),
            content: Some(content.to_string()),
            raw_request: None,
            raw_response: raw_response.map(String::from),
            error: None,
        };

        self.session.next_interaction();

        tracing::info!(
            interaction_id = %record.id,
            duration_ms = duration_ms,
            prompt_tokens = ?token_count.as_ref().map(|t| t.prompt_tokens),
            completion_tokens = ?token_count.as_ref().map(|t| t.completion_tokens),
            total_tokens = ?token_count.as_ref().map(|t| t.total_tokens),
            content_len = content.len(),
            "LLM response"
        );
        tracing::debug!(content = content, "LLM response content");

        if let Ok(db) = self.db.lock() {
            let _ = db.insert_interaction(&record);
        }
    }

    /// Log a tool call
    pub async fn log_tool_call(
        &self,
        tool_name: &str,
        arguments: &serde_json::Value,
        result: Result<&str, &str>,
        duration_ms: u64,
    ) {
        let (result_str, error_str) = match result {
            Ok(r) => (Some(r.to_string()), None),
            Err(e) => (None, Some(e.to_string())),
        };

        let tool_record = ToolCallRecord {
            id: Uuid::new_v4().to_string(),
            name: tool_name.to_string(),
            arguments: arguments.clone(),
            result: result_str.clone(),
            error: error_str.clone(),
            duration_ms,
        };

        let interaction_id = Uuid::new_v4();
        let record = InteractionRecord {
            id: interaction_id,
            session_id: self.session.id,
            timestamp: Utc::now(),
            interaction_type: InteractionType::ToolCall,
            duration_ms: Some(duration_ms),
            prompt_tokens: None,
            completion_tokens: None,
            total_tokens: None,
            model: None,
            content: Some(format!("{}({})", tool_name, arguments)),
            raw_request: None,
            raw_response: None,
            error: error_str,
        };

        self.session.next_interaction();

        tracing::info!(
            tool = tool_name,
            duration_ms = duration_ms,
            success = result.is_ok(),
            "Tool call"
        );
        tracing::debug!(
            arguments = %arguments,
            result = ?result_str,
            "Tool call details"
        );

        if let Ok(db) = self.db.lock() {
            let _ = db.insert_interaction(&record);
            let _ = db.insert_tool_call(interaction_id, self.session.id, &tool_record);
        }
    }

    /// Log an error
    pub async fn log_error(&self, context: &str, error: &str) {
        tracing::error!(context = context, error = error, "Error occurred");

        let record = InteractionRecord {
            id: Uuid::new_v4(),
            session_id: self.session.id,
            timestamp: Utc::now(),
            interaction_type: InteractionType::AssistantMessage,
            duration_ms: None,
            prompt_tokens: None,
            completion_tokens: None,
            total_tokens: None,
            model: None,
            content: None,
            raw_request: None,
            raw_response: None,
            error: Some(format!("{}: {}", context, error)),
        };

        if let Ok(db) = self.db.lock() {
            let _ = db.insert_interaction(&record);
        }
    }

    /// Start a trace - call before making an LLM request
    /// Returns a TraceBuilder that should be completed after the request
    pub fn start_trace(
        &self,
        agent_name: impl Into<String>,
        model_provider: impl Into<String>,
        model_id: impl Into<String>,
        request_messages: impl Into<String>,
    ) -> TraceBuilder {
        TraceBuilder::new(
            self.session.id,
            agent_name,
            model_provider,
            model_id,
            request_messages,
        )
    }

    /// Save a completed trace to the database
    pub fn save_trace(&self, trace: TraceRecord) {
        tracing::debug!(
            trace_id = %trace.id,
            agent = %trace.agent_name,
            model = %trace.model_id,
            input_tokens = ?trace.input_tokens,
            output_tokens = ?trace.output_tokens,
            latency_ms = ?trace.latency_ms,
            error = ?trace.error,
            "LLM trace"
        );

        match self.db.lock() {
            Ok(db) => {
                if let Err(e) = db.insert_trace(&trace) {
                    tracing::error!(error = %e, trace_id = %trace.id, "Failed to insert trace");
                }
            }
            Err(e) => {
                tracing::error!(error = %e, "Failed to lock telemetry db");
            }
        }
    }

    /// Get session statistics
    pub fn stats(&self) -> SessionStats {
        SessionStats {
            session_id: self.session.id,
            started_at: self.session.started_at,
            interaction_count: self.session.interaction_count.load(Ordering::SeqCst),
            total_tokens: self.session.total_tokens.load(Ordering::SeqCst),
        }
    }

    /// Get the database path for direct querying
    pub fn db_path(&self) -> PathBuf {
        self.log_dir.join("telemetry.db")
    }

    /// Query recent sessions
    pub fn recent_sessions(&self, limit: usize) -> anyhow::Result<Vec<SessionSummary>> {
        let db = self.db.lock().map_err(|e| anyhow::anyhow!("Lock error: {}", e))?;

        let mut stmt = db.conn.prepare(r#"
            SELECT s.id, s.started_at, s.working_dir, s.model, s.provider,
                   COUNT(i.id) as interaction_count,
                   COALESCE(SUM(i.total_tokens), 0) as total_tokens
            FROM sessions s
            LEFT JOIN interactions i ON s.id = i.session_id
            GROUP BY s.id
            ORDER BY s.started_at DESC
            LIMIT ?1
        "#)?;

        let rows = stmt.query_map([limit], |row| {
            Ok(SessionSummary {
                id: row.get::<_, String>(0)?,
                started_at: row.get::<_, String>(1)?,
                working_dir: row.get::<_, Option<String>>(2)?,
                model: row.get::<_, Option<String>>(3)?,
                provider: row.get::<_, Option<String>>(4)?,
                interaction_count: row.get::<_, i64>(5)? as u64,
                total_tokens: row.get::<_, i64>(6)? as u64,
            })
        })?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row?);
        }
        Ok(results)
    }

    /// Query tool usage statistics
    pub fn tool_stats(&self) -> anyhow::Result<Vec<ToolUsageStat>> {
        let db = self.db.lock().map_err(|e| anyhow::anyhow!("Lock error: {}", e))?;

        let mut stmt = db.conn.prepare(r#"
            SELECT tool_name,
                   COUNT(*) as call_count,
                   AVG(duration_ms) as avg_duration_ms,
                   SUM(CASE WHEN error IS NULL THEN 1 ELSE 0 END) as success_count,
                   SUM(CASE WHEN error IS NOT NULL THEN 1 ELSE 0 END) as error_count
            FROM tool_calls
            GROUP BY tool_name
            ORDER BY call_count DESC
        "#)?;

        let rows = stmt.query_map([], |row| {
            Ok(ToolUsageStat {
                tool_name: row.get(0)?,
                call_count: row.get::<_, i64>(1)? as u64,
                avg_duration_ms: row.get(2)?,
                success_count: row.get::<_, i64>(3)? as u64,
                error_count: row.get::<_, i64>(4)? as u64,
            })
        })?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row?);
        }
        Ok(results)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct SessionStats {
    pub session_id: Uuid,
    pub started_at: DateTime<Utc>,
    pub interaction_count: u64,
    pub total_tokens: u64,
}

impl std::fmt::Display for SessionStats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Session {} | Started: {} | Interactions: {} | Tokens: {}",
            self.session_id,
            self.started_at.format("%Y-%m-%d %H:%M:%S UTC"),
            self.interaction_count,
            self.total_tokens
        )
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct SessionSummary {
    pub id: String,
    pub started_at: String,
    pub working_dir: Option<String>,
    pub model: Option<String>,
    pub provider: Option<String>,
    pub interaction_count: u64,
    pub total_tokens: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolUsageStat {
    pub tool_name: String,
    pub call_count: u64,
    pub avg_duration_ms: f64,
    pub success_count: u64,
    pub error_count: u64,
}
