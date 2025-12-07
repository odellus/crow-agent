# Crow Agent Tool Testing Plan

## Overview

This document outlines a comprehensive testing strategy for all crow-agent tools.
Tests are organized into three tiers:
1. **Unit Tests** - Direct function calls with assertions
2. **Integration Tests** - Tool execution through rig framework
3. **E2E Tests** - Agent-driven tests with acceptance criteria validation

## Tool Inventory & Issues Found

| Tool | Severity | Key Issues |
|------|----------|------------|
| `now` | Low | Fake timezone support - claims to support any TZ but only does UTC/Local |
| `thinking` | Medium | No input size limits, unclear persistence |
| `list_directory` | Medium | No max depth limit, potential stack overflow with deep recursion |
| `read_file` | High | No file size limit (OOM risk), reads binary files as text |
| `edit_file` | Critical | Arbitrary file creation, no atomic writes, ambiguous empty old_string |
| `find_path` | Medium | ReDoS vulnerability in glob-to-regex, case-insensitive only |
| `grep` | Medium | ReDoS vulnerability, incomplete binary file filtering |
| `terminal` | Critical | Arbitrary command execution, no command validation |

---

## 1. NOW TOOL

### Unit Tests
```rust
#[test] fn test_now_returns_current_time()
#[test] fn test_now_utc_timezone()
#[test] fn test_now_local_timezone()
#[test] fn test_now_invalid_timezone_handling() // Should fail gracefully
#[test] fn test_now_output_format_parseable() // ISO8601 format
```

### Issues to Test
- [ ] **FAKE CAPABILITY**: Requesting "America/New_York" returns local time with misleading label
- [ ] **NO VALIDATION**: Any string accepted as timezone without error
- [ ] **OUTPUT FORMAT**: Verify output can be parsed as valid datetime

### E2E Acceptance Criteria
```bash
# test_now_e2e.sh
echo "=== NOW TOOL E2E TEST ==="
echo ""
echo "ACCEPTANCE CRITERIA:"
echo "1. Tool returns current time within 5 seconds of actual time"
echo "2. UTC timezone returns actual UTC time (not local)"
echo "3. Invalid timezone returns clear error OR correctly states fallback"
echo "4. Output is parseable ISO8601 format"
echo ""
echo "EDGE CASES TO VERIFY:"
echo "- timezone='America/New_York' should NOT return local time silently"
echo "- timezone='INVALID_TZ_12345' should error or clearly state unsupported"
echo "- Empty timezone should default to local"
```

---

## 2. THINKING TOOL

### Unit Tests
```rust
#[test] fn test_thinking_accepts_string()
#[test] fn test_thinking_returns_char_count()
#[test] fn test_thinking_empty_string()
#[test] fn test_thinking_unicode_char_count() // emoji, CJK, etc.
#[test] fn test_thinking_very_large_input() // 1MB+
```

### Issues to Test
- [ ] **NO SIZE LIMIT**: Can submit gigabytes of text
- [ ] **TELEMETRY OVERFLOW**: Large thoughts could overflow logs
- [ ] **UNICODE COUNTING**: Grapheme vs char vs byte count

### E2E Acceptance Criteria
```bash
# test_thinking_e2e.sh
echo "=== THINKING TOOL E2E TEST ==="
echo ""
echo "ACCEPTANCE CRITERIA:"
echo "1. Tool accepts any string and returns acknowledgment"
echo "2. Character count in response matches actual input length"
echo "3. Tool does not hang or crash on very large input"
echo "4. Unicode characters counted correctly (not bytes)"
echo ""
echo "EDGE CASES TO VERIFY:"
echo "- 1MB of repeated text: should complete without OOM"
echo "- Emoji string '👨‍👩‍👧‍👦' has 7 code points but 1 grapheme - which is reported?"
echo "- Null bytes in input: should be handled safely"
```

---

## 3. LIST_DIRECTORY TOOL

### Unit Tests
```rust
#[test] fn test_list_simple_directory()
#[test] fn test_list_empty_directory()
#[test] fn test_list_hidden_files_excluded_by_default()
#[test] fn test_list_hidden_files_when_requested()
#[test] fn test_list_recursive_depth_0()
#[test] fn test_list_recursive_depth_1()
#[test] fn test_list_nonexistent_directory()
#[test] fn test_list_file_instead_of_directory()
#[test] fn test_list_outside_working_dir() // Security test
#[test] fn test_list_symlink_escape() // Security test
#[test] fn test_list_permission_denied()
```

### Issues to Test
- [ ] **STACK OVERFLOW**: depth=u32::MAX on deep directory
- [ ] **SYMLINK ESCAPE**: Symlink pointing outside working_dir
- [ ] **TOCTOU**: Directory deleted between canonicalize and is_dir check
- [ ] **NO MAX DEPTH**: depth=1000000 could cause problems

### E2E Acceptance Criteria
```bash
# test_list_directory_e2e.sh
echo "=== LIST_DIRECTORY TOOL E2E TEST ==="
echo ""
echo "ACCEPTANCE CRITERIA:"
echo "1. Lists files and directories in specified path"
echo "2. Hidden files excluded by default, included when show_hidden=true"
echo "3. Recursive listing respects depth parameter"
echo "4. SECURITY: Cannot list directories outside working_dir"
echo "5. SECURITY: Symlinks pointing outside working_dir are blocked"
echo ""
echo "EDGE CASES TO VERIFY:"
echo "- Path '../../../etc' should be blocked"
echo "- Symlink to /etc/passwd inside working_dir should be blocked or show symlink target clearly"
echo "- Empty directory shows '(empty directory)' message"
echo "- depth=100 on flat directory completes quickly"
echo "- Path with special chars: spaces, unicode, etc."
```

---

## 4. READ_FILE TOOL

### Unit Tests
```rust
#[test] fn test_read_entire_file()
#[test] fn test_read_line_range()
#[test] fn test_read_single_line()
#[test] fn test_read_beyond_file_end()
#[test] fn test_read_start_greater_than_end() // Silent correction?
#[test] fn test_read_nonexistent_file()
#[test] fn test_read_directory()
#[test] fn test_read_outside_working_dir() // Security test
#[test] fn test_read_symlink_outside_working_dir() // Security test
#[test] fn test_read_binary_file() // Invalid UTF-8
#[test] fn test_read_empty_file()
#[test] fn test_read_file_no_newline_at_end()
#[test] fn test_read_crlf_line_endings()
```

### Issues to Test
- [ ] **MEMORY EXHAUSTION**: No file size limit - 10GB file could OOM
- [ ] **BINARY FILES**: Fails on invalid UTF-8 with unclear error
- [ ] **LINE NUMBER CORRECTION**: start>end silently corrected
- [ ] **SENSITIVE FILES**: Can read .env, .git/config, private keys

### E2E Acceptance Criteria
```bash
# test_read_file_e2e.sh
echo "=== READ_FILE TOOL E2E TEST ==="
echo ""
echo "ACCEPTANCE CRITERIA:"
echo "1. Returns file contents accurately"
echo "2. Line range selection works correctly (1-indexed, inclusive)"
echo "3. SECURITY: Cannot read files outside working_dir"
echo "4. SECURITY: Symlinks pointing outside are blocked"
echo "5. Binary files return clear error (not garbage UTF-8)"
echo ""
echo "EDGE CASES TO VERIFY:"
echo "- Large file (100MB+): should either work or fail gracefully with size error"
echo "- Binary file: should return 'Binary file detected' or similar"
echo "- start_line=100, end_line=50: should error or return lines 50-50 (clarify behavior)"
echo "- Unicode filenames: should work"
echo "- File with mixed CRLF/LF endings: line count should be consistent"
echo ""
echo "SECURITY EDGE CASES:"
echo "- Path='../.env' should be blocked"
echo "- Reading .git/config should work (it's in working_dir) but maybe warn?"
echo "- Path='/etc/passwd' (absolute) should be blocked"
```

---

## 5. EDIT_FILE TOOL (CRITICAL)

### Unit Tests
```rust
#[test] fn test_edit_replace_string()
#[test] fn test_edit_replace_all()
#[test] fn test_edit_string_not_found()
#[test] fn test_edit_create_new_file()
#[test] fn test_edit_append_to_file()
#[test] fn test_edit_outside_working_dir() // Security test
#[test] fn test_edit_create_nested_directory()
#[test] fn test_edit_binary_file()
#[test] fn test_edit_unicode_content()
#[test] fn test_edit_empty_old_string_existing_file() // Appends!
#[test] fn test_edit_empty_old_string_new_file() // Creates
#[test] fn test_edit_identical_old_new() // Should this be allowed?
#[test] fn test_edit_file_becomes_empty()
#[test] fn test_edit_permission_denied()
```

### Issues to Test
- [ ] **ARBITRARY FILE CREATION**: Can create files anywhere in working_dir
- [ ] **DIRECTORY CREATION**: create_dir_all could create deep structures
- [ ] **AMBIGUOUS APPEND**: empty old_string appends to existing (unexpected)
- [ ] **NO ATOMIC WRITE**: Partial write on crash corrupts file
- [ ] **NO BACKUP**: No way to rollback changes

### E2E Acceptance Criteria
```bash
# test_edit_file_e2e.sh
echo "=== EDIT_FILE TOOL E2E TEST ==="
echo ""
echo "ACCEPTANCE CRITERIA:"
echo "1. Replace string works correctly (single and replace_all)"
echo "2. File creation with empty old_string works"
echo "3. SECURITY: Cannot create/edit files outside working_dir"
echo "4. Clear error when old_string not found"
echo "5. Replace count is accurate in response"
echo ""
echo "EDGE CASES TO VERIFY:"
echo "- old_string='' on EXISTING file: Should this append or error? DOCUMENT BEHAVIOR"
echo "- Creating file with path 'a/b/c/d/e/f.txt' creates all parent dirs"
echo "- Replace '.', new_string='X', replace_all=true on large file: could corrupt everything"
echo "- Line ending differences: old_string with LF vs file with CRLF"
echo "- Unicode in old_string/new_string: grapheme handling"
echo ""
echo "DESTRUCTIVE EDGE CASES (TEST ON COPY):"
echo "- edit_file(path='important.rs', old_string='fn ', new_string='FN ', replace_all=true)"
echo "- edit_file that makes file empty"
echo "- edit_file during concurrent reads"
```

---

## 6. FIND_PATH TOOL

### Unit Tests
```rust
#[test] fn test_find_simple_glob()
#[test] fn test_find_star_wildcard()
#[test] fn test_find_question_wildcard()
#[test] fn test_find_nested_pattern()
#[test] fn test_find_no_matches()
#[test] fn test_find_file_type_filter()
#[test] fn test_find_max_depth()
#[test] fn test_find_hidden_files_excluded()
#[test] fn test_find_hidden_files_included()
#[test] fn test_find_max_results_truncation()
#[test] fn test_find_outside_working_dir() // Security test
#[test] fn test_find_case_insensitive()
#[test] fn test_find_redos_pattern() // Security test
```

### Issues to Test
- [ ] **ReDoS**: pattern="a*a*a*a*a*b" could hang on certain filenames
- [ ] **CASE INSENSITIVE ONLY**: No way to do case-sensitive search
- [ ] **INCOMPLETE GLOB**: [abc] not supported as character class
- [ ] **SILENT TRUNCATION**: Returns max 200 results without total count

### E2E Acceptance Criteria
```bash
# test_find_path_e2e.sh
echo "=== FIND_PATH TOOL E2E TEST ==="
echo ""
echo "ACCEPTANCE CRITERIA:"
echo "1. Glob patterns * and ? work correctly"
echo "2. File type filtering (file/directory/both) works"
echo "3. max_depth limits search depth"
echo "4. Hidden files excluded by default"
echo "5. SECURITY: Cannot search outside working_dir"
echo ""
echo "EDGE CASES TO VERIFY:"
echo "- Pattern with ReDoS potential: 'a*a*a*a*b' should complete in <1 second"
echo "- Pattern '[abc]*.txt': is this treated as glob or literal?"
echo "- More than 200 matches: response should clearly indicate truncation AND total"
echo "- Case sensitivity: 'Readme.md' vs 'README.md' - should match state this"
echo "- Empty pattern: should error or match everything?"
```

---

## 7. GREP TOOL

### Unit Tests
```rust
#[test] fn test_grep_simple_pattern()
#[test] fn test_grep_regex_pattern()
#[test] fn test_grep_case_insensitive()
#[test] fn test_grep_with_context()
#[test] fn test_grep_line_numbers()
#[test] fn test_grep_file_extension_filter()
#[test] fn test_grep_no_matches()
#[test] fn test_grep_binary_file_skipped()
#[test] fn test_grep_hidden_files_skipped()
#[test] fn test_grep_max_results()
#[test] fn test_grep_output_truncation()
#[test] fn test_grep_redos_pattern() // Security test
#[test] fn test_grep_invalid_regex()
#[test] fn test_grep_outside_working_dir() // Security test
```

### Issues to Test
- [ ] **ReDoS**: pattern="(a+)+b" could hang on certain files
- [ ] **INCOMPLETE BINARY DETECTION**: blocklist misses many formats
- [ ] **SILENT FILE SKIPS**: Files with read errors silently skipped
- [ ] **OUTPUT TRUNCATION**: With context, output could be truncated inconsistently

### E2E Acceptance Criteria
```bash
# test_grep_e2e.sh
echo "=== GREP TOOL E2E TEST ==="
echo ""
echo "ACCEPTANCE CRITERIA:"
echo "1. Regex patterns match correctly"
echo "2. Case insensitivity works"
echo "3. Context lines (before/after) included"
echo "4. Line numbers shown by default"
echo "5. Binary files skipped (not searched)"
echo ""
echo "EDGE CASES TO VERIFY:"
echo "- ReDoS pattern '(a+)+b': should complete in <1 second"
echo "- File with invalid UTF-8: should be skipped with optional warning"
echo "- .wasm, .7z, other unlisted binary extensions: should these be skipped?"
echo "- 100 matches each with 10 context lines: is output coherent or truncated mid-match?"
echo "- Searching single file vs directory"
echo "- Pattern with special regex chars: '\\d+' vs 'd+'"
```

---

## 8. TERMINAL TOOL (CRITICAL)

### Unit Tests
```rust
#[test] fn test_terminal_simple_command()
#[test] fn test_terminal_with_output()
#[test] fn test_terminal_stderr()
#[test] fn test_terminal_exit_code_zero()
#[test] fn test_terminal_exit_code_nonzero()
#[test] fn test_terminal_timeout()
#[test] fn test_terminal_cd_to_directory()
#[test] fn test_terminal_cd_outside_working_dir() // Security test
#[test] fn test_terminal_output_truncation()
#[test] fn test_terminal_no_output()
#[test] fn test_terminal_command_injection() // Security test
#[test] fn test_terminal_binary_output()
```

### Issues to Test
- [ ] **ARBITRARY EXECUTION**: Any command can be run
- [ ] **NO COMMAND VALIDATION**: rm -rf allowed
- [ ] **ENVIRONMENT EXPOSURE**: env command shows all env vars
- [ ] **TIMEOUT KILL**: Process might not die immediately
- [ ] **NONZERO EXIT**: grep with no matches returns error (correct?)

### E2E Acceptance Criteria
```bash
# test_terminal_e2e.sh
echo "=== TERMINAL TOOL E2E TEST ==="
echo ""
echo "ACCEPTANCE CRITERIA:"
echo "1. Commands execute and return stdout"
echo "2. stderr is captured and returned"
echo "3. Non-zero exit codes are reported as errors"
echo "4. Timeout kills long-running processes"
echo "5. cd parameter changes working directory"
echo ""
echo "SECURITY CONSIDERATIONS:"
echo "- This tool is INHERENTLY DANGEROUS"
echo "- Consider: Should there be a command allowlist?"
echo "- Consider: Should env vars be filtered?"
echo "- Consider: Should output from certain commands be redacted?"
echo ""
echo "EDGE CASES TO VERIFY:"
echo "- Timeout: sleep 1000 with timeout=1 should return timeout error"
echo "- Large output: command that generates 10MB output should truncate cleanly"
echo "- Interactive commands: 'read input' should timeout, not hang forever"
echo "- cd='../../../etc': should be blocked or clearly escape working_dir"
echo "- command='env': shows all environment - is this acceptable?"
echo "- grep with no matches: exit code 1 - is this an 'error'?"
```

---

## Test Directory Structure

```
tests/
├── unit/
│   ├── now_test.rs
│   ├── thinking_test.rs
│   ├── list_directory_test.rs
│   ├── read_file_test.rs
│   ├── edit_file_test.rs
│   ├── find_path_test.rs
│   ├── grep_test.rs
│   └── terminal_test.rs
├── integration/
│   ├── tool_framework_test.rs    # Tools work through rig
│   └── telemetry_capture_test.rs # Tools captured in telemetry
├── e2e/
│   ├── fixtures/                  # Test files and directories
│   │   ├── sample_project/
│   │   ├── binary_files/
│   │   ├── unicode_files/
│   │   └── edge_case_files/
│   ├── scripts/
│   │   ├── test_now_e2e.sh
│   │   ├── test_thinking_e2e.sh
│   │   ├── test_list_directory_e2e.sh
│   │   ├── test_read_file_e2e.sh
│   │   ├── test_edit_file_e2e.sh
│   │   ├── test_find_path_e2e.sh
│   │   ├── test_grep_e2e.sh
│   │   └── test_terminal_e2e.sh
│   └── agent_tests/
│       ├── now_agent_test.rs      # LLM validates tool behavior
│       ├── thinking_agent_test.rs
│       └── ...
└── security/
    ├── path_traversal_test.rs
    ├── symlink_escape_test.rs
    ├── redos_test.rs
    ├── resource_exhaustion_test.rs
    └── command_injection_test.rs
```

---

## Priority Order for Implementation

### Phase 1: Critical Security (Must Fix First)
1. `terminal` - Add command validation/blocklist
2. `read_file` - Add file size limits
3. `edit_file` - Clarify empty old_string behavior, add atomic writes
4. `find_path` & `grep` - Fix ReDoS vulnerabilities

### Phase 2: Correctness Issues
5. `now` - Either implement timezone support or remove the parameter
6. `thinking` - Add size limits
7. `list_directory` - Add max depth enforcement
8. `read_file` - Better binary file detection

### Phase 3: E2E Test Coverage
9. Write E2E scripts for each tool
10. Create agent-driven tests with acceptance criteria
11. Run tests against real LLM to find unexpected behaviors

---

## Next Steps

1. Create `tests/` directory structure
2. Implement unit tests for each tool
3. Create test fixtures (sample files, directories)
4. Write E2E bash scripts with acceptance criteria
5. Implement agent-driven validation tests
6. Fix bugs found during testing
7. Document final behavior for each tool
