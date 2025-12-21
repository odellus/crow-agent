#!/usr/bin/env bash
# TEST: Edit Tool
# WHAT: Agent modifies existing file content
# AGENT CHECK: File changed on disk, diff shown in output

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CROW_AGENT="${CROW_AGENT:-$SCRIPT_DIR/../../../target/release/crow-agent}"
TEST_DIR=$(mktemp -d)
cd "$TEST_DIR"

echo "Test dir: $TEST_DIR"

# Setup
echo "Hello World" > edit_me.txt

echo ""
echo "=== Before Edit ==="
cat edit_me.txt

echo ""
echo "=== Test: Edit File ==="
"$CROW_AGENT" prompt "Edit edit_me.txt: replace World with Crow"

echo ""
echo "=== After Edit ==="
cat edit_me.txt

echo ""
echo "=== AGENT: Verify ==="
echo "1. File now contains 'Hello Crow' (not 'Hello World')"
echo "2. Agent read file first, then edited"
echo "3. Diff was shown in output"
echo "4. RED FLAG: Edit tool called multiple times"
echo "5. RED FLAG: File unchanged = tool execution bug"

rm -rf "$TEST_DIR"
