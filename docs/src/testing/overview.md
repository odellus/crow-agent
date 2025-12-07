# Testing Overview

Crow Agent testing includes unit tests, integration tests, and manual end-to-end testing.

## Test Types

| Type | Location | Purpose |
|------|----------|---------|
| Unit tests | `src/*.rs` | Test individual functions |
| Integration tests | `tests/` | Test component integration |
| Manual ACP tests | Python scripts | End-to-end ACP testing |
| UI testing | Zed editor | Real-world usage |

## Running Tests

### Unit Tests

```bash
cd crow_agent
cargo test
```

### With Logging

```bash
RUST_LOG=debug cargo test -- --nocapture
```

## Test Coverage Areas

### Agent (`agent.rs`)

- Agent creation with config
- Streaming vs non-streaming paths
- Hook attachment
- History management

### ACP (`acp.rs`)

- Session creation
- Prompt handling
- Cancellation
- Stream notification conversion

### Hooks (`hooks.rs`)

- Tool call timing
- Cancel signal storage
- Telemetry logging

### Telemetry (`telemetry.rs`)

- SQLite persistence
- OTLP export
- Span attributes

## Manual Testing Workflow

1. **Build release binary**
   ```bash
   cargo build --release
   ```

2. **Test ACP protocol**
   ```bash
   uv run scripts/test_acp.py
   ```

3. **Test cancellation**
   ```bash
   uv run scripts/test_cancel.py
   ```

4. **Test in Zed**
   - Configure Zed to use crow-agent
   - Send prompts
   - Verify streaming works
   - Test cancel button

5. **Verify telemetry**
   ```bash
   sqlite3 ~/.crow/telemetry.db "SELECT * FROM tool_calls LIMIT 5"
   ```
