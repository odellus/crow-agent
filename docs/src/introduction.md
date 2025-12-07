# Crow Agent

Crow Agent is an AI coding assistant built on the [rig](https://github.com/0xPlaygrounds/rig) framework. It supports multiple interfaces:

- **CLI**: Interactive command-line chat interface
- **ACP**: Agent Client Protocol server for integration with editors like Zed

## Key Features

- **Streaming responses**: Real-time streaming of LLM responses
- **Multi-turn conversations**: Full chat history management
- **Tool execution**: Built-in tools for file operations, search, and more
- **Telemetry**: OpenTelemetry integration for observability
- **Cancellation support**: Proper handling of user interrupts

## Project Structure

```
crow_agent/
├── src/
│   ├── main.rs          # CLI entry point
│   ├── agent.rs         # Core CrowAgent implementation
│   ├── acp.rs           # ACP server implementation
│   ├── hooks.rs         # Telemetry hooks
│   ├── telemetry.rs     # OpenTelemetry setup
│   ├── config.rs        # Configuration
│   └── tools/           # Tool implementations
├── rig/                 # Forked rig framework
└── docs/                 # This documentation
```

## Quick Start

```bash
# Run CLI mode
cargo run -- chat

# Run ACP server mode
cargo run -- acp
```
