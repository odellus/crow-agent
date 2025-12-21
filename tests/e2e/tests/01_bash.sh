#!/usr/bin/env bash
# TEST: Bash Tool
# WHAT: Agent executes shell commands
# AGENT CHECK: Output correct, single tool call

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CROW_AGENT="${CROW_AGENT:-$SCRIPT_DIR/../../../target/release/crow-agent}"
TEST_DIR=$(mktemp -d)
cd "$TEST_DIR"

echo "Test dir: $TEST_DIR"
echo ""

echo "=== Test 1: Simple Echo ==="
"$CROW_AGENT" prompt "Run this command: echo CROW_BASH_TEST"

echo ""
echo "=== Test 2: Command with Pipe ==="
"$CROW_AGENT" prompt "Run: echo hello | tr a-z A-Z"

echo ""
echo "=== Test 3: PWD ==="
"$CROW_AGENT" prompt "Run: pwd"

echo ""
echo "=== AGENT: Verify ==="
echo "1. First test output contains CROW_BASH_TEST"
echo "2. Second test output contains HELLO"
echo "3. Third test shows $TEST_DIR"
echo "4. Each command used bash tool exactly once"
echo "5. RED FLAG: Multiple tool calls for simple commands"

rm -rf "$TEST_DIR"
