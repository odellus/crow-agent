# Crow Agent

```


                       ██████╗ ██████╗   ██████╗  ██╗    ██╗
                      ██╔════╝ ██╔══██╗ ██╔═══██╗ ██║    ██║
                      ██║      ██████╔╝ ██║   ██║ ██║ █╗ ██║
                      ██║      ██╔══██╗ ██║   ██║ ██║███╗██║
                      ╚██████╗ ██║  ██║ ╚██████╔╝ ╚███╔███╔╝
                       ╚═════╝ ╚═╝  ╚═╝  ╚═════╝   ╚══╝╚══╝

                  █████╗   ██████╗  ███████╗ ███╗   ██╗ ████████╗
                 ██╔══██╗ ██╔════╝  ██╔════╝ ████╗  ██║ ╚══██╔══╝
                 ███████║ ██║  ███╗ █████╗   ██╔██╗ ██║    ██║
                 ██╔══██║ ██║   ██║ ██╔══╝   ██║╚██╗██║    ██║
                 ██║  ██║ ╚██████╔╝ ███████╗ ██║ ╚████║    ██║
                 ╚═╝  ╚═╝  ╚═════╝  ╚══════╝ ╚═╝  ╚═══╝    ╚═╝

```

A standalone LLM-powered coding agent built in Rust, designed for editor integration via the Agent Client Protocol (ACP).

Crow Agent provides a full agentic coding experience with streaming, tool use, multi-turn conversations, cancellation support, and comprehensive telemetry - all in a single binary.

## Features

- **ACP Server** - Integrates with Zed and other ACP-compatible editors via JSON-RPC over stdio
- **Streaming** - Real-time streaming of LLM responses and tool calls
- **Multi-turn** - Persistent conversation history within sessions
- **Cancellation** - Graceful cancellation of in-flight requests
- **Telemetry** - SQLite-backed tracing of all LLM calls, tool executions, and token usage
- **14 Built-in Tools** - File operations, search, terminal, web fetch, diagnostics, and more
- **Multiple Providers** - OpenRouter, custom OpenAI-compatible endpoints (LM Studio, vLLM, etc.)
- **REPL Mode** - Interactive command-line chat interface

## Installation

### From Source

```bash
# Clone the repository
git clone https://github.com/anthropics/crow-project
cd crow-project/crow_agent/crow_agent

# Build release binary
cargo build --release

# Binary is at ./target/release/crow-agent
```

### Dependencies

- Rust 1.75+ (uses nightly features like `let_chains`)
- For web search: SearXNG instance (optional, set `SEARXNG_URL`)

## Quick Start

### 1. Configure API Key

Create `~/.crow_agent/auth.json`:

```json
{
  "openrouter": {
    "api_key": "sk-or-v1-your-key-here"
  }
}
```

Or set environment variable:

```bash
export OPENROUTER_API_KEY="sk-or-v1-your-key-here"
```

### 2. Run Interactive REPL

```bash
crow-agent repl
```

### 3. Run Single Prompt

```bash
crow-agent prompt "Explain what this project does"
```

### 4. Run as ACP Server (for Zed)

```bash
crow-agent acp
```

## CLI Reference

```
crow-agent [OPTIONS] [COMMAND]

Commands:
  repl       Start an interactive REPL session (default)
  prompt     Run a single prompt
  acp        Run as ACP server over stdio
  telemetry  View traces and query telemetry database
  stats      Show session and tool statistics
  query      Run SQL query on telemetry DB

Options:
  -d, --working-dir <PATH>   Working directory [default: .]
  -m, --model <MODEL>        LLM model [default: glm-4.5-air@q4_k_m]
      --base-url <URL>       Custom LLM endpoint (LM Studio, etc.)
      --api-key <KEY>        API key (overrides auth.json)
      --data-dir <PATH>      Data directory [default: ~/.crow_agent]
      --otel-endpoint <URL>  OpenTelemetry collector endpoint
  -v, --verbose              Verbose logging
```

### Telemetry Commands

```bash
# List recent traces
crow-agent telemetry traces

# Show specific trace details
crow-agent telemetry trace <trace-id>

# Run SQL query
crow-agent telemetry sql "SELECT * FROM traces LIMIT 10"

# Show database schema
crow-agent telemetry schema
```

### REPL Commands

```
/quit, /exit  - Exit the REPL
/clear        - Clear chat history
/stats        - Show session statistics
/tools        - Show tool usage stats
/sessions     - Show recent sessions
/query <sql>  - Run SQL query on telemetry DB
/help         - Show help
```

## Configuration

### auth.json

Store API credentials in `~/.crow_agent/auth.json`:

```json
{
  "openrouter": {
    "api_key": "sk-or-v1-..."
  },
  "lm-studio": {
    "api_key": "lm-studio",
    "base_url": "http://localhost:1234/v1"
  },
  "anthropic": {
    "api_key": "sk-ant-..."
  }
}
```

The provider is auto-detected from the model name prefix (e.g., `anthropic/claude-3.5-sonnet` uses the `anthropic` entry).

### Environment Variables

| Variable | Description |
|----------|-------------|
| `OPENROUTER_API_KEY` | OpenRouter API key |
| `SEARXNG_URL` | SearXNG instance for web search (default: `http://localhost:8082`) |
| `XDG_DATA_HOME` | Base for data directory |

## Built-in Tools

| Tool | Description |
|------|-------------|
| `read_file` | Read file contents with line range support |
| `edit_file` | Edit files with fuzzy string replacement |
| `list_directory` | List directory contents |
| `grep` | Regex search across files |
| `find_path` | Find files by glob pattern |
| `terminal` | Execute shell commands |
| `thinking` | Scratchpad for reasoning |
| `now` | Get current date/time |
| `todo_write` | Manage task list |
| `todo_read` | Read current task list |
| `fetch` | Fetch URL content |
| `web_search` | Search the web via SearXNG |
| `diagnostics` | Get LSP diagnostics |
| `task_complete` | Signal task completion |

## Architecture

```
crow_agent/
├── src/
│   ├── main.rs          # CLI entry point
│   ├── agent.rs         # Core agent (builds rig agent with tools)
│   ├── acp.rs           # ACP server implementation
│   ├── config.rs        # Configuration structs
│   ├── hooks.rs         # TelemetryHook for rig events
│   ├── telemetry.rs     # SQLite telemetry storage
│   ├── trace_layer.rs   # Tracing layer for span capture
│   ├── templates/       # Handlebars system prompt templates
│   └── tools/           # Tool implementations
│       ├── read_file.rs
│       ├── edit_file.rs
│       ├── grep.rs
│       ├── terminal.rs
│       └── ...
└── rig/                 # Forked rig-core with streaming enhancements
```

### Key Components

**`agent.rs`** - The heart of the system. Creates a rig `Agent` with:
- System prompt from Handlebars templates
- All tools registered via `.tool()`
- Multi-turn execution via `.multi_turn(20)`

**`acp.rs`** - Implements the ACP `Agent` trait for editor integration:
- Session management (create, prompt, cancel)
- Streaming via `session/update` notifications
- Tool call → ACP `ToolCall` / `ToolCallUpdate` mapping
- Todo list → ACP `Plan` conversion

**`hooks.rs`** - `TelemetryHook` implements rig's `PromptHook` and `StreamingPromptHook`:
- Captures completion calls, tool calls, tool results
- Stores `CancelSignal` for graceful cancellation
- Logs to telemetry system

**`trace_layer.rs`** - Custom tracing `Layer` that captures rig's `gen_ai.*` spans:
- Extracts request/response data from tracing spans
- Persists to SQLite for later analysis

### Rig Fork

The `rig/` directory contains a forked `rig-core` with enhancements:
- Response accumulation during streaming for telemetry
- Tool call accumulation for telemetry
- Enhanced span fields for provider/model tracking

## Zed Integration

Add to Zed's `settings.json`:

```json
{
  "agent": {
    "enabled": true,
    "default_profile": {
      "provider": "agent_client_protocol",
      "model": "crow-agent",
      "agent_client_protocol": {
        "command": "/path/to/crow-agent",
        "args": ["acp", "-d", "/path/to/project", "-m", "anthropic/claude-3.5-sonnet"]
      }
    }
  }
}
```

## Telemetry Database

All LLM calls and tool executions are logged to `~/.crow_agent/telemetry.db`:

```sql
-- View recent traces
SELECT id, started_at, model_provider, latency_ms, 
       substr(response_content, 1, 50) as preview
FROM traces 
ORDER BY started_at DESC 
LIMIT 10;

-- Tool usage statistics
SELECT tool_name, COUNT(*) as calls, 
       AVG(duration_ms) as avg_ms,
       SUM(CASE WHEN success THEN 1 ELSE 0 END) as successes
FROM tool_calls 
GROUP BY tool_name;

-- Token usage by model
SELECT model_id, 
       SUM(input_tokens) as total_input,
       SUM(output_tokens) as total_output
FROM traces 
GROUP BY model_id;
```

## Development

### Building

```bash
# Debug build
cargo build

# Release build
cargo build --release

# Run tests
cargo test
```

### Adding a New Tool

1. Create `src/tools/my_tool.rs`:

```rust
use rig::tool::Tool;
use rig::completion::ToolDefinition;
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Debug, Deserialize)]
pub struct MyToolArgs {
    pub input: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MyTool;

impl Tool for MyTool {
    const NAME: &'static str = "my_tool";
    type Error = String;
    type Args = MyToolArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: "my_tool".to_string(),
            description: "Does something useful".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "input": { "type": "string", "description": "Input value" }
                },
                "required": ["input"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        Ok(format!("Processed: {}", args.input))
    }
}
```

2. Add to `src/tools/mod.rs`:
```rust
mod my_tool;
pub use my_tool::MyTool;
```

3. Register in `src/agent.rs`:
```rust
.tool(MyTool)
```

### Testing ACP

```python
#!/usr/bin/env python3
import subprocess
import json

proc = subprocess.Popen(
    ["./target/release/crow-agent", "acp"],
    stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True
)

def send(msg):
    proc.stdin.write(json.dumps(msg) + "\n")
    proc.stdin.flush()

def recv():
    return json.loads(proc.stdout.readline())

# Initialize
send({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
    "clientInfo": {"name": "test", "version": "1.0"},
    "capabilities": {},
    "protocolVersion": "0.1"
}})
print(recv())

# Create session
send({"jsonrpc": "2.0", "id": 2, "method": "session/new", "params": {
    "cwd": "/tmp",
    "mcpServers": []
}})
print(recv())

# Send prompt
send({"jsonrpc": "2.0", "id": 3, "method": "session/prompt", "params": {
    "sessionId": "0",
    "prompt": [{"type": "text", "text": "Hello!"}]
}})

# Read streaming responses
while True:
    resp = recv()
    print(resp)
    if resp.get("id") == 3 and "result" in resp:
        break
```

## License

MIT

## Related Projects

- [rig](https://github.com/0xPlaygrounds/rig) - Rust LLM framework (we use a fork)
- [agent-client-protocol](https://github.com/anthropics/agent-client-protocol) - ACP spec and Rust SDK
- [Zed](https://zed.dev) - Editor with ACP support
