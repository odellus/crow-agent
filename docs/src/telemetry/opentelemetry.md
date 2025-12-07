# OpenTelemetry Integration

Crow Agent uses OpenTelemetry for distributed tracing, with traces exported to OTLP-compatible backends.

## Rig's Built-in Tracing

Rig includes OpenTelemetry spans for LLM operations. The spans are created in the provider implementations.

### Non-Streaming Span (`completion.rs`)

```rust
let span = info_span!(
    target: "rig::completions",
    "chat",
    gen_ai.operation.name = "chat",
    gen_ai.provider.name = "openrouter",
    gen_ai.request.model = self.model,
    gen_ai.system_instructions = preamble,
    gen_ai.request.tools = tracing::field::Empty,
    gen_ai.request.body = tracing::field::Empty,
    gen_ai.usage.input_tokens = tracing::field::Empty,
    gen_ai.usage.output_tokens = tracing::field::Empty,
);
```

### Streaming Span (`streaming.rs`)

We modified the fork to include telemetry in streaming:

```rust
let span = info_span!(
    target: "rig::completions",
    "chat_streaming",
    gen_ai.operation.name = "chat_streaming",
    gen_ai.provider.name = "openrouter",
    gen_ai.request.model = self.model,
    gen_ai.system_instructions = preamble,
    gen_ai.request.tools = tracing::field::Empty,   // Added in fork
    gen_ai.request.body = tracing::field::Empty,    // Added in fork
    gen_ai.response.id = tracing::field::Empty,
    gen_ai.usage.input_tokens = tracing::field::Empty,
    gen_ai.usage.output_tokens = tracing::field::Empty,
);

// Record tools and body
if let Some(ref tools) = tools_json {
    span.record("gen_ai.request.tools", tools.as_str());
}
if let Some(ref body) = body_json {
    span.record("gen_ai.request.body", body.as_str());
}
```

## Setup

### Environment Variables

```bash
# OTLP endpoint (gRPC)
export OTEL_EXPORTER_OTLP_ENDPOINT="http://localhost:4317"

# Service name for traces
export OTEL_SERVICE_NAME="crow-agent"

# Optional: trace level
export RUST_LOG="crow_agent=debug,rig=info"
```

### Running Jaeger

```bash
# Start Jaeger with OTLP support
docker run -d --name jaeger \
  -p 16686:16686 \
  -p 4317:4317 \
  jaegertracing/all-in-one:latest
```

### Viewing Traces

1. Open Jaeger UI: http://localhost:16686
2. Select service: "crow-agent"
3. Find traces to see:
   - Agent invocation span
   - Chat/streaming spans
   - Tool execution spans

## Trace Structure

```
invoke_agent (gen_ai.agent.name="crow")
├── chat_streaming (gen_ai.operation.name="chat_streaming")
│   ├── gen_ai.request.model="anthropic/claude-sonnet..."
│   ├── gen_ai.request.tools="[{\"name\":\"read_file\"...}]"
│   └── gen_ai.request.body="{\"model\":...}"
├── execute_tool (gen_ai.tool.name="read_file")
│   ├── gen_ai.tool.call.arguments="{\"path\":\"...\"}"
│   └── gen_ai.tool.call.result="file contents..."
├── chat_streaming (follow-up after tool)
│   └── ...
└── execute_tool (gen_ai.tool.name="edit_file")
    └── ...
```

## Querying Traces

### By Request Tools

Find all requests that included a specific tool:

```
gen_ai.request.tools contains "read_file"
```

### By Model

```
gen_ai.request.model = "anthropic/claude-sonnet-4-20250514"
```

### By Duration

Find slow requests:

```
duration > 10s
```

## Verifying Telemetry

To verify streaming telemetry is working:

```sql
-- Query SQLite telemetry database
SELECT 
    json_extract(attributes, '$.gen_ai.request.tools') as tools,
    json_extract(attributes, '$.gen_ai.request.model') as model
FROM spans
WHERE name = 'chat_streaming'
ORDER BY timestamp DESC
LIMIT 5;
```

Expected: `tools` should contain JSON array of tool definitions, not NULL.
