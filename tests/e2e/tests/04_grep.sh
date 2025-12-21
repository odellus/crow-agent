#!/usr/bin/env bash
# TEST: Grep Tool
# WHAT: Agent searches file contents
# AGENT CHECK: Correct files found, used grep tool not bash grep

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CROW_AGENT="${CROW_AGENT:-$SCRIPT_DIR/../../../target/release/crow-agent}"
TEST_DIR=$(mktemp -d)
cd "$TEST_DIR"

echo "Test dir: $TEST_DIR"

# Setup
echo "ERROR: database connection failed" > log1.txt
echo "INFO: server started successfully" > log2.txt
echo "ERROR: timeout waiting for response" > log3.txt
echo "DEBUG: processing request" > log4.txt

echo ""
echo "=== Files Created ==="
ls -la *.txt

echo ""
echo "=== Test: Find ERROR Files ==="
"$CROW_AGENT" prompt "Use grep to find all files containing ERROR"

echo ""
echo "=== AGENT: Verify ==="
echo "1. Agent found log1.txt and log3.txt"
echo "2. Did NOT report log2.txt or log4.txt"
echo "3. Used grep tool (not bash grep/rg)"
echo "4. RED FLAG: Called grep multiple times"
echo "5. RED FLAG: Used bash grep instead of grep tool"

rm -rf "$TEST_DIR"
