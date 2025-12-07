# Telemetry Overview

Crow Agent includes comprehensive telemetry using OpenTelemetry for observability into agent operations.

## What's Captured

### LLM Requests

- Model name
- System prompt
- Request body (including tools)
- Response tokens
- Timing

### Tool Calls

- Tool name
- Arguments
- Result (success/error)
- Duration

### Spans

Hierarchical tracing spans for:
- Agent invocation
- Chat completions
- Tool execution

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                    TelemetryHook                            │
│  - Implements PromptHook + StreamingPromptHook              │
│  - Captures events from rig                                  │
│  - Logs to Telemetry                                         │
└─────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────┐
│                      Telemetry                               │
│  - SQLite storage                                            │
│  - OpenTelemetry export                                      │
│  - Async logging                                             │
└─────────────────────────────────────────────────────────────┘
                              │
              ┌───────────────┴───────────────┐
              ▼                               ▼
┌─────────────────────────┐     ┌─────────────────────────────┐
│     SQLite Database      │     │    OTLP Exporter            │
│  (local persistence)     │     │  (Jaeger, etc.)             │
└─────────────────────────┘     └─────────────────────────────┘
```

## Configuration

Telemetry is configured via environment:

```bash
# OTLP endpoint for traces
export OTEL_EXPORTER_OTLP_ENDPOINT="http://localhost:4317"

# Service name
export OTEL_SERVICE_NAME="crow-agent"
```

## Querying Telemetry

### SQLite

Tool calls are stored in SQLite for local analysis:

```sql
SELECT 
    tool_name,
    args,
    result,
    duration_ms,
    created_at
FROM tool_calls
ORDER BY created_at DESC
LIMIT 10;
```

### Jaeger

If exporting to Jaeger, view traces at `http://localhost:16686`.

## Span Attributes

Crow Agent follows OpenTelemetry semantic conventions for GenAI:

| Attribute | Description |
|-----------|-------------|
| `gen_ai.operation.name` | Operation type (chat, chat_streaming, etc.) |
| `gen_ai.provider.name` | LLM provider (openrouter) |
| `gen_ai.request.model` | Model name |
| `gen_ai.system_instructions` | System prompt |
| `gen_ai.request.tools` | Tool definitions (JSON) |
| `gen_ai.request.body` | Full request body (JSON) |
| `gen_ai.usage.input_tokens` | Input token count |
| `gen_ai.usage.output_tokens` | Output token count |
| `gen_ai.tool.name` | Tool being executed |
| `gen_ai.tool.call.arguments` | Tool arguments |
| `gen_ai.tool.call.result` | Tool result |
