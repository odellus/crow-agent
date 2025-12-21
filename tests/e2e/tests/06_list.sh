#!/usr/bin/env bash
# TEST: List Directory Tool
# WHAT: Agent lists directory contents
# AGENT CHECK: Contents shown correctly

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CROW_AGENT="${CROW_AGENT:-$SCRIPT_DIR/../../../target/release/crow-agent}"
TEST_DIR=$(mktemp -d)
cd "$TEST_DIR"

echo "Test dir: $TEST_DIR"

# Setup
mkdir -p project/src project/tests
touch project/README.md project/Cargo.toml
touch project/src/main.rs project/src/lib.rs
touch project/tests/test_main.rs

echo ""
echo "=== Actual Structure ==="
find project -type f | sort

echo ""
echo "=== Test: List Project Directory ==="
"$CROW_AGENT" prompt "List the contents of the project directory"

echo ""
echo "=== AGENT: Verify ==="
echo "1. Shows README.md, Cargo.toml"
echo "2. Shows src/ and tests/ directories"
echo "3. Used list_directory tool"
echo "4. RED FLAG: Used bash ls instead of list_directory"

rm -rf "$TEST_DIR"
