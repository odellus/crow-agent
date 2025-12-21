#!/usr/bin/env bash
# TEST: Write File Tool
# WHAT: Agent creates new files using dedicated write_file tool
# AGENT CHECK: File created with correct content, write_file tool used (not bash)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CROW_AGENT="${CROW_AGENT:-$SCRIPT_DIR/../../../target/release/crow-agent}"
TEST_DIR=$(mktemp -d)
cd "$TEST_DIR"

echo "Test dir: $TEST_DIR"

echo ""
echo "=== Test 1: Create Simple File ==="
"$CROW_AGENT" prompt "Use the write_file tool to create a file called hello.txt with the content: Hello from Crow Agent"

echo ""
echo "=== Check File ==="
if [[ -f hello.txt ]]; then
    echo "File exists:"
    cat hello.txt
    if grep -q "Hello from Crow Agent" hello.txt; then
        echo "SUCCESS: Content matches"
    else
        echo "WARNING: Content differs from expected"
    fi
else
    echo "ERROR: hello.txt was not created"
fi

echo ""
echo "=== Test 2: Create JSON File ==="
"$CROW_AGENT" prompt 'Use the write_file tool to create a file called config.json. The content parameter must be a string, so write this JSON as a string: {"name": "test", "version": 1}'

echo ""
echo "=== Check JSON ==="
if [[ -f config.json ]]; then
    echo "File exists:"
    cat config.json
else
    echo "ERROR: config.json was not created"
fi

echo ""
echo "=== Test 3: Overwrite Existing File ==="
echo "Original content" > overwrite_me.txt
echo "Before overwrite:"
cat overwrite_me.txt
"$CROW_AGENT" prompt "First read overwrite_me.txt, then use write_file to overwrite it with: New content from write_file"

echo ""
echo "After overwrite:"
if [[ -f overwrite_me.txt ]]; then
    cat overwrite_me.txt
    if grep -q "New content" overwrite_me.txt; then
        echo "SUCCESS: File was overwritten"
    else
        echo "WARNING: Content may not have changed"
    fi
else
    echo "ERROR: overwrite_me.txt was deleted instead of overwritten"
fi

echo ""
echo "=== Test 4: Create File in Subdirectory ==="
mkdir -p subdir
"$CROW_AGENT" prompt "Use write_file to create a file at subdir/nested.txt with content: Nested file content"

echo ""
echo "=== Check Nested File ==="
if [[ -f subdir/nested.txt ]]; then
    echo "File exists:"
    cat subdir/nested.txt
else
    echo "ERROR: subdir/nested.txt was not created"
fi

echo ""
echo "=== AGENT: Verify ==="
echo "1. hello.txt exists with 'Hello from Crow Agent'"
echo "2. config.json exists with valid JSON"
echo "3. overwrite_me.txt has new content (read first, then write)"
echo "4. subdir/nested.txt exists"
echo "5. GREEN FLAG: write_file tool used (not bash echo/cat)"
echo "6. RED FLAG: File not created"
echo "7. RED FLAG: Used bash instead of write_file"
echo "8. RED FLAG: Overwrote without reading first"

rm -rf "$TEST_DIR"
