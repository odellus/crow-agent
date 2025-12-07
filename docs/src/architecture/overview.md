# Architecture Overview

Crow Agent is built on a layered architecture that separates concerns between the AI framework, protocol handling, and telemetry.

## Component Diagram

```
┌─────────────────────────────────────────────────────────────┐
│                      User Interface                          │
│                  (CLI / Zed via ACP)                        │
└─────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────┐
│                    ACP Server (acp.rs)                       │
│  - JSON-RPC 2.0 over stdio                                  │
│  - Session management                                        │
│  - Cancellation handling                                     │
│  - Stream → notification conversion                          │
└─────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────┐
│                   CrowAgent (agent.rs)                       │
│  - Wraps rig Agent                                          │
│  - Manages streaming/non-streaming paths                     │
│  - Attaches telemetry hooks                                  │
└─────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────┐
│                  Rig Framework (forked)                      │
│  - Agent execution                                          │
│  - Tool calling                                             │
│  - Streaming support                                         │
│  - OpenRouter provider                                       │
└─────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────┐
│                    LLM Provider                              │
│                    (OpenRouter)                              │
└─────────────────────────────────────────────────────────────┘
```

## Key Components

### CrowAgent (`agent.rs`)

The core agent wrapper that:
- Creates and configures the rig Agent with tools and system prompt
- Provides `chat()` for non-streaming and `chat_stream()` for streaming
- Attaches `TelemetryHook` to capture events

```rust
pub struct CrowAgent {
    config: Config,
    telemetry: Arc<Telemetry>,
}

impl CrowAgent {
    pub async fn chat_stream(
        &self,
        message: &str,
        history: Vec<Message>,
    ) -> Result<(StreamingPromptRequest<...>, TelemetryHook)>
}
```

### ACP Server (`acp.rs`)

Implements the Agent Client Protocol for editor integration:
- `CrowAcpAgent` implements `acp::Agent` trait
- Manages sessions with history and cancellation state
- Converts rig stream items to ACP notifications

### Telemetry Hooks (`hooks.rs`)

`TelemetryHook` implements both `PromptHook` and `StreamingPromptHook`:
- Captures tool call timing and results
- Stores cancel signal for interruption
- Logs events to OpenTelemetry

## Data Flow

### Streaming Request Flow

```
1. User sends prompt via ACP
2. CrowAcpAgent.prompt() called
3. CrowAgent.chat_stream() creates StreamingPromptRequest
4. Stream yields MultiTurnStreamItem variants:
   - StreamAssistantItem (text, tool calls, reasoning)
   - StreamUserItem (tool results)
   - FinalResponse
5. Each item converted to SessionNotification
6. Notifications sent to client
7. History updated on FinalResponse
```

### Cancellation Flow

```
1. Client sends session/cancel notification
2. CrowAcpAgent.cancel() called
3. Session.cancelled flag set to true
4. Hook.cancel() triggers CancelSignal
5. Stream loop checks cancelled flag
6. Returns StopReason::Cancelled
```
