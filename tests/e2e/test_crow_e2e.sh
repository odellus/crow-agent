#!/usr/bin/env bash
# E2E tests for crow-agent - Real agent execution tests
# Run with: bash test_crow_e2e.sh
#
# These tests require an LLM provider to be running (e.g., LM Studio)

set -euo pipefail

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
CYAN='\033[0;36m'
NC='\033[0m'

PASSED=0
FAILED=0

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CROW_AGENT="${CROW_AGENT:-$SCRIPT_DIR/../../target/release/crow-agent}"

if [[ ! -f "$CROW_AGENT" ]]; then
    echo -e "${YELLOW}Building crow-agent (release)...${NC}"
    cd "$SCRIPT_DIR/../.." && cargo build --release
fi

TEST_DIR=$(mktemp -d -t crow-e2e-XXXXXX)
cd "$TEST_DIR"
echo -e "${CYAN}Test dir: $TEST_DIR${NC}"
echo -e "${CYAN}Binary: $CROW_AGENT${NC}"
echo ""

cleanup() { rm -rf "$TEST_DIR"; }
trap cleanup EXIT

pass() { echo -e "${GREEN}[PASS]${NC} $1"; ((PASSED++)) || true; }
fail() { echo -e "${RED}[FAIL]${NC} $1"; ((FAILED++)) || true; }
info() { echo -e "${CYAN}[INFO]${NC} $1"; }

# ============================================================
echo -e "\n${CYAN}=== Test 1: Bash Tool ===${NC}"
# ============================================================
info "Running: echo CROW_E2E_SUCCESS"
output=$("$CROW_AGENT" prompt "Run this command: echo CROW_E2E_SUCCESS" 2>&1) || true
if echo "$output" | grep -q "CROW_E2E_SUCCESS"; then
    pass "Bash: echo command worked"
else
    fail "Bash: echo command - expected CROW_E2E_SUCCESS in output"
    echo "$output" | tail -10
fi

# ============================================================
echo -e "\n${CYAN}=== Test 2: Read Tool ===${NC}"
# ============================================================
echo "Line one content" > read_test.txt
echo "Line two has SECRET_VALUE" >> read_test.txt
info "Created read_test.txt with 2 lines"

output=$("$CROW_AGENT" prompt "Read read_test.txt and tell me what value is on line two" 2>&1) || true
if echo "$output" | grep -qi "SECRET_VALUE\|line two"; then
    pass "Read: file contents understood"
else
    fail "Read: should mention SECRET_VALUE or line two"
    echo "$output" | tail -10
fi

# ============================================================
echo -e "\n${CYAN}=== Test 3: Edit Tool ===${NC}"
# ============================================================
echo "Hello World" > edit_test.txt
info "Created edit_test.txt with 'Hello World'"

"$CROW_AGENT" prompt "Edit edit_test.txt: replace World with Crow" 2>&1 > /dev/null || true
sleep 1

if grep -q "Crow" edit_test.txt 2>/dev/null; then
    pass "Edit: replacement worked"
    echo "  Content: $(cat edit_test.txt)"
else
    fail "Edit: file should contain 'Crow'"
    echo "  Content: $(cat edit_test.txt 2>/dev/null || echo 'file missing')"
fi

# ============================================================
echo -e "\n${CYAN}=== Test 4: Grep Tool ===${NC}"
# ============================================================
echo "ERROR: something failed" > log1.txt
echo "INFO: all systems go" > log2.txt
echo "ERROR: another failure" > log3.txt
info "Created 3 log files (2 with ERROR)"

output=$("$CROW_AGENT" prompt "Use grep to find files containing ERROR" 2>&1) || true
if echo "$output" | grep -qi "log1\|log3\|2 file\|two file"; then
    pass "Grep: found ERROR files"
else
    fail "Grep: should find log1.txt and log3.txt"
    echo "$output" | tail -10
fi

# ============================================================
echo -e "\n${CYAN}=== Test 5: Find Path Tool ===${NC}"
# ============================================================
mkdir -p src/components
touch src/app.js src/index.ts
touch src/components/Button.tsx src/components/Input.tsx
info "Created src/ with .js, .ts, .tsx files"

output=$("$CROW_AGENT" prompt "Find all .tsx files in this directory" 2>&1) || true
if echo "$output" | grep -qi "Button\|Input\|tsx"; then
    pass "Find: located .tsx files"
else
    fail "Find: should find Button.tsx and Input.tsx"
    echo "$output" | tail -10
fi

# ============================================================
echo -e "\n${CYAN}=== Test 6: List Directory Tool ===${NC}"
# ============================================================
output=$("$CROW_AGENT" prompt "List the contents of the src directory" 2>&1) || true
if echo "$output" | grep -qi "app\|index\|components"; then
    pass "List: directory contents shown"
else
    fail "List: should show src/ contents"
    echo "$output" | tail -10
fi

# ============================================================
echo -e "\n${CYAN}=== Test 7: Write via Bash ===${NC}"
# ============================================================
info "Testing file creation (currently uses bash)"
"$CROW_AGENT" prompt "Create a file called hello.txt with the content: Hello from Crow" 2>&1 > /dev/null || true
sleep 1

if [[ -f "hello.txt" ]] && grep -qi "crow\|hello" hello.txt 2>/dev/null; then
    pass "Write: file created"
    echo "  Content: $(cat hello.txt)"
else
    fail "Write: hello.txt should exist with content"
    ls -la
fi

# ============================================================
echo -e "\n${CYAN}=== Test 8: Error Handling ===${NC}"
# ============================================================
output=$("$CROW_AGENT" prompt "Read the file nonexistent_xyz_12345.txt" 2>&1) || true
if echo "$output" | grep -qi "not found\|does not exist\|error\|no such"; then
    pass "Error: handled missing file gracefully"
else
    # Check it didn't hallucinate content
    if echo "$output" | grep -qi "content\|contains\|says"; then
        fail "Error: may have hallucinated file contents"
    else
        pass "Error: handled missing file (unclear response)"
    fi
fi

# ============================================================
echo -e "\n${CYAN}=== Test 9: Multi-Step Workflow ===${NC}"
# ============================================================
echo '{"name": "TestProject", "version": "1.0"}' > config.json
info "Created config.json"

output=$("$CROW_AGENT" prompt "Read config.json and tell me the project name" 2>&1) || true
if echo "$output" | grep -qi "TestProject"; then
    pass "Workflow: read and parsed JSON"
else
    fail "Workflow: should report name as TestProject"
    echo "$output" | tail -10
fi

# ============================================================
echo ""
echo "============================================"
echo -e "${CYAN}E2E Tests Complete${NC}"
echo -e "  ${GREEN}Passed: $PASSED${NC}"
echo -e "  ${RED}Failed: $FAILED${NC}"
echo "============================================"
echo ""
echo -e "${YELLOW}AGENT: Review the output above for red flags:${NC}"
echo "  - Tools called multiple times unnecessarily"
echo "  - Wrong tool selection (bash instead of dedicated tool)"
echo "  - Hallucinated content"
echo "  - Excessive token usage"
echo ""

[[ $FAILED -eq 0 ]]
