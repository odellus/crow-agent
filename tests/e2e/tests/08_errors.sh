#!/usr/bin/env bash
# TEST: Error Handling
# WHAT: Agent handles errors gracefully
# AGENT CHECK: No hallucination, graceful error messages

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CROW_AGENT="${CROW_AGENT:-$SCRIPT_DIR/../../../target/release/crow-agent}"
TEST_DIR=$(mktemp -d)
cd "$TEST_DIR"

echo "Test dir: $TEST_DIR"

echo ""
echo "=== Test 1: Read Nonexistent File ==="
"$CROW_AGENT" prompt "Read the file this_file_does_not_exist_xyz123.txt"

echo ""
echo "=== Test 2: Edit Nonexistent File ==="
"$CROW_AGENT" prompt "Edit nonexistent.txt: change foo to bar"

echo ""
echo "=== AGENT: Verify ==="
echo "1. Agent acknowledged files don't exist"
echo "2. Did NOT hallucinate file contents"
echo "3. Did NOT retry excessively"
echo "4. RED FLAG: Agent made up file contents"
echo "5. RED FLAG: Agent claimed success when file doesn't exist"

rm -rf "$TEST_DIR"
