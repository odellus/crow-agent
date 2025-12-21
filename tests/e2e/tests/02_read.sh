#!/usr/bin/env bash
# TEST: Read Tool
# WHAT: Agent reads file contents
# AGENT CHECK: Content reported correctly, used read_file not bash cat

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CROW_AGENT="${CROW_AGENT:-$SCRIPT_DIR/../../../target/release/crow-agent}"
TEST_DIR=$(mktemp -d)
cd "$TEST_DIR"

echo "Test dir: $TEST_DIR"

# Setup
cat > sample.txt << 'EOF'
First line of the file
Second line has SECRET_CODE_123
Third line is here
EOF

echo ""
echo "=== File Contents ==="
cat sample.txt

echo ""
echo "=== Test: Read and Report ==="
"$CROW_AGENT" prompt "Read sample.txt and tell me what code is on line 2"

echo ""
echo "=== AGENT: Verify ==="
echo "1. Agent mentions SECRET_CODE_123"
echo "2. Used read_file tool (not bash cat)"
echo "3. Understood the content, didn't just dump it"
echo "4. RED FLAG: Used bash cat instead of read_file"

rm -rf "$TEST_DIR"
