#!/usr/bin/env bash
# TEST: Find Path Tool
# WHAT: Agent finds files by pattern
# AGENT CHECK: Correct files found, reasonable tool call count

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CROW_AGENT="${CROW_AGENT:-$SCRIPT_DIR/../../../target/release/crow-agent}"
TEST_DIR=$(mktemp -d)
cd "$TEST_DIR"

echo "Test dir: $TEST_DIR"

# Setup
mkdir -p src/components src/utils
touch src/app.js src/index.ts src/main.rs
touch src/components/Button.tsx src/components/Modal.tsx
touch src/utils/helpers.ts src/utils/format.js

echo ""
echo "=== Directory Structure ==="
find . -type f | sort

echo ""
echo "=== Test 1: Find .tsx Files ==="
"$CROW_AGENT" prompt "Find all .tsx files"

echo ""
echo "=== Test 2: Find .js Files ==="
"$CROW_AGENT" prompt "Find all JavaScript files (.js)"

echo ""
echo "=== AGENT: Verify ==="
echo "1. Test 1 found Button.tsx and Modal.tsx (2 files)"
echo "2. Test 2 found app.js and format.js (2 files)"
echo "3. Used find_path tool"
echo "4. RED FLAG: Tool called many times for simple glob"

rm -rf "$TEST_DIR"
