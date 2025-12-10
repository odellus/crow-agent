# Changelog

## [Unreleased] - 2025-12-10

### Added

- **Session Continuation** - New `--session/-s` flag for `prompt` command allows continuing previous sessions
  ```bash
  crow-agent-dev prompt -s fa73dee4 "what did we discuss?"
  ```
  Loads message history from the specified session and continues the conversation.

- **Model-Specific System Prompts** - System prompts are now selected based on model ID:
  - `claude*` → anthropic.txt (full Claude-optimized prompt)
  - `gpt-5*` → codex.txt
  - `gpt-*`, `o1*`, `o3*` → beast.txt
  - `gemini*` → gemini.txt
  - `polaris*` → polaris.txt
  - Others → qwen.txt (default)

- **Environment Context in Prompts** - System prompts now include:
  - `<env>` block with working directory, git status, platform, date
  - `<files>` block with file tree (up to 200 files) for git repos
  - Custom instructions from AGENTS.md, CLAUDE.md, or CONTEXT.md

- **File Tree in Prompts** - For git repositories, the system prompt now includes a file listing (like opencode), providing better context for the agent.

### Changed

- **Branding** - All prompts updated from "opencode" to "Crow"
- **Tool Schemas** - Fixed to match opencode conventions:
  - `read_file`: Uses `filePath` (camelCase), output in `cat -n` format
  - `edit`: Uses `filePath`, `oldString`, `newString` (camelCase)
  - `bash`: Requires `description` alongside `command`
  - `grep`: Simplified to `pattern`, `path`, `include` params
  - `list_directory`: Uses `path`, `pattern`, `recursive` params

### Fixed

- System prompt now properly includes all components (~8K tokens for system prompt in git repos)
- Tool parameter names use camelCase with serde aliases for backwards compatibility
- Session replay works correctly via `crow-agent-dev replay <session-id>`

### Infrastructure

- **Snapshot System** - Git-based snapshot system in `snapshot.rs` for tracking and reverting file changes (infrastructure ready, not yet wired to tools)
- **Telemetry** - Full LLM call tracing with request/response capture in SQLite

## [0.1.0] - 2025-11-XX

Initial release with:
- ACP server implementation
- 14 built-in tools
- REPL mode
- Telemetry system
- Multiple provider support (OpenRouter, LM Studio, etc.)
