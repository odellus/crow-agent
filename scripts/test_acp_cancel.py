#!/usr/bin/env python3
"""Test ACP cancel functionality

This script tests that the ACP server properly handles cancellation:
1. Sends initialize and creates a session
2. Sends a prompt that would take some time
3. Sends a cancel notification shortly after
4. Verifies the response has stopReason: cancelled

Usage:
    python3 scripts/test_acp_cancel.py
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


def send_notification(proc, method, params):
    """Send JSON-RPC notification (no id)"""
    request = {"jsonrpc": "2.0", "method": method, "params": params}
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
        bufsize=1
    )

    try:
        # Initialize
        print("Sending initialize...")
        send_jsonrpc(proc, "initialize", {"protocolVersion": "0.1"}, 1)
        resp = read_response(proc)
        if not resp or "result" not in resp:
            print(f"FAIL: Bad initialize response: {resp}")
            return 1
        print(f"  Agent: {resp['result']['agentInfo']['name']} v{resp['result']['agentInfo']['version']}")

        # Create session
        print("Sending session/new...")
        send_jsonrpc(proc, "session/new", {"cwd": "/tmp", "mcpServers": []}, 2)
        resp = read_response(proc)
        if not resp or "result" not in resp:
            print(f"FAIL: Bad session/new response: {resp}")
            return 1
        session_id = resp["result"]["sessionId"]
        print(f"  Session ID: {session_id}")

        # Send a prompt that will take some time
        print(f"\nSending prompt to session {session_id}...")
        send_jsonrpc(proc, "session/prompt", {
            "sessionId": session_id,
            "prompt": [{"type": "text", "text": "Please think carefully and explain quantum physics in detail."}]
        }, 3)

        # Wait briefly then send cancel
        cancel_delay = 0.5
        print(f"Waiting {cancel_delay}s before cancel...")
        time.sleep(cancel_delay)
        print("Sending cancel notification...")
        send_notification(proc, "session/cancel", {"sessionId": session_id})

        # Read responses until we get the result
        print("\nWaiting for response...")
        for i in range(20):
            resp = read_response(proc, timeout=2)
            if resp:
                if "method" in resp:
                    # Notification
                    update = resp.get("params", {}).get("update", {})
                    update_type = update.get("sessionUpdate", "unknown")
                    print(f"  Notification: {update_type}")
                elif "result" in resp:
                    if resp.get("id") == 3:
                        stop_reason = resp["result"].get("stopReason")
                        print(f"\n=== Result ===")
                        print(f"Stop reason: {stop_reason}")
                        if stop_reason == "cancelled":
                            print("\nSUCCESS: Cancel was handled correctly!")
                            return 0
                        else:
                            print(f"\nFAIL: Expected 'cancelled', got '{stop_reason}'")
                            return 1
            else:
                print("  (timeout waiting for response)")

        print("\nFAIL: No result received")
        return 1

    finally:
        proc.terminate()
        proc.wait()


if __name__ == "__main__":
    sys.exit(main())
