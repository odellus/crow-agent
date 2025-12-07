# Tools Implementation Plan

## Tools to Add (in order)

### 1. TODO Tool
Port from `agent_crate/src/tools/todo_tool.rs`

**Files to create:**
- `src/tools/todo.rs`

**Implementation:**
- `TodoStore` - shared state storage (HashMap<session_id, Vec<TodoItem>>)
- `TodoItem` struct: content, status (pending/in_progress/completed/cancelled), active_form
- `TodoWriteTool` - replaces entire todo list
- `TodoReadTool` - returns current todos

**Changes needed:**
- Remove gpui dependencies, use standard Rust
- Remove ACP Plan update (we don't have ACP event streams in tool layer)
- Add to agent.rs toolset
- Add TodoStore to CrowAgent struct

---

### 2. FETCH Tool
Port from `agent_crate/src/tools/fetch_tool.rs`

**Files to create:**
- `src/tools/fetch.rs`

**Implementation:**
- HTTP GET using reqwest
- Convert HTML to markdown using html_to_markdown crate
- Handle content types: HTML, JSON, plaintext

**Dependencies to add:**
- `html_to_markdown` (check if available on crates.io, else implement basic version)

---

### 3. WEB_SEARCH Tool
Based on `~/src/smolagents-example/search.py` (SearXNG)

**Files to create:**
- `src/tools/web_search.rs`

**Implementation:**
- Query SearXNG at configurable URL (env: SEARXNG_URL, default: http://localhost:8082)
- Parse JSON response with infoboxes and results
- Return formatted text with titles, URLs, content
- Limit parameter for number of results

**Dependencies:**
- reqwest (already have)
- serde for Response/SearchResult/Infobox structs

---

### 4. TASK_COMPLETE Tool
Port from `agent_crate/src/tools/task_complete_tool.rs`

**Files to create:**
- `src/tools/task_complete.rs`

**Implementation:**
- Signal that agent has completed the task
- Returns completion message
- Used by agent to indicate it's done

---

### 5. DIAGNOSTICS Tool (requires LSP)
Port from `agent_crate/src/tools/diagnostics_tool.rs`

**Files to create:**
- `src/tools/diagnostics.rs`
- `src/lsp.rs` (LSP client infrastructure)

**Implementation:**
- LSP client that connects to language servers
- Get diagnostics (errors/warnings) for a file or project
- Format output with severity, line number, message

**LSP servers to support initially:**
- rust-analyzer (Rust)
- typescript-language-server (TS/JS)
- pylsp or pyright (Python)

**This is the most complex - requires:**
- tower-lsp or lsp-types crate
- Spawning/managing LSP server processes
- LSP initialize/initialized handshake
- textDocument/diagnostic or workspace/diagnostic requests

---

## Changes to mod.rs

Add all new tools to `src/tools/mod.rs`:
```rust
pub mod todo;
pub mod fetch;
pub mod web_search;
pub mod task_complete;
pub mod diagnostics;
```

## Changes to agent.rs

1. Add TodoStore field to CrowAgent
2. Register all new tools in toolset
3. Pass TodoStore to todo tools

## Dependencies to add to Cargo.toml

```toml
html_to_markdown = "0.x"  # or similar
lsp-types = "0.x"  # for diagnostics
tower-lsp = "0.x"  # LSP client (if needed)
```
