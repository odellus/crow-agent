# Debugging

## Logging

### Enable Debug Logs

```bash
# All debug logs
RUST_LOG=debug cargo run -- acp

# Specific modules
RUST_LOG=crow_agent=debug,rig=info cargo run -- acp

# Very verbose
RUST_LOG=trace cargo run -- acp
```

### Log Levels

| Level | Use |
|-------|-----|
| `error` | Failures that stop operation |
| `warn` | Recoverable issues |
| `info` | Key events (tool calls, completions) |
| `debug` | Detailed flow information |
| `trace` | Very verbose, includes all data |

### Recommended Development Settings

```bash
export RUST_LOG="crow_agent=debug,rig=debug,agent_client_protocol=info"
```

## ACP Debugging

### Capture All Traffic

Create a wrapper script to log all ACP traffic:

```bash
#!/bin/bash
# debug_acp.sh
tee /tmp/acp_input.log | ./target/release/crow-agent acp | tee /tmp/acp_output.log
```

Then examine:

```bash
# Input (from client)
cat /tmp/acp_input.log

# Output (to client)
cat /tmp/acp_output.log
```

### Test Individual Messages

```python
#!/usr/bin/env python3
"""Send a single ACP message and see response"""
import subprocess
import json
import sys

message = json.loads(sys.argv[1])

proc = subprocess.Popen(
    ["./target/release/crow-agent", "acp"],
    stdin=subprocess.PIPE,
    stdout=subprocess.PIPE,
    stderr=subprocess.PIPE,
    text=True
)

proc.stdin.write(json.dumps(message) + "\n")
proc.stdin.flush()

# Read responses for 5 seconds
import select
while True:
    ready, _, _ = select.select([proc.stdout], [], [], 5)
    if ready:
        line = proc.stdout.readline()
        if line:
            print(json.dumps(json.loads(line), indent=2))
    else:
        break

proc.terminate()
```

Usage:

```bash
uv run debug_msg.py '{"jsonrpc":"2.0","method":"initialize","params":{"protocolVersion":"0.1"},"id":1}'
```

## Telemetry Debugging

### Check SQLite Database

```bash
sqlite3 ~/.crow/telemetry.db

# List tables
.tables

# Recent tool calls
SELECT * FROM tool_calls ORDER BY created_at DESC LIMIT 10;

# Tool call durations
SELECT tool_name, AVG(duration_ms), COUNT(*) 
FROM tool_calls 
GROUP BY tool_name;
```

### Verify OTLP Export

```bash
# Start Jaeger
docker run -d --name jaeger \
  -p 16686:16686 \
  -p 4317:4317 \
  jaegertracing/all-in-one:latest

# Run with OTLP export
OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4317 \
OTEL_SERVICE_NAME=crow-agent \
./target/release/crow-agent acp

# View traces at http://localhost:16686
```

## Rig Debugging

### Stream Item Inspection

Add logging to see stream items:

```rust
while let Some(item) = stream.next().await {
    tracing::debug!(?item, "Stream item received");
    // ...
}
```

### Hook Debugging

The TelemetryHook logs events:

```
DEBUG Tool call starting tool="read_file" args_len=42
INFO  Tool call completed tool="read_file" duration_ms=15
```

### Cancel Signal Debugging

```rust
// In hooks.rs
pub async fn cancel(&self) {
    if let Some(ref signal) = *self.cancel_signal.lock().await {
        tracing::info!("Cancel signal triggered");
        signal.cancel();
    } else {
        tracing::warn!("No cancel signal stored");
    }
}
```

## Common Issues

### Stream Never Completes

Check:
1. Is the model responding? (check OTLP/logs)
2. Is there a tool call loop? (max turns exceeded)
3. Is the cancelled flag being set incorrectly?

### Cancel Not Working

Check:
1. Is the hook being stored? (`active_hook` populated)
2. Is the cancel signal stored? (`cancel_signal` in hook)
3. Is the cancelled flag being checked? (logs)

### History Not Persisting

Check:
1. Is `FinalResponse` being received?
2. Is history being pushed correctly?
3. Are you using the same session ID?

## IDE Debugging

### VS Code

`.vscode/launch.json`:

```json
{
  "version": "0.2.0",
  "configurations": [
    {
      "type": "lldb",
      "request": "launch",
      "name": "Debug ACP",
      "cargo": {
        "args": ["build", "--bin=crow-agent"],
        "filter": {
          "name": "crow-agent",
          "kind": "bin"
        }
      },
      "args": ["acp"],
      "env": {
        "RUST_LOG": "debug",
        "OPENROUTER_API_KEY": "${env:OPENROUTER_API_KEY}"
      },
      "stdio": ["${workspaceFolder}/test_input.jsonl", null, null]
    }
  ]
}
```

### rust-analyzer

Ensure `rust-analyzer.cargo.features` includes any needed features.

## Performance Profiling

```bash
# CPU profiling with perf
perf record ./target/release/crow-agent acp
perf report

# Memory profiling with valgrind
valgrind --tool=massif ./target/release/crow-agent acp
ms_print massif.out.*
```
