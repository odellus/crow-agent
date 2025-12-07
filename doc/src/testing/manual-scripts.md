# Manual Testing Scripts

This page documents the Python scripts used for end-to-end testing of the ACP server.

## test_acp_basic.py

Basic ACP protocol test:

```python
#!/usr/bin/env python3
"""Basic ACP protocol test"""
import subprocess
import json
import select

def send_jsonrpc(proc, method, params, id):
    request = {"jsonrpc": "2.0", "method": method, "params": params, "id": id}
    proc.stdin.write(json.dumps(request) + "\n")
    proc.stdin.flush()

def send_notification(proc, method, params):
    request = {"jsonrpc": "2.0", "method": method, "params": params}
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
        print("=== Initialize ===")
        send_jsonrpc(proc, "initialize", {"protocolVersion": "0.1"}, 1)
        resp = read_response(proc)
        print(f"Agent: {resp['result']['agentInfo']['name']} v{resp['result']['agentInfo']['version']}")
        
        # Create session
        print("\n=== Create Session ===")
        send_jsonrpc(proc, "session/new", {"cwd": "/tmp", "mcpServers": []}, 2)
        resp = read_response(proc)
        session_id = resp["result"]["sessionId"]
        print(f"Session ID: {session_id}")
        
        # Send prompt
        print("\n=== Send Prompt ===")
        send_jsonrpc(proc, "session/prompt", {
            "sessionId": session_id,
            "prompt": [{"type": "text", "text": "Say hello in exactly 3 words."}]
        }, 3)
        
        # Collect responses
        print("\n=== Responses ===")
        text_chunks = []
        for i in range(100):
            resp = read_response(proc, timeout=5)
            if not resp:
                continue
            
            if "method" in resp and resp["method"] == "session/update":
                update = resp["params"]["update"]
                if update.get("sessionUpdate") == "agent_message_chunk":
                    text = update["content"].get("text", "")
                    text_chunks.append(text)
                    print(f"  Text: {repr(text)}")
                elif update.get("sessionUpdate") == "tool_call":
                    print(f"  Tool: {update.get('title')}")
                elif update.get("sessionUpdate") == "tool_call_update":
                    print(f"  Tool done: {update.get('toolCallId')}")
            elif "result" in resp:
                print(f"\nFinal: stopReason={resp['result']['stopReason']}")
                break
        
        print(f"\nFull response: {''.join(text_chunks)}")
        
    finally:
        proc.terminate()
        proc.wait()

if __name__ == "__main__":
    main()
```

## test_cancel.py

Test cancellation functionality:

```python
#!/usr/bin/env python3
"""Test ACP cancel functionality"""
import subprocess
import json
import time
import select

def send_jsonrpc(proc, method, params, id):
    request = {"jsonrpc": "2.0", "method": method, "params": params, "id": id}
    proc.stdin.write(json.dumps(request) + "\n")
    proc.stdin.flush()

def send_notification(proc, method, params):
    request = {"jsonrpc": "2.0", "method": method, "params": params}
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
        print(f"Initialize: OK")
        
        # Create session
        print("Sending session/new...")
        send_jsonrpc(proc, "session/new", {"cwd": "/tmp", "mcpServers": []}, 2)
        resp = read_response(proc)
        session_id = resp["result"]["sessionId"]
        print(f"Session: {session_id}")
        
        # Send a prompt that takes time
        print(f"\nSending prompt...")
        send_jsonrpc(proc, "session/prompt", {
            "sessionId": session_id,
            "prompt": [{"type": "text", "text": "Please think carefully and explain quantum physics."}]
        }, 3)
        
        # Wait briefly, then cancel
        time.sleep(0.5)
        print("Sending cancel...")
        send_notification(proc, "session/cancel", {"sessionId": session_id})
        
        # Read until we get the result
        print("\nWaiting for response...")
        for i in range(20):
            resp = read_response(proc, timeout=2)
            if resp:
                if "result" in resp and resp.get("id") == 3:
                    stop_reason = resp["result"].get("stopReason")
                    print(f"\n=== Result ===")
                    print(f"Stop reason: {stop_reason}")
                    if stop_reason == "cancelled":
                        print("SUCCESS: Cancel worked!")
                    else:
                        print(f"UNEXPECTED: Expected 'cancelled', got '{stop_reason}'")
                    break
        else:
            print("TIMEOUT: No result received")
                
    finally:
        proc.terminate()
        proc.wait()

if __name__ == "__main__":
    main()
```

## test_multi_turn.py

Test conversation history:

```python
#!/usr/bin/env python3
"""Test multi-turn conversation"""
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

def wait_for_result(proc, expected_id, timeout=60):
    """Read until we get a result with the expected id"""
    text_chunks = []
    start = time.time()
    while time.time() - start < timeout:
        resp = read_response(proc, timeout=5)
        if not resp:
            continue
        if "method" in resp and resp["method"] == "session/update":
            update = resp["params"]["update"]
            if update.get("sessionUpdate") == "agent_message_chunk":
                text_chunks.append(update["content"].get("text", ""))
        elif "result" in resp and resp.get("id") == expected_id:
            return resp["result"], "".join(text_chunks)
    return None, "".join(text_chunks)

import time

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
        send_jsonrpc(proc, "initialize", {"protocolVersion": "0.1"}, 1)
        read_response(proc)
        
        # Create session
        send_jsonrpc(proc, "session/new", {"cwd": "/tmp", "mcpServers": []}, 2)
        resp = read_response(proc)
        session_id = resp["result"]["sessionId"]
        print(f"Session: {session_id}\n")
        
        # First message - introduce ourselves
        print("=== Turn 1: Introduce ===")
        send_jsonrpc(proc, "session/prompt", {
            "sessionId": session_id,
            "prompt": [{"type": "text", "text": "My name is Alice and my favorite color is blue. Remember this!"}]
        }, 3)
        result, text = wait_for_result(proc, 3)
        print(f"Response: {text[:200]}...")
        print(f"Stop: {result['stopReason']}\n")
        
        # Second message - ask about name
        print("=== Turn 2: Ask Name ===")
        send_jsonrpc(proc, "session/prompt", {
            "sessionId": session_id,
            "prompt": [{"type": "text", "text": "What is my name?"}]
        }, 4)
        result, text = wait_for_result(proc, 4)
        print(f"Response: {text}")
        
        if "alice" in text.lower():
            print("SUCCESS: Agent remembered the name!")
        else:
            print("FAIL: Agent did not remember the name")
        print(f"Stop: {result['stopReason']}\n")
        
        # Third message - ask about color
        print("=== Turn 3: Ask Color ===")
        send_jsonrpc(proc, "session/prompt", {
            "sessionId": session_id,
            "prompt": [{"type": "text", "text": "What is my favorite color?"}]
        }, 5)
        result, text = wait_for_result(proc, 5)
        print(f"Response: {text}")
        
        if "blue" in text.lower():
            print("SUCCESS: Agent remembered the color!")
        else:
            print("FAIL: Agent did not remember the color")
        print(f"Stop: {result['stopReason']}")
                
    finally:
        proc.terminate()
        proc.wait()

if __name__ == "__main__":
    main()
```

## Running Tests

```bash
# Build first
cargo build --release

# Run basic test
uv run test_acp_basic.py

# Run cancel test
uv run test_cancel.py

# Run multi-turn test
uv run test_multi_turn.py
```

## Expected Output

### Cancel Test

```
Sending initialize...
Initialize: OK
Sending session/new...
Session: 0

Sending prompt...
Sending cancel...

Waiting for response...

=== Result ===
Stop reason: cancelled
SUCCESS: Cancel worked!
```

### Multi-Turn Test

```
Session: 0

=== Turn 1: Introduce ===
Response: Nice to meet you, Alice! I'll remember...
Stop: end_turn

=== Turn 2: Ask Name ===
Response: Your name is Alice.
SUCCESS: Agent remembered the name!
Stop: end_turn

=== Turn 3: Ask Color ===
Response: Your favorite color is blue.
SUCCESS: Agent remembered the color!
Stop: end_turn
```
