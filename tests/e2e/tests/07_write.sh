#!/usr/bin/env bash
# TEST: Write Tool (currently via bash)
# WHAT: Agent creates new files
# AGENT CHECK: File created with correct content
# NOTE: crow-agent currently lacks dedicated write tool, uses bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CROW_AGENT="${CROW_AGENT:-$SCRIPT_DIR/../../../target/release/crow-agent}"
TEST_DIR=$(mktemp -d)
cd "$TEST_DIR"

echo "Test dir: $TEST_DIR"

echo ""
echo "=== Test 1: Create Simple File ==="
"$CROW_AGENT" prompt "Create a file called hello.txt with the content: Hello from Crow Agent"

echo ""
echo "=== Check File ==="
if [[ -f hello.txt ]]; then
    echo "File exists:"
    cat hello.txt
else
    echo "ERROR: hello.txt was not created"
fi

echo ""
echo "=== Test 2: Create JSON File ==="
"$CROW_AGENT" prompt 'Create a file called config.json with: {"name": "test", "version": 1}'

echo ""
echo "=== Check JSON ==="
if [[ -f config.json ]]; then
    echo "File exists:"
    cat config.json
else
    echo "ERROR: config.json was not created"
fi

echo ""
echo "=== AGENT: Verify ==="
echo "1. hello.txt exists with 'Hello from Crow Agent'"
echo "2. config.json exists with valid JSON"
echo "3. NOTE: Currently uses bash (echo/cat) - needs write tool"
echo "4. RED FLAG: File not created"
echo "5. RED FLAG: Content is wrong"

rm -rf "$TEST_DIR"
