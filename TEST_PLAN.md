# Crow Agent Testing Plan

## Overview

This document outlines the comprehensive testing strategy for crow-agent, covering:
1. Core functionality (streaming, tools, cancellation)
2. Agent configurations and modes
3. HITL levels and autonomous execution
4. Humanized fixture compression
5. ACP protocol testing
6. Telemetry verification

All tests verify results via telemetry database queries.

---

## 1. Streaming Responses

### Test 1.1: Basic Text Streaming
```bash
cd /tmp && crow-agent prompt "count from 1 to 5, one number per line"
crow-agent query "SELECT response_content FROM traces ORDER BY started_at DESC LIMIT 1"
```
**Expected:** Visible character-by-character streaming, complete response in traces.

### Test 1.2: Long Response Streaming
```bash
crow-agent prompt "explain the rust ownership model in detail"
crow-agent traces -n 1
```
**Expected:** Smooth streaming, latency_ms reflects full generation time.

### Test 1.3: Thinking/Reasoning Streaming (verbose mode)
```bash
crow-agent -v prompt "what is 17 * 23? think step by step"
```
**Expected:** Thinking content visible in gray, separate from main response.

---

## 2. Tool Calling

### Test 2.1: read_file Tool
```bash
cd /tmp && echo "hello world" > test.txt
crow-agent prompt "read test.txt"
crow-agent query "SELECT tool_name, success FROM tool_calls ORDER BY timestamp DESC LIMIT 1"
```
**Expected:** Tool call logged, success=1, output shows file content.

### Test 2.2: edit Tool
```bash
crow-agent prompt "change 'hello' to 'goodbye' in test.txt"
cat test.txt  # Should show "goodbye world"
crow-agent query "SELECT tool_name, arguments FROM tool_calls WHERE tool_name='edit' ORDER BY timestamp DESC LIMIT 1"
```
**Expected:** Edit applied, tool call logged with oldString/newString.

### Test 2.3: bash Tool
```bash
crow-agent prompt "run 'echo hello from bash'"
crow-agent query "SELECT tool_name, duration_ms, success FROM tool_calls WHERE tool_name='bash' ORDER BY timestamp DESC LIMIT 1"
```
**Expected:** Command executed, duration logged.

### Test 2.4: grep Tool
```bash
crow-agent prompt "search for 'TODO' in the current directory"
crow-agent tools  # Should show grep in tool stats
```
**Expected:** Grep results returned, tool stats updated.

### Test 2.5: list_directory Tool
```bash
crow-agent prompt "list files in /tmp"
```
**Expected:** Directory listing returned.

### Test 2.6: Multiple Tools in Sequence
```bash
crow-agent prompt "create a file called multi.txt with 'test', then read it back"
crow-agent query "SELECT tool_name FROM tool_calls WHERE session_id=(SELECT session_id FROM traces ORDER BY started_at DESC LIMIT 1) ORDER BY timestamp"
```
**Expected:** Multiple tool calls logged in order (write, read).

---

## 3. Interrupt/Cancellation

### Test 3.1: Ctrl+C During Streaming
```bash
crow-agent prompt "write a 1000 word essay about rust" &
sleep 2 && kill -INT $!
crow-agent query "SELECT error FROM traces ORDER BY started_at DESC LIMIT 1"
```
**Expected:** Graceful cancellation, partial response preserved if any.

### Test 3.2: Ctrl+C During Tool Execution
```bash
crow-agent prompt "run 'sleep 30'" &
sleep 2 && kill -INT $!
```
**Expected:** Tool cancelled, session remains usable.

---

## 4. Different Agents

### Test 4.1: List Available Agents
```bash
crow-agent --list-agents
```
**Expected:** Shows build, plan, general, executor, arbiter, planner, architect.

### Test 4.2: Build Agent (default)
```bash
crow-agent -a build prompt "what tools do you have?"
crow-agent query "SELECT agent_name FROM traces ORDER BY started_at DESC LIMIT 1"
```
**Expected:** agent_name='build' in trace.

### Test 4.3: Plan Agent (read-only)
```bash
crow-agent -a plan prompt "analyze the structure of /tmp"
crow-agent query "SELECT agent_name FROM traces ORDER BY started_at DESC LIMIT 1"
```
**Expected:** agent_name='plan', only read-only tools used.

### Test 4.4: General Agent (subagent - should fail as primary)
```bash
crow-agent -a general prompt "hello" 2>&1
```
**Expected:** Error - general is subagent-only mode.

### Test 4.5: Custom Agent from Markdown
```bash
mkdir -p /tmp/test-project/.crow/agent
cat > /tmp/test-project/.crow/agent/tester.md << 'EOF'
---
name: tester
description: Testing agent
mode: primary
temperature: 0.1
tools:
  bash: false
  edit: false
---
You are a read-only testing agent. You can only read files and search.
EOF
cd /tmp/test-project && crow-agent --list-agents
crow-agent -a tester prompt "what can you do?"
```
**Expected:** tester agent loaded, tools restricted per config.

---

## 5. Humanized Tool Fixtures Compression

### Test 5.1: Verify Fixture Compression in Multi-Turn
```bash
crow-agent repl << 'EOF'
read all files in /tmp ending in .txt
now count how many you found
/quit
EOF
crow-agent trace $(crow-agent query "SELECT id FROM traces ORDER BY started_at DESC LIMIT 1" | tail -1) --json | jq '.request_messages[-2].content'
```
**Expected:** Previous tool results condensed (e.g., "read `file.txt` (N lines)") not full content.

### Test 5.2: Long Output Trimming
```bash
crow-agent prompt "run 'seq 1 1000'"
crow-agent trace $(crow-agent query "SELECT id FROM traces ORDER BY started_at DESC LIMIT 1" | tail -1) --full | grep -A5 "ran \`seq"
```
**Expected:** Output trimmed to first 3 + last 2 lines with "... (1000 lines) ..." indicator.

---

## 6. HITL Levels and Autonomous Modes

### Test 6.1: Level -1 (HITL) - Single Turn
```bash
# Default prompt command is essentially HITL - returns after one turn
crow-agent prompt "say hello"
```
**Expected:** Single turn, returns immediately.

### Test 6.2: Level 0 (Loop) - Multi-Turn Until Complete
```bash
crow-agent prompt "create a file test1.txt, then test2.txt, then test3.txt, then say done"
crow-agent query "SELECT COUNT(*) FROM tool_calls WHERE session_id=(SELECT session_id FROM traces ORDER BY started_at DESC LIMIT 1)"
```
**Expected:** Multiple tool calls in one session, continues until task_complete or text response.

### Test 6.3: Verify Control Flow in REPL
```bash
crow-agent repl << 'EOF'
create 3 files named a.txt b.txt c.txt with "test" content
/stats
/quit
EOF
```
**Expected:** Stats show multiple interactions.

---

## 7. Task Tool (Subagent Spawning)

### Test 7.1: Basic Subagent Spawn
```bash
crow-agent -a build prompt "use the task tool to spawn a 'general' agent to research what files exist in /tmp"
crow-agent query "SELECT tool_name, arguments FROM tool_calls WHERE tool_name='task' ORDER BY timestamp DESC LIMIT 1"
```
**Expected:** task tool called with subagent_type='general'.

### Test 7.2: Subagent Tool Restrictions
```bash
crow-agent -a build prompt "spawn an executor agent to read /etc/hostname"
# Verify executor can read but cannot call task_complete
```
**Expected:** Executor runs, returns result to primary.

### Test 7.3: Invalid Subagent Type
```bash
crow-agent -a build prompt "spawn a 'build' agent to do something"
```
**Expected:** Error - build is primary-only, not a subagent.

---

## 8. ACP Protocol Testing

### Test 8.1: Initialize
```bash
echo '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"clientInfo":{"name":"test","version":"1.0"},"capabilities":{},"protocolVersion":"0.1"}}' | crow-agent acp
```
**Expected:** Valid JSON response with agentCapabilities.

### Test 8.2: Create Session
```bash
(
echo '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"clientInfo":{"name":"test","version":"1.0"},"capabilities":{},"protocolVersion":"0.1"}}'
sleep 0.5
echo '{"jsonrpc":"2.0","id":2,"method":"session/new","params":{"cwd":"/tmp","mcpServers":[]}}'
) | crow-agent acp 2>/dev/null
```
**Expected:** Session created with sessionId.

### Test 8.3: Send Prompt
```bash
(
echo '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"clientInfo":{"name":"test","version":"1.0"},"capabilities":{},"protocolVersion":"0.1"}}'
sleep 0.5
echo '{"jsonrpc":"2.0","id":2,"method":"session/new","params":{"cwd":"/tmp","mcpServers":[]}}'
sleep 0.5
echo '{"jsonrpc":"2.0","id":3,"method":"session/prompt","params":{"sessionId":"0","prompt":[{"type":"text","text":"say hello"}]}}'
sleep 3
) | timeout 10 crow-agent acp 2>/dev/null
```
**Expected:** Streaming notifications followed by result.

### Test 8.4: Cancel Request
```bash
# Start long-running prompt, then send cancel
(
echo '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"clientInfo":{"name":"test","version":"1.0"},"capabilities":{},"protocolVersion":"0.1"}}'
sleep 0.5
echo '{"jsonrpc":"2.0","id":2,"method":"session/new","params":{"cwd":"/tmp","mcpServers":[]}}'
sleep 0.5
echo '{"jsonrpc":"2.0","id":3,"method":"session/prompt","params":{"sessionId":"0","prompt":[{"type":"text","text":"write a very long story"}]}}'
sleep 1
echo '{"jsonrpc":"2.0","method":"session/cancel","params":{"sessionId":"0"}}'
sleep 1
) | timeout 10 crow-agent acp 2>/dev/null
```
**Expected:** Request cancelled, appropriate error/stop response.

---

## 9. Telemetry Verification

### Test 9.1: Session Tracking
```bash
crow-agent stats
```
**Expected:** Shows recent sessions with interaction counts.

### Test 9.2: Tool Statistics
```bash
crow-agent tools
```
**Expected:** Shows tool usage with call counts, avg duration, success rate.

### Test 9.3: Trace Details
```bash
crow-agent traces -n 5
crow-agent trace <id> --full
```
**Expected:** Full request/response captured, tokens logged (if provider returns them).

### Test 9.4: SQL Queries
```bash
crow-agent query "SELECT model_id, COUNT(*) as calls, AVG(latency_ms) as avg_ms FROM traces GROUP BY model_id"
```
**Expected:** Aggregated stats by model.

---

## 10. Session Continuation

### Test 10.1: Continue Previous Session
```bash
# First session
crow-agent prompt "remember the number 42"
SESSION=$(crow-agent query "SELECT session_id FROM traces ORDER BY started_at DESC LIMIT 1" | tail -1)

# Continue with -s flag
crow-agent prompt -s ${SESSION:0:8} "what number did I ask you to remember?"
```
**Expected:** Agent recalls "42" from previous context.

---

## Automated Test Runner

Create a script to run all tests:

```bash
#!/bin/bash
# test_crow_agent.sh

CROW=/path/to/crow-agent
PASS=0
FAIL=0

run_test() {
    local name="$1"
    local cmd="$2"
    local expect="$3"
    
    echo -n "Testing $name... "
    result=$(eval "$cmd" 2>&1)
    if echo "$result" | grep -q "$expect"; then
        echo "PASS"
        ((PASS++))
    else
        echo "FAIL"
        echo "  Expected: $expect"
        echo "  Got: $result"
        ((FAIL++))
    fi
}

# Run tests...
run_test "initialize" "echo '{...}' | $CROW acp" "agentCapabilities"

echo "Results: $PASS passed, $FAIL failed"
```

---

## Notes

- All tests should verify via telemetry, not just stdout
- Use `crow-agent query` for precise verification
- Cleanup: `rm -f /tmp/test*.txt` between test runs
- For ACP tests, use `timeout` to prevent hangs
