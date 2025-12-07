#!/usr/bin/env python3
"""Test ACP multi-turn conversation

This script tests that the ACP server maintains conversation history:
1. Sends a message introducing a name
2. Asks "what is my name?" - should remember
3. Verifies the agent remembered the name

Usage:
    python3 scripts/test_acp_multi_turn.py
"""

import subprocess
import json
import time
import select
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


def wait_for_result(proc, expected_id, timeout=60):
    """Read until we get a result with the expected id, collecting text"""
    text_chunks = []
    start = time.time()
    while time.time() - start < timeout:
        resp = read_response(proc, timeout=5)
        if not resp:
            continue
        if "method" in resp and resp["method"] == "session/update":
            update = resp["params"]["update"]
            if update.get("sessionUpdate") == "agent_message_chunk":
                text = update["content"].get("text", "")
                text_chunks.append(text)
        elif "result" in resp and resp.get("id") == expected_id:
            return resp["result"], "".join(text_chunks)
    return None, "".join(text_chunks)


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
        send_jsonrpc(proc, "initialize", {"protocolVersion": "0.1"}, 1)
        resp = read_response(proc)
        if not resp or "result" not in resp:
            print(f"FAIL: Bad initialize response")
            return 1

        # Create session
        send_jsonrpc(proc, "session/new", {"cwd": "/tmp", "mcpServers": []}, 2)
        resp = read_response(proc)
        if not resp or "result" not in resp:
            print(f"FAIL: Bad session/new response")
            return 1
        session_id = resp["result"]["sessionId"]
        print(f"Session: {session_id}\n")

        # First message - introduce ourselves
        print("=== Turn 1: Introduce ===")
        send_jsonrpc(
            proc,
            "session/prompt",
            {
                "sessionId": session_id,
                "prompt": [
                    {
                        "type": "text",
                        "text": "My name is Alice and I like pizza. Remember this!",
                    }
                ],
            },
            3,
        )
        result, text = wait_for_result(proc, 3)
        if not result:
            print("FAIL: No response to first prompt")
            return 1
        print(f"Response: {text[:150]}..." if len(text) > 150 else f"Response: {text}")
        print(f"Stop: {result['stopReason']}\n")

        # Second message - ask about name
        print("=== Turn 2: Ask Name ===")
        send_jsonrpc(
            proc,
            "session/prompt",
            {
                "sessionId": session_id,
                "prompt": [{"type": "text", "text": "What is my name?"}],
            },
            4,
        )
        result, text = wait_for_result(proc, 4)
        if not result:
            print("FAIL: No response to second prompt")
            return 1
        print(f"Response: {text}")

        success = True
        if "alice" in text.lower():
            print("SUCCESS: Agent remembered the name!")
        else:
            print("FAIL: Agent did not remember the name")
            success = False
        print(f"Stop: {result['stopReason']}\n")

        # Third message - ask about food
        print("=== Turn 3: Ask Food ===")
        send_jsonrpc(
            proc,
            "session/prompt",
            {
                "sessionId": session_id,
                "prompt": [{"type": "text", "text": "What food do I like?"}],
            },
            5,
        )
        result, text = wait_for_result(proc, 5)
        if not result:
            print("FAIL: No response to third prompt")
            return 1
        print(f"Response: {text}")

        if "pizza" in text.lower():
            print("SUCCESS: Agent remembered the food!")
        else:
            print("FAIL: Agent did not remember the food")
            success = False
        print(f"Stop: {result['stopReason']}")

        return 0 if success else 1

    finally:
        proc.terminate()
        proc.wait()


if __name__ == "__main__":
    sys.exit(main())
