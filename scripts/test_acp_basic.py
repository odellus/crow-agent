#!/usr/bin/env python3
"""Basic ACP protocol test

This script tests basic ACP functionality:
1. Initialize connection
2. Create session
3. Send a simple prompt
4. Verify response

Usage:
    python3 scripts/test_acp_basic.py
"""

import json
import select
import subprocess
import sys


def send_jsonrpc(proc, method, params, id):
    """Send JSON-RPC request"""
    request = {"jsonrpc": "2.0", "method": method, "params": params, "id": id}
    msg = json.dumps(request)
    proc.stdin.write(msg + "\n")
    proc.stdin.flush()


def read_response(proc, timeout=30):
    """Read JSON-RPC response with timeout"""
    ready, _, _ = select.select([proc.stdout], [], [], timeout)
    if ready:
        line = proc.stdout.readline()
        if line:
            return json.loads(line)
    return None


def main():
    binary = "./target/release/crow-agent"

    # Start the ACP server
    print(f"Starting ACP server: {binary} acp")
    proc = subprocess.Popen(
        [binary, "acp"],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        bufsize=1,
    )

    try:
        # Initialize
        print("\n=== Initialize ===")
        send_jsonrpc(proc, "initialize", {"protocolVersion": "0.1"}, 1)
        resp = read_response(proc)
        if not resp or "result" not in resp:
            print(f"FAIL: Bad initialize response: {resp}")
            return 1
        agent_info = resp["result"]["agentInfo"]
        print(f"Agent: {agent_info['name']} v{agent_info['version']}")
        print(f"Protocol: {resp['result']['protocolVersion']}")

        # Create session
        print("\n=== Create Session ===")
        send_jsonrpc(proc, "session/new", {"cwd": "/tmp", "mcpServers": []}, 2)
        resp = read_response(proc)
        if not resp or "result" not in resp:
            print(f"FAIL: Bad session/new response: {resp}")
            return 1
        session_id = resp["result"]["sessionId"]
        print(f"Session ID: {session_id}")

        # Send prompt
        print("\n=== Send Prompt ===")
        send_jsonrpc(
            proc,
            "session/prompt",
            {
                "sessionId": session_id,
                "prompt": [{"type": "text", "text": "Say hello in exactly 3 words."}],
            },
            3,
        )

        # Collect responses
        print("\n=== Responses ===")
        text_chunks = []
        tool_calls = []
        for i in range(100):
            resp = read_response(proc, timeout=5)
            if not resp:
                continue

            if "method" in resp and resp["method"] == "session/update":
                update = resp["params"]["update"]
                update_type = update.get("sessionUpdate", "unknown")

                if update_type == "agent_message_chunk":
                    text = update["content"].get("text", "")
                    text_chunks.append(text)
                    print(f"  Text chunk: {repr(text)}")
                elif update_type == "agent_thought_chunk":
                    print(f"  Thought chunk")
                elif update_type == "tool_call":
                    tool_calls.append(update.get("title", "unknown"))
                    print(f"  Tool call: {update.get('title')}")
                elif update_type == "tool_call_update":
                    print(f"  Tool done: {update.get('toolCallId')}")
                elif update_type == "plan":
                    print(f"  Plan update: {len(update.get('entries', []))} entries")
                else:
                    print(f"  Update: {update_type}")

            elif "result" in resp:
                print(f"\n=== Final Result ===")
                print(f"Stop reason: {resp['result']['stopReason']}")
                break

        full_response = "".join(text_chunks)
        print(f"\nFull response: {full_response}")
        print(f"Tool calls: {tool_calls}")

        if resp and resp.get("result", {}).get("stopReason") == "end_turn":
            print("\nSUCCESS: Basic ACP test passed!")
            return 0
        else:
            print("\nFAIL: Unexpected stop reason")
            return 1

    finally:
        proc.terminate()
        proc.wait()


if __name__ == "__main__":
    sys.exit(main())
