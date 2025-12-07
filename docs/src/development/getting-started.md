# Getting Started

This guide covers setting up a development environment for Crow Agent.

## Prerequisites

- Rust toolchain (rustup recommended)
- Python 3 (for testing scripts)
- OpenRouter API key

## Clone and Setup

```bash
# Clone the repository
git clone <repo-url>
cd crow_agent

# The rig fork is included as a subdirectory
ls rig/  # Should show rig-core, etc.
```

## Environment Variables

Create a `.env` file or export these:

```bash
# Required: OpenRouter API key
export OPENROUTER_API_KEY="sk-or-..."

# Optional: Model selection
export CROW_MODEL="anthropic/claude-sonnet-4-20250514"

# Optional: Telemetry
export OTEL_EXPORTER_OTLP_ENDPOINT="http://localhost:4317"
export OTEL_SERVICE_NAME="crow-agent"
```

## Build

```bash
# Debug build
cargo build

# Release build (faster, smaller)
cargo build --release
```

## Run

### CLI Mode

```bash
# Interactive chat
cargo run -- chat

# Or with release build
./target/release/crow-agent chat
```

### ACP Mode

```bash
# Start ACP server (for Zed integration)
cargo run -- acp

# Or with release build
./target/release/crow-agent acp
```

## Verify Installation

### Test CLI

```bash
$ ./target/release/crow-agent chat
> Hello!
Hello! How can I help you today?
> /exit
```

### Test ACP

```bash
# In one terminal
./target/release/crow-agent acp

# In another terminal, send JSON-RPC
echo '{"jsonrpc":"2.0","method":"initialize","params":{"protocolVersion":"0.1"},"id":1}' | nc localhost -U /dev/stdin
```

Or use the Python test script:

```bash
uv run scripts/test_acp_basic.py
```

## Project Structure

```
crow_agent/
├── Cargo.toml           # Project manifest
├── src/
│   ├── main.rs          # Entry point, CLI parsing
│   ├── agent.rs         # CrowAgent implementation
│   ├── acp.rs           # ACP server
│   ├── hooks.rs         # Telemetry hooks
│   ├── telemetry.rs     # OpenTelemetry setup
│   ├── config.rs        # Configuration
│   └── tools/           # Tool implementations
│       ├── mod.rs
│       ├── read_file.rs
│       ├── edit_file.rs
│       └── ...
├── rig/                 # Forked rig framework
│   └── rig-core/
│       └── src/
│           └── providers/
│               └── openrouter/
│                   ├── completion.rs
│                   └── streaming.rs
└── docs/                 # This documentation
    ├── book.toml
    └── src/
        └── ...
```

## Next Steps

- Read [Architecture Overview](../architecture/overview.md)
- Understand [Rig Integration](../architecture/rig-integration.md)
- Learn about [ACP Protocol](../acp/overview.md)
- Set up [Debugging](./debugging.md)
