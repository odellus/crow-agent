# AGENTS.md - AI Assistant Context for Crow Agent

This file is for YOU - the AI agent working on this codebase. Crow Agent is an agent framework built BY agents, FOR agents.

## The Most Important Thing: Testing Your Changes

When you modify crow-agent, you can immediately run it and inspect exactly what happened via telemetry. This is your superpower.

### Quick Test Loop

```bash
# 1. Build
cd crow_agent && cargo build --release

# 2. Run a test prompt
./target/release/crow-agent prompt "Say hello in 5 words"

# 3. See what just happened
./target/release/crow-agent telemetry traces
```

### Full ACP Test (Streaming + Tools)

```python
#!/usr/bin/env python3
# Save as /tmp/test_crow.py and run with: uv run /tmp/test_crow.py
import subprocess, json

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
print("Init:", recv())

# Create session
send({"jsonrpc": "2.0", "id": 2, "method": "session/new", "params": {
    "cwd": "/tmp",
    "mcpServers": []
}})
resp = recv()
print("Session:", resp)
session_id = resp["result"]["sessionId"]

# Send prompt (will trigger tool calls if you ask for something like todo_write)
send({"jsonrpc": "2.0", "id": 3, "method": "session/prompt", "params": {
    "sessionId": session_id,
    "prompt": [{"type": "text", "text": "Add 'test task 1' and 'test task 2' to the todo list"}]
}})

# Stream responses
while True:
    resp = recv()
    if "method" in resp:
        update = resp.get("params", {}).get("update", {})
        session_update = update.get("sessionUpdate")
        if session_update == "agent_message_chunk":
            print(update.get("content", {}).get("text", ""), end="", flush=True)
        elif session_update == "tool_call":
            print(f"\n[TOOL] {update.get('name')}")
    elif resp.get("id") == 3:
        print(f"\n\nDone: {resp.get('result', {}).get('stopReason')}")
        break

proc.terminate()
```

### Inspecting Telemetry

Every LLM call and tool execution is logged to `~/.crow_agent/telemetry.db`:

```bash
# List recent traces (shows: time, id, provider, latency, response preview)
crow-agent telemetry traces

# Output:
# 22:44:10 f469e1d0 openrouter   6760ms [bbbe69f2] Hello there, how are you?
# 22:43:37 048af305 openrouter  22677ms [7001b114]
# 22:25:40 4be401f4 openrouter   5507ms [6d92ec0a]

# Get full details for a trace (use first 8 chars of ID)
crow-agent telemetry trace f469e1d0

# Output shows:
# - Full request messages (system prompt + user message)
# - Response content (accumulated from streaming)
# - Tool calls made (with arguments)
# - Token counts, latency, model info

# Run arbitrary SQL queries
crow-agent telemetry sql "SELECT response_content, response_tool_calls FROM traces ORDER BY started_at DESC LIMIT 1"

# See the schema
crow-agent telemetry schema
```

### Key Telemetry Tables

**`traces`** - Every LLM completion call:
```sql
SELECT 
    id,
    started_at,
    model_provider,
    model_id,
    latency_ms,
    input_tokens,
    output_tokens,
    request_messages,    -- Full request JSON
    response_content,    -- Accumulated response text
    response_tool_calls  -- Tool calls JSON array
FROM traces;
```

**`tool_calls`** - Every tool execution:
```sql
SELECT 
    tool_name,
    arguments,
    result,
    duration_ms,
    success
FROM tool_calls;
```

### Example: Verify Tool Calls Work

```bash
# Run a prompt that should trigger todo_write
./target/release/crow-agent prompt "Add 'buy milk' to my todo list"

# Check what tool calls were made
crow-agent telemetry sql "SELECT response_tool_calls FROM traces ORDER BY started_at DESC LIMIT 1"

# Output:
# [{"arguments":{"todos":[{"activeForm":"Adding...","content":"buy milk","status":"pending"}]},"id":"...","name":"todo_write"}]
```

## Project Overview

Crow Agent is a standalone LLM-powered coding agent written in Rust. It provides:
- An ACP (Agent Client Protocol) server for editor integration (Zed, etc.)
- A CLI REPL for interactive use  
- Comprehensive telemetry and tracing
- 14 built-in tools for file operations, search, terminal, web, etc.

## Tech Stack

- **Language**: Rust (nightly, uses `let_chains` feature)
- **LLM Framework**: [rig](https://github.com/0xPlaygrounds/rig) (forked in `rig/` directory)
- **Protocol**: ACP over JSON-RPC 2.0 via stdio
- **Database**: SQLite (rusqlite) for telemetry
- **Async Runtime**: Tokio
- **CLI**: Clap
- **Templating**: Handlebars (system prompts)

## Directory Structure

```
crow_agent/
├── src/
│   ├── main.rs           # CLI entry, command parsing, REPL loop
│   ├── lib.rs            # Public exports
│   ├── agent.rs          # CrowAgent - builds rig agent with tools  
│   ├── acp.rs            # ACP server - CrowAcpAgent trait impl
│   ├── config.rs         # Config, LlmConfig, LlmProvider
│   ├── auth.rs           # AuthConfig - loads ~/.crow_agent/auth.json
│   ├── hooks.rs          # TelemetryHook - rig PromptHook impl
│   ├── telemetry.rs      # Telemetry - SQLite logging, stats
│   ├── trace_layer.rs    # SqliteTraceLayer - captures gen_ai.* spans
│   ├── templates/
│   │   ├── mod.rs        # Template structs
│   │   └── system.hbs    # System prompt Handlebars template
│   └── tools/
│       ├── mod.rs        # Tool exports
│       ├── read_file.rs  # ReadFile tool
│       ├── edit_file.rs  # EditFile tool (fuzzy replacement)
│       ├── grep.rs       # Regex search
│       ├── find_path.rs  # Glob file finder
│       ├── terminal.rs   # Shell command execution
│       ├── thinking.rs   # Reasoning scratchpad
│       ├── todo.rs       # TodoWrite, TodoRead, TodoStore
│       ├── fetch.rs      # URL fetcher
│       ├── web_search.rs # SearXNG search
│       ├── diagnostics.rs # LSP diagnostics
│       └── ...
├── rig/                  # Forked rig-core with telemetry enhancements
│   └── rig-core/
│       └── src/providers/openrouter/streaming.rs
└── Cargo.toml
```

## Core Architecture

### Agent Flow

1. **`main.rs`** parses CLI args, builds `Config`, initializes `Telemetry`
2. **`CrowAgent::new()`** creates agent with config and telemetry
3. **`build_agent()`** in `agent.rs` constructs rig `Agent` with:
   ```rust
   client
       .agent(&self.config.llm.model)
       .preamble(&self.system_prompt())
       .tool(ReadFile::new(wd.clone()))
       .tool(EditFile::new(wd.clone()))
       .tool(Terminal::new(wd.clone()))
       // ... all 14 tools
       .build()
   ```
4. **Execution** via `.multi_turn(20)`:
   - `agent.chat()` - non-streaming, history passed by `&mut`
   - `agent.chat_stream()` - streaming, history passed by value (caller must update)

### ACP Server Flow (`acp.rs`)

1. `run_stdio_server()` starts JSON-RPC on stdin/stdout
2. `session/new` creates `Session` with its own `CrowAgent` + history
3. `session/prompt` calls `agent.chat_stream()`:
   - Text → `SessionUpdate::AgentMessageChunk`
   - Tool calls → `SessionUpdate::ToolCall`
   - Tool results → `SessionUpdate::ToolCallUpdate`
   - Todo writes → `SessionUpdate::Plan`
4. `session/cancel` sets `cancelled` flag + calls `hook.cancel()`

### Telemetry Architecture

Two systems capture data:

**`TelemetryHook`** (`hooks.rs`):
- Implements rig's `PromptHook` / `StreamingPromptHook` traits
- Captures: tool calls with timing, cancel signals
- Logs to `tool_calls` table

**`SqliteTraceLayer`** (`trace_layer.rs`):
- Tracing `Layer` intercepting `gen_ai.*` spans from rig
- Captures: request/response bodies, tokens, model info
- Logs to `traces` table on span close

**Rig Fork** (`rig/rig-core/src/providers/openrouter/streaming.rs`):
- Accumulates `response_content` during streaming
- Accumulates `response_tool_calls` during streaming
- Records to span before `FinalResponse` yield
- Without this, streaming responses would show NULL in telemetry

## Key Files Reference

| File | What It Does | Modify When |
|------|--------------|-------------|
| `agent.rs:107-178` | `build_agent()` - assembles tools | Adding new tools |
| `agent.rs:230-248` | `chat_stream()` - streaming entry | Changing streaming behavior |
| `acp.rs:149-350` | `prompt()` - ACP stream handling | Protocol changes |
| `acp.rs:352-375` | `cancel()` - cancellation | Cancel behavior |
| `tools/*.rs` | Tool implementations | Adding/modifying tools |
| `hooks.rs` | Event capture + cancellation | Telemetry additions |
| `trace_layer.rs` | Span → SQLite | New telemetry fields |
| `templates/system.hbs` | System prompt | Prompt engineering |
| `rig/.../streaming.rs` | Response accumulation | Streaming telemetry |

## Common Tasks

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
pub struct MyTool { /* fields */ }

impl Tool for MyTool {
    const NAME: &'static str = "my_tool";
    type Error = String;
    type Args = MyToolArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: "my_tool".to_string(),
            description: "Does X".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "input": {"type": "string", "description": "The input"}
                },
                "required": ["input"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        Ok(format!("Result: {}", args.input))
    }
}
```

2. Export in `src/tools/mod.rs`:
```rust
mod my_tool;
pub use my_tool::MyTool;
```

3. Register in `agent.rs` `build_agent()`:
```rust
.tool(MyTool::new())
```

4. Add to `available_tools()` in `agent.rs`

5. **Test it**:
```bash
cargo build --release
./target/release/crow-agent prompt "Use my_tool with input 'hello'"
crow-agent telemetry sql "SELECT response_tool_calls FROM traces ORDER BY started_at DESC LIMIT 1"
```

### Adding Telemetry Fields

1. Add to `TraceData` in `trace_layer.rs`:
```rust
pub my_field: Option<String>,
```

2. Capture in `on_record()`:
```rust
if let Some(val) = visitor.fields.get("gen_ai.my_field") {
    trace.my_field = Some(val.clone());
}
```

3. Add to `save_trace()` SQL INSERT

4. If from streaming, also modify `rig/rig-core/src/providers/openrouter/streaming.rs`:
```rust
// Add to span definition
gen_ai.my_field = tracing::field::Empty,

// Record before FinalResponse
span.record("gen_ai.my_field", &my_value);
```

### Debugging a Problem

```bash
# 1. Run with verbose
crow-agent -v prompt "your test"

# 2. Check what was sent/received
crow-agent telemetry trace <id>

# 3. Check specific fields
crow-agent telemetry sql "SELECT request_messages, response_content FROM traces WHERE id LIKE '<id>%'"

# 4. Check tool execution
crow-agent telemetry sql "SELECT * FROM tool_calls ORDER BY id DESC LIMIT 5"
```

## Build & Test Commands

```bash
# Build
cargo build              # debug
cargo build --release    # release

# Test
cargo test

# Run
./target/release/crow-agent repl           # interactive
./target/release/crow-agent prompt "..."   # single shot
./target/release/crow-agent acp            # ACP server mode

# Inspect
crow-agent telemetry traces                # list traces
crow-agent telemetry trace <id>            # trace details
crow-agent telemetry sql "..."             # raw SQL
crow-agent telemetry schema                # see tables
```

## Environment Variables

| Variable | Purpose |
|----------|---------|
| `OPENROUTER_API_KEY` | Default API key |
| `SEARXNG_URL` | Web search endpoint (default: `http://localhost:8082`) |
| `RUST_LOG` | Tracing filter (e.g., `crow_agent=debug`) |

## Auth Configuration

`~/.crow_agent/auth.json`:
```json
{
  "openrouter": {
    "api_key": "sk-or-v1-..."
  },
  "lm-studio": {
    "api_key": "lm-studio", 
    "base_url": "http://localhost:1234/v1"
  }
}
```

Provider auto-detected from model prefix: `anthropic/claude-3.5-sonnet` → uses `anthropic` entry.

## Security Notes

- **Path traversal**: All file tools validate paths within `working_dir`
- **Binary files**: `read_file` rejects binary content
- **API keys**: In `auth.json`, not hardcoded

## Remember

**You can always check what happened:**
```bash
crow-agent telemetry traces
crow-agent telemetry trace <id>
```

This is your feedback loop. Use it after every change.
