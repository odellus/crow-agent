//! Custom tracing Layer that captures rig's GenAI spans to SQLite
//!
//! This layer intercepts tracing spans with gen_ai.* fields and stores them
//! in the traces table for later querying via CLI.

use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::{Event, Subscriber};
use tracing_subscriber::layer::Context;
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::Layer;
use uuid::Uuid;

/// Storage for a single trace (LLM call)
#[derive(Debug, Clone, Default)]
pub struct TraceData {
    pub id: String,
    pub session_id: String,
    pub started_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub operation_name: Option<String>,
    pub model_provider: Option<String>,
    pub model_id: Option<String>,
    pub system_instructions: Option<String>,
    pub prompt: Option<String>,
    pub completion: Option<String>,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub tool_name: Option<String>,
    pub tool_call_id: Option<String>,
    pub tool_arguments: Option<String>,
    pub tool_result: Option<String>,
    pub error: Option<String>,
    // Full request/response data
    pub request_tools: Option<String>,
    pub request_body: Option<String>,
    pub response_body: Option<String>,
    pub response_content: Option<String>,
    pub response_tool_calls: Option<String>,
}

/// Visitor to extract gen_ai.* fields from spans
struct GenAiVisitor {
    fields: HashMap<String, String>,
}

impl GenAiVisitor {
    fn new() -> Self {
        Self {
            fields: HashMap::new(),
        }
    }
}

impl Visit for GenAiVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        let name = field.name();
        if name.starts_with("gen_ai.") || name == "agent_name" {
            self.fields.insert(name.to_string(), format!("{:?}", value));
        }
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        let name = field.name();
        if name.starts_with("gen_ai.") || name == "agent_name" {
            self.fields.insert(name.to_string(), value.to_string());
        }
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        let name = field.name();
        if name.starts_with("gen_ai.") {
            self.fields.insert(name.to_string(), value.to_string());
        }
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        let name = field.name();
        if name.starts_with("gen_ai.") {
            self.fields.insert(name.to_string(), value.to_string());
        }
    }
}

/// Layer that captures gen_ai spans to SQLite
pub struct SqliteTraceLayer {
    db_path: PathBuf,
    session_id: String,
    /// In-flight spans being recorded
    spans: Arc<Mutex<HashMap<u64, TraceData>>>,
}

impl SqliteTraceLayer {
    pub fn new(db_path: PathBuf, session_id: String) -> Self {
        Self {
            db_path,
            session_id,
            spans: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    fn save_trace(&self, trace: &TraceData) {
        if let Ok(conn) = Connection::open(&self.db_path) {
            let latency_ms = trace
                .completed_at
                .map(|end| (end - trace.started_at).num_milliseconds());

            let _ = conn.execute(
                r#"INSERT OR REPLACE INTO traces
                   (id, session_id, started_at, completed_at, model_provider, model_id,
                    request_messages, request_tools, response_content, response_tool_calls,
                    input_tokens, output_tokens, total_tokens, latency_ms, error)
                   VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)"#,
                params![
                    trace.id,
                    trace.session_id,
                    trace.started_at.to_rfc3339(),
                    trace.completed_at.map(|t| t.to_rfc3339()),
                    trace.model_provider.as_deref().unwrap_or("unknown"),
                    trace.model_id.as_deref().unwrap_or("unknown"),
                    // request_messages: use full request body if available, else system+prompt
                    trace.request_body.clone().or_else(|| {
                        trace.system_instructions.as_ref().map(|s| {
                            serde_json::json!({
                                "system": s,
                                "prompt": trace.prompt
                            })
                            .to_string()
                        })
                    }),
                    // request_tools: use full tools from request if available
                    trace.request_tools.clone().or_else(|| {
                        trace.tool_name.as_ref().map(|_| {
                            serde_json::json!({
                                "tool": trace.tool_name,
                                "arguments": trace.tool_arguments
                            })
                            .to_string()
                        })
                    }),
                    // response_content: use streaming accumulated content, else response body, else completion
                    trace
                        .response_content
                        .clone()
                        .or_else(|| trace.response_body.clone())
                        .or_else(|| trace.completion.clone()),
                    // response_tool_calls: use streaming accumulated tool calls, else tool result
                    trace.response_tool_calls.clone().or_else(|| {
                        trace.tool_result.as_ref().map(|r| {
                            serde_json::json!({
                                "tool": trace.tool_name,
                                "result": r
                            })
                            .to_string()
                        })
                    }),
                    trace.input_tokens,
                    trace.output_tokens,
                    trace
                        .input_tokens
                        .zip(trace.output_tokens)
                        .map(|(i, o)| i + o),
                    latency_ms,
                    trace.error
                ],
            );
        }
    }
}

impl<S> Layer<S> for SqliteTraceLayer
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, _ctx: Context<'_, S>) {
        let mut visitor = GenAiVisitor::new();
        attrs.record(&mut visitor);

        // Only track spans with gen_ai fields
        if visitor.fields.iter().any(|(k, _)| k.starts_with("gen_ai.")) {
            let mut trace = TraceData {
                id: Uuid::new_v4().to_string(),
                session_id: self.session_id.clone(),
                started_at: Utc::now(),
                ..Default::default()
            };

            // Extract known fields
            if let Some(op) = visitor.fields.get("gen_ai.operation.name") {
                trace.operation_name = Some(op.clone());
            }
            if let Some(sys) = visitor.fields.get("gen_ai.system_instructions") {
                trace.system_instructions = Some(sys.clone());
            }
            if let Some(provider) = visitor.fields.get("gen_ai.provider.name") {
                trace.model_provider = Some(provider.clone());
            }
            if let Some(model) = visitor.fields.get("gen_ai.request.model") {
                trace.model_id = Some(model.clone());
            }
            if let Some(tool) = visitor.fields.get("gen_ai.tool.name") {
                trace.tool_name = Some(tool.clone());
            }
            if let Some(call_id) = visitor.fields.get("gen_ai.tool.call.id") {
                trace.tool_call_id = Some(call_id.clone());
            }
            // Capture request tools and body if available at span creation
            if let Some(tools) = visitor.fields.get("gen_ai.request.tools") {
                trace.request_tools = Some(tools.clone());
            }
            if let Some(body) = visitor.fields.get("gen_ai.request.body") {
                trace.request_body = Some(body.clone());
            }

            if let Ok(mut spans) = self.spans.lock() {
                spans.insert(id.into_u64(), trace);
            }
        }
    }

    fn on_record(&self, id: &Id, values: &Record<'_>, _ctx: Context<'_, S>) {
        let mut visitor = GenAiVisitor::new();
        values.record(&mut visitor);

        if let Ok(mut spans) = self.spans.lock() {
            if let Some(trace) = spans.get_mut(&id.into_u64()) {
                // Update trace with new values
                if let Some(prompt) = visitor.fields.get("gen_ai.prompt") {
                    trace.prompt = Some(prompt.clone());
                }
                if let Some(completion) = visitor.fields.get("gen_ai.completion") {
                    trace.completion = Some(completion.clone());
                }
                if let Some(input) = visitor.fields.get("gen_ai.usage.input_tokens") {
                    trace.input_tokens = input.parse().ok();
                }
                if let Some(output) = visitor.fields.get("gen_ai.usage.output_tokens") {
                    trace.output_tokens = output.parse().ok();
                }
                if let Some(tool) = visitor.fields.get("gen_ai.tool.name") {
                    trace.tool_name = Some(tool.clone());
                }
                if let Some(call_id) = visitor.fields.get("gen_ai.tool.call.id") {
                    trace.tool_call_id = Some(call_id.clone());
                }
                if let Some(args) = visitor.fields.get("gen_ai.tool.call.arguments") {
                    trace.tool_arguments = Some(args.clone());
                }
                if let Some(result) = visitor.fields.get("gen_ai.tool.call.result") {
                    trace.tool_result = Some(result.clone());
                }
                if let Some(provider) = visitor.fields.get("gen_ai.provider.name") {
                    trace.model_provider = Some(provider.clone());
                }
                if let Some(model) = visitor.fields.get("gen_ai.request.model") {
                    trace.model_id = Some(model.clone());
                }
                // Capture full request/response
                if let Some(tools) = visitor.fields.get("gen_ai.request.tools") {
                    trace.request_tools = Some(tools.clone());
                }
                if let Some(body) = visitor.fields.get("gen_ai.request.body") {
                    trace.request_body = Some(body.clone());
                }
                if let Some(body) = visitor.fields.get("gen_ai.response.body") {
                    trace.response_body = Some(body.clone());
                }
                if let Some(content) = visitor.fields.get("gen_ai.response.content") {
                    trace.response_content = Some(content.clone());
                }
                if let Some(tool_calls) = visitor.fields.get("gen_ai.response.tool_calls") {
                    trace.response_tool_calls = Some(tool_calls.clone());
                }
            }
        }
    }

    fn on_event(&self, _event: &Event<'_>, _ctx: Context<'_, S>) {
        // We primarily care about spans, not events
    }

    fn on_close(&self, id: Id, _ctx: Context<'_, S>) {
        if let Ok(mut spans) = self.spans.lock() {
            if let Some(mut trace) = spans.remove(&id.into_u64()) {
                trace.completed_at = Some(Utc::now());
                self.save_trace(&trace);
            }
        }
    }
}
