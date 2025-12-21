#!/usr/bin/env bash
# TEST: Multi-Tool Workflow
# WHAT: Agent chains multiple tools together
# AGENT CHECK: Correct tool sequence, synthesizes results

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CROW_AGENT="${CROW_AGENT:-$SCRIPT_DIR/../../../target/release/crow-agent}"
TEST_DIR=$(mktemp -d)
cd "$TEST_DIR"

echo "Test dir: $TEST_DIR"

# Setup
cat > data.json << 'EOF'
{
  "project": "crow-agent",
  "version": "0.1.0",
  "author": "Thomas"
}
EOF

echo ""
echo "=== Setup: data.json ==="
cat data.json

echo ""
echo "=== Test: Read, Understand, Report ==="
"$CROW_AGENT" prompt "Read data.json and tell me: 1) the project name, 2) the version, 3) who wrote it"

echo ""
echo "=== Test: Read and Modify ==="
echo "Count: 0" > counter.txt
"$CROW_AGENT" prompt "Read counter.txt, increment the count by 1, and save it back"

echo ""
echo "=== Check counter.txt ==="
cat counter.txt

echo ""
echo "=== AGENT: Verify ==="
echo "1. First test: reported crow-agent, 0.1.0, Thomas"
echo "2. Second test: counter.txt now says 'Count: 1'"
echo "3. Used appropriate tool sequence (read -> edit)"
echo "4. RED FLAG: Didn't understand JSON structure"
echo "5. RED FLAG: Counter not incremented"

rm -rf "$TEST_DIR"
