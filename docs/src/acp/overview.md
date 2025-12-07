# ACP Overview

ACP (Agent Client Protocol) is a JSON-RPC 2.0 based protocol for communication between AI agents and clients (like editors). Crow Agent implements the ACP server side to integrate with Zed and other ACP-compatible clients.

## Protocol Basics

- **Transport**: stdio (stdin/stdout)
- **Format**: JSON-RPC 2.0
- **Direction**: Bidirectional - client sends requests, agent sends responses and notifications

## Message Types

### Requests (Client → Agent)

| Method | Description |
|--------|-------------|
| `initialize` | Initialize the connection |
| `session/new` | Create a new session |
| `session/prompt` | Send a prompt to the agent |
| `session/cancel` | Cancel an ongoing operation |
| `session/setMode` | Change session mode |

### Notifications (Agent → Client)

Session updates sent during prompt execution:

| Update Type | Description |
|-------------|-------------|
| `agent_message_chunk` | Text content from the agent |
| `agent_thought_chunk` | Reasoning/thinking content |
| `tool_call` | Tool execution started |
| `tool_call_update` | Tool execution completed |
| `plan` | Todo list updates |

## Implementation

Crow Agent's ACP implementation is in `src/acp.rs`:

```rust
pub struct CrowAcpAgent {
    session_update_tx: mpsc::UnboundedSender<(SessionNotification, oneshot::Sender<()>)>,
    next_session_id: Cell<u64>,
    sessions: RefCell<HashMap<String, Session>>,
    config: Config,
    telemetry: Arc<Telemetry>,
}

#[async_trait(?Send)]
impl acp::Agent for CrowAcpAgent {
    async fn initialize(&self, args: InitializeRequest) -> acp::Result<InitializeResponse>;
    async fn new_session(&self, args: NewSessionRequest) -> acp::Result<NewSessionResponse>;
    async fn prompt(&self, args: PromptRequest) -> acp::Result<PromptResponse>;
    async fn cancel(&self, args: CancelNotification) -> acp::Result<()>;
    // ... other methods
}
```

## Running the ACP Server

```bash
# Start ACP server on stdio
cargo run -- acp

# Or with release build
./target/release/crow-agent acp
```

## Testing with Manual Requests

You can test the ACP server by sending JSON-RPC messages to stdin:

```bash
# In one terminal, start the server
./target/release/crow-agent acp

# Send initialize
{"jsonrpc":"2.0","method":"initialize","params":{"protocolVersion":"0.1"},"id":1}

# Send new session
{"jsonrpc":"2.0","method":"session/new","params":{"cwd":"/tmp","mcpServers":[]},"id":2}

# Send prompt
{"jsonrpc":"2.0","method":"session/prompt","params":{"sessionId":"0","prompt":[{"type":"text","text":"Hello!"}]},"id":3}
```

## Reference

- [ACP Specification](https://github.com/anthropics/agent-client-protocol)
- [claude-code-acp](https://github.com/anthropics/claude-code/tree/main/claude-code-acp) - Reference implementation
