# E2E Testing Guide for Agents

This document is for AI agents running E2E tests on crow-agent. These are not scripted tests - they require intelligence to evaluate.

## Philosophy

Traditional CI/CD: Script runs, checks exit code, pass/fail.

Agent CI/CD: Agent runs commands, observes behavior, uses judgment to determine if things are working correctly. Catches issues like:
- Model calling tools repeatedly for no reason
- Tool succeeding but agent not reporting result
- Subtle behavioral regressions
- Context bloat
- Wrong tool selection

---

## Before You Start

### 1. Build crow-agent
```bash
cd crow_agent
cargo build --release
```

### 2. Verify provider is working
```bash
./target/release/crow-agent prompt "Say: HELLO"
```
You should see output with thinking, tool calls, response. If it fails, check API keys and LM Studio.

### 3. Create a fresh test directory
```bash
TEST_DIR=$(mktemp -d)
cd "$TEST_DIR"
```

---

## Test 1: Bash Tool

### Setup
```bash
TEST_DIR=$(mktemp -d) && cd "$TEST_DIR"
```

### Run
```bash
crow-agent prompt "Run this bash command: echo CROW_BASH_TEST"
```

### Verify
- Output contains `CROW_BASH_TEST`
- Tool was called exactly ONCE
- Exit code was 0

### What to watch for
- **RED FLAG**: Tool called multiple times for simple echo
- **RED FLAG**: Agent uses wrong approach (like trying to write a file instead of running bash)

---

## Test 2: Read Tool

### Setup
```bash
echo "This is line one" > read_test.txt
echo "This is line two" >> read_test.txt
```

### Run
```bash
crow-agent prompt "Read read_test.txt and tell me what's on line two"
```

### Verify
- Agent mentions "line two" or the content
- Agent used read_file tool (check output shows tool call)

### What to watch for
- Agent should NOT use bash `cat` - should use read_file tool
- Agent should understand and report the content, not just dump it

---

## Test 3: Edit Tool

### Setup
```bash
echo "Hello World" > edit_test.txt
```

### Run
```bash
crow-agent prompt "Edit edit_test.txt: replace World with Crow"
```

### Verify
```bash
cat edit_test.txt
# Should say "Hello Crow"
```

### What to watch for
- **CRITICAL**: File actually changed on disk
- Agent read the file first (should see `read_file` then `edit` in tool calls)
- Edit tool showed the diff in output
- **RED FLAG**: Agent calls edit multiple times
- **RED FLAG**: Agent says it edited but file unchanged

---

## Test 4: Grep Tool

### Setup
```bash
echo "ERROR: something failed" > log1.txt
echo "INFO: all systems go" > log2.txt
echo "ERROR: another failure" > log3.txt
```

### Run
```bash
crow-agent prompt "Use grep to find files containing ERROR"
```

### Verify
- Agent reports log1.txt and log3.txt (not log2.txt)
- Used the grep tool (not bash grep)

### What to watch for
- Should find exactly 2 files
- **RED FLAG**: Agent runs bash `grep` instead of grep tool
- **RED FLAG**: Calls grep tool multiple times

---

## Test 5: Find Path / Glob Tool

### Setup
```bash
mkdir -p src/components
touch src/app.js src/index.ts
touch src/components/Button.tsx src/components/Input.tsx
```

### Run
```bash
crow-agent prompt "Find all .tsx files"
```

### Verify
- Agent finds Button.tsx and Input.tsx
- Used find_path or list_directory tool with pattern

### What to watch for
- Should find exactly 2 .tsx files
- **RED FLAG**: Tool called many times

---

## Test 6: List Directory Tool

### Setup
Use same directory structure from previous test.

### Run
```bash
crow-agent prompt "List the contents of the src directory"
```

### Verify
- Agent lists app.js, index.ts, components/
- Understands directory structure

---

## Test 7: Write Tool (MISSING - uses bash echo)

Currently crow-agent lacks a dedicated write tool. Agent must use bash:
```bash
crow-agent prompt "Create a file called hello.txt with content: Hello from Crow"
```

### Verify
```bash
cat hello.txt
# Should contain "Hello from Crow"
```

### What to watch for
- Agent uses `echo "..." > file` or similar bash approach
- File actually created with correct content
- **NOTE**: This is a gap - should have a write tool

---

## Test 8: Multi-Tool Workflow

### Run
```bash
echo '{"name": "test"}' > config.json
crow-agent prompt "Read config.json and tell me what the name field is"
```

### Verify
- Agent correctly reports name is "test"
- Used read_file tool

---

## Test 9: Error Handling

### Run
```bash
crow-agent prompt "Read the file nonexistent_file_12345.txt"
```

### Verify
- Agent gracefully handles error
- Reports file doesn't exist
- Does NOT hallucinate file contents

### What to watch for
- Agent should acknowledge the error, not make up content
- Should not retry excessively

---

## Test 10: Task Tool (Subagent)

### Run
```bash
crow-agent prompt "Use the task tool to research: What is the capital of France? Have a subagent verify this."
```

### Verify
- Task tool was invoked
- Subagent executed
- Result returned correctly

### What to watch for
- **RED FLAG**: Agent answered from memory instead of spawning Task
- **RED FLAG**: Subagent did nothing

---

## Evaluating Results

After running tests, evaluate:

### Per-Test Checklist
- [ ] Tool produced correct result
- [ ] Tool called appropriate number of times (usually 1-2)
- [ ] Agent reported result correctly
- [ ] No hallucinated information
- [ ] Reasonable token usage

### Red Flags That Need Investigation
1. Tool called 5+ times for simple task → Model confusion, check prompts
2. Agent says success but filesystem unchanged → Tool execution bug
3. Agent uses bash when dedicated tool exists → Prompt needs adjustment
4. Consistent timeouts → Process management issue

---

## Reporting Results

When reporting test results, include:
1. Which tests passed/failed
2. Any red flags observed
3. Any behavioral issues (even if test "passed")

Example:
```
E2E Test Results:
- Bash: PASS
- Read: PASS
- Edit: PASS (but called tool 3x, investigate)
- Grep: PASS
- Find: FAIL - tool called 10 times, model confused
- List: PASS
- Write: PASS (used bash - needs write tool)
- Workflow: PASS
- Error handling: PASS
- Task: PASS

Issues:
- Find test shows model calling tool repeatedly
- Edit test has unnecessary extra read at end
- No dedicated write tool
```

---

## Quick Smoke Test

If you just need to verify crow-agent is working:

```bash
TEST_DIR=$(mktemp -d) && cd "$TEST_DIR"
CROW="path/to/crow-agent"

# Test 1: Bash
$CROW prompt "Run: echo SMOKE_TEST"

# Test 2: Read + Edit
echo "foo bar" > test.txt
$CROW prompt "Read test.txt, then edit it to change foo to baz"
cat test.txt  # Should say "baz bar"

# Test 3: Grep
echo "ERROR here" > a.txt
echo "OK here" > b.txt
$CROW prompt "Find files containing ERROR"

echo "Smoke test complete. Check outputs above."
```

If all three work and files are correct, crow-agent is healthy.
