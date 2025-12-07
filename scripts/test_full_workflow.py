#!/usr/bin/env python3
"""Full workflow test - tests tools, todo, edit, terminal

This test:
1. Creates a session in /tmp/crow_test
2. Asks the agent to use TodoWrite to plan the task
3. Asks to read hello.py, change "Hello World" to "Hello Universe"
4. Asks to run the script and verify it works

Usage:
    uv run scripts/test_full_workflow.py
"""

import json
import os
import select
import subprocess
import sys
import time


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


def wait_for_result(proc, expected_id, timeout=120):
    """Read until we get a result with the expected id, collecting events"""
    text_chunks = []
    tool_calls = []
    tool_updates = []
    plans = []

    start = time.time()
    while time.time() - start < timeout:
        resp = read_response(proc, timeout=5)
        if not resp:
            continue
        if "method" in resp and resp["method"] == "session/update":
            update = resp["params"]["update"]
            update_type = update.get("sessionUpdate", "unknown")

            if update_type == "agent_message_chunk":
                text = update["content"].get("text", "")
                text_chunks.append(text)
            elif update_type == "tool_call":
                tool_calls.append(
                    {
                        "id": update.get("toolCallId"),
                        "title": update.get("title"),
                        "kind": update.get("kind"),
                        "input": update.get("rawInput"),
                    }
                )
                print(f"  [TOOL] {update.get('title')}")
            elif update_type == "tool_call_update":
                tool_updates.append(
                    {"id": update.get("toolCallId"), "status": update.get("status")}
                )
                print(
                    f"  [TOOL DONE] {update.get('toolCallId')} - {update.get('status')}"
                )
            elif update_type == "plan":
                entries = update.get("entries", [])
                plans.append(entries)
                print(f"  [PLAN] {len(entries)} entries:")
                for e in entries:
                    status = e.get("status", "?")
                    content = e.get("content", "?")
                    print(f"    [{status}] {content}")

        elif "result" in resp and resp.get("id") == expected_id:
            return {
                "result": resp["result"],
                "text": "".join(text_chunks),
                "tool_calls": tool_calls,
                "tool_updates": tool_updates,
                "plans": plans,
            }
    return None


def main():
    # Setup test directory
    test_dir = "/tmp/crow_test"
    os.makedirs(test_dir, exist_ok=True)
    with open(f"{test_dir}/hello.py", "w") as f:
        f.write('print("Hello World")\n')

    binary = "./target/release/crow-agent"

    print(f"=== Full Workflow Test ===")
    print(f"Test directory: {test_dir}")
    print(f"Starting ACP server...\n")

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
            print("FAIL: Initialize failed")
            return 1
        print("Initialized OK\n")

        # Create session with test directory as cwd
        send_jsonrpc(proc, "session/new", {"cwd": test_dir, "mcpServers": []}, 2)
        resp = read_response(proc)
        if not resp or "result" not in resp:
            print("FAIL: Session creation failed")
            return 1
        session_id = resp["result"]["sessionId"]
        print(f"Session: {session_id}\n")

        # Prompt 1: Plan the task using TodoWrite
        print("=" * 60)
        print("PROMPT 1: Plan the task")
        print("=" * 60)
        prompt1 = """I need you to modify hello.py to change "Hello World" to "Hello Universe", then run it to verify.

First, use the todo_write tool to create a plan with these steps:
1. Read hello.py
2. Edit hello.py to change Hello World to Hello Universe
3. Run hello.py to verify the change

Just create the plan for now, don't execute yet."""

        send_jsonrpc(
            proc,
            "session/prompt",
            {"sessionId": session_id, "prompt": [{"type": "text", "text": prompt1}]},
            3,
        )

        result = wait_for_result(proc, 3)
        if not result:
            print("FAIL: No response to prompt 1")
            return 1

        print(
            f"\nResponse: {result['text'][:300]}..."
            if len(result["text"]) > 300
            else f"\nResponse: {result['text']}"
        )
        print(f"Stop reason: {result['result']['stopReason']}")

        if not result["plans"]:
            print("\nWARNING: No plan was created!")

        # Prompt 2: Execute the plan
        print("\n" + "=" * 60)
        print("PROMPT 2: Execute the plan")
        print("=" * 60)
        prompt2 = """Now execute the plan:
1. Read hello.py to see its current contents
2. Edit it to change "Hello World" to "Hello Universe"
3. Run it with the terminal tool to verify it works"""

        send_jsonrpc(
            proc,
            "session/prompt",
            {"sessionId": session_id, "prompt": [{"type": "text", "text": prompt2}]},
            4,
        )

        result = wait_for_result(proc, 4)
        if not result:
            print("FAIL: No response to prompt 2")
            return 1

        print(
            f"\nResponse: {result['text'][:500]}..."
            if len(result["text"]) > 500
            else f"\nResponse: {result['text']}"
        )
        print(f"Stop reason: {result['result']['stopReason']}")
        print(f"Tool calls made: {len(result['tool_calls'])}")
        for tc in result["tool_calls"]:
            print(f"  - {tc['title']}")

        # Verify the file was actually changed
        print("\n" + "=" * 60)
        print("VERIFICATION")
        print("=" * 60)
        with open(f"{test_dir}/hello.py", "r") as f:
            final_content = f.read()
        print(f"Final hello.py content: {repr(final_content)}")

        if "Hello Universe" in final_content:
            print("\nSUCCESS: File was correctly modified!")
            return 0
        else:
            print("\nFAIL: File was not modified correctly")
            return 1

    finally:
        proc.terminate()
        proc.wait()


if __name__ == "__main__":
    sys.exit(main())
