# ACP Testing

Testing the ACP server requires sending JSON-RPC messages over stdio. This can be done manually or with test scripts.

## Manual Testing

### Start the Server

```bash
./target/release/crow-agent acp
```

The server reads from stdin and writes to stdout.

### Send Messages

Type JSON-RPC messages followed by newline:

```json
{"jsonrpc":"2.0","method":"initialize","params":{"protocolVersion":"0.1"},"id":1}
```

### Expected Response

```json
{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":1,"agentCapabilities":{...},"agentInfo":{"name":"crow-agent","version":"0.1.0"}}}
```

## Test Sequence

### 1. Initialize

```json
{"jsonrpc":"2.0","method":"initialize","params":{"protocolVersion":"0.1"},"id":1}
```

Response includes:
- `protocolVersion`: 1
- `agentCapabilities`: What the agent supports
- `agentInfo`: Name and version

### 2. Create Session

```json
{"jsonrpc":"2.0","method":"session/new","params":{"cwd":"/tmp","mcpServers":[]},"id":2}
```

Response:
```json
{"jsonrpc":"2.0","id":2,"result":{"sessionId":"0"}}
```

**Note**: `mcpServers` is required (can be empty array).

### 3. Send Prompt

```json
{"jsonrpc":"2.0","method":"session/prompt","params":{"sessionId":"0","prompt":[{"type":"text","text":"Hello!"}]},"id":3}
```

Response includes notifications (streaming) followed by result:

```json
{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"0","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"Hello"}}}}
{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"0","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"!"}}}}
{"jsonrpc":"2.0","id":3,"result":{"stopReason":"end_turn"}}
```

### 4. Cancel (Optional)

```json
{"jsonrpc":"2.0","method":"session/cancel","params":{"sessionId":"0"}}
```

This is a notification (no id), no response expected.

## Python Test Script

Create `test_acp.py`:

```python
#!/usr/bin/env python3
"""Test ACP server"""
import subprocess
import json
import select

def send_jsonrpc(proc, method, params, id):
    request = {"jsonrpc": "2.0", "method": method, "params": params, "id": id}
    proc.stdin.write(json.dumps(request) + "\n")
    proc.stdin.flush()

def read_response(proc, timeout=30):
    ready, _, _ = select.select([proc.stdout], [], [], timeout)
    if ready:
        line = proc.stdout.readline()
        if line:
            return json.loads(line)
    return None

def main():
    proc = subprocess.Popen(
        ["./target/release/crow-agent", "acp"],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        bufsize=1
    )
    
    try:
        # Initialize
        print("Sending initialize...")
        send_jsonrpc(proc, "initialize", {"protocolVersion": "0.1"}, 1)
        resp = read_response(proc)
        print(f"Response: {resp}")
        
        # Create session
        print("\nSending session/new...")
        send_jsonrpc(proc, "session/new", {"cwd": "/tmp", "mcpServers": []}, 2)
        resp = read_response(proc)
        print(f"Response: {resp}")
        session_id = resp["result"]["sessionId"]
        
        # Send prompt
        print(f"\nSending prompt to session {session_id}...")
        send_jsonrpc(proc, "session/prompt", {
            "sessionId": session_id,
            "prompt": [{"type": "text", "text": "What is 2+2?"}]
        }, 3)
        
        # Read responses
        print("\nReading responses...")
        for i in range(50):
            resp = read_response(proc, timeout=5)
            if resp:
                if "method" in resp:
                    print(f"Notification: {resp['method']}")
                elif "result" in resp:
                    print(f"Result: {resp['result']}")
                    break
            else:
                print("(timeout)")
                
    finally:
        proc.terminate()
        proc.wait()

if __name__ == "__main__":
    main()
```

## Testing Multi-Turn

```python
# First prompt
send_jsonrpc(proc, "session/prompt", {
    "sessionId": session_id,
    "prompt": [{"type": "text", "text": "My name is Alice"}]
}, 3)
# ... read responses ...

# Second prompt - should remember name
send_jsonrpc(proc, "session/prompt", {
    "sessionId": session_id,
    "prompt": [{"type": "text", "text": "What's my name?"}]
}, 4)
# ... read responses - should mention "Alice" ...
```

## Common Issues

### "mcpServers" Required

```
Error: invalid params
```

Solution: Include `"mcpServers": []` in session/new params.

### Session Not Found

```
Error: invalid params
```

Solution: Ensure session was created and use correct sessionId.

### No Response

Possible causes:
- Server crashed (check stderr)
- Request malformed (missing newline)
- Waiting on LLM (give it time)
