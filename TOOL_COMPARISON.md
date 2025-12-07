# Tool Comparison: crow_agent vs agent_crate

This document compares our tool implementations against Zed's agent_crate reference implementations to identify gaps.

## Summary

| Tool | crow_agent LOC | agent_crate LOC | Parity Status | Notes |
|------|----------------|-----------------|---------------|-------|
| edit_file | ~250 | 1757 | **CRITICAL GAP** | Needs full edit agent |
| read_file | ~175 | 968 | **GAP** | Needs size limits, outline fallback |
| grep | ~320 | 1188 | **GAP** | Needs syntax awareness, pagination |
| list_directory | ~175 | 660 | Moderate | Needs privacy/exclusion settings |
| find_path | ~240 | N/A | N/A | agent_crate uses different approach |
| terminal | ~200 | N/A | OK | Both allow arbitrary execution |
| now | ~80 | N/A | OK | Simple utility |
| thinking | ~50 | N/A | OK | Simple utility |

---

## 1. EDIT_FILE - CRITICAL GAP

### agent_crate Features (1757 LOC)
- **Full Edit Agent**: Uses a sub-agent/model to interpret edit instructions
- **Three modes**: `edit`, `create`, `overwrite`
- **Diff generation**: Returns unified diff of changes
- **Format on save**: Respects user's format_on_save settings, runs LSP formatters
- **Authorization system**: Prompts user for confirmation on sensitive paths
- **Streaming edits**: Supports streaming output from edit agent
- **Hallucination detection**: Detects when model hallucinates old_text
- **Ambiguous range detection**: Handles when old_text matches multiple locations
- **Buffer integration**: Works with editor buffers, not just raw files
- **Action log**: Records all edits for undo/review

### crow_agent Implementation (250 LOC)
- **Simple search/replace**: Just `content.replace(old, new)`
- **No modes**: Empty old_string = create or append (confusing)
- **No diff output**: Just says "Edited file (N replacements)"
- **No formatting**: Raw write
- **No authorization**: Any file in working_dir
- **No streaming**: Blocking operation
- **No validation**: Silently fails if old_string not found after count check

### Required Changes

```
1. Implement EditAgent (separate sub-agent that interprets edit instructions)
2. Add modes: edit/create/overwrite with proper semantics
3. Return unified diff in output
4. Add file size/complexity limits
5. Better error messages with context
6. Atomic writes (write to temp, rename)
```

**Recommendation**: This is a complete rewrite. The agent_crate approach uses an AI model to interpret "Fix the bug on line 42" type instructions, which our simple search/replace cannot do.

---

## 2. READ_FILE - GAP

### agent_crate Features (968 LOC)
- **Size limits via outline**: Large files return syntax outline instead of full content
- **Image support**: Can read and return image files as LanguageModelImage
- **Privacy/exclusion checks**: Respects `file_scan_exclusions` and `private_files` settings
- **Buffer integration**: Opens files through project buffers
- **Action log**: Records file reads
- **Worktree awareness**: Knows about project structure
- **Line range validation**: Handles edge cases (start=0 treated as 1, etc.)

### crow_agent Implementation (175 LOC)
- **No size limits**: Will try to read any size file
- **No binary detection**: Returns garbled text for binary files
- **No privacy checks**: Reads any file in working_dir
- **Raw file IO**: Direct fs::read_to_string
- **No outline fallback**: Just fails or returns huge content

### Required Changes

```rust
// Add constants
const MAX_FILE_SIZE: usize = 50_000; // bytes, same as agent_crate hint
const MAX_LINES_DEFAULT: usize = 2000;

// Add to ReadFileArgs
pub struct ReadFileArgs {
    pub path: String,
    pub start_line: Option<u32>,
    pub end_line: Option<u32>,
    pub limit: Option<usize>,  // NEW: max lines to return
}

// Implementation changes:
1. Check file size before reading, return outline if too large
2. Detect binary files (check for null bytes in first 8KB)
3. Add configurable exclusion patterns
4. Truncate at MAX_LINES_DEFAULT if no range specified
5. Return metadata about truncation
```

---

## 3. GREP - GAP

### agent_crate Features (1188 LOC)
- **Syntax awareness**: Uses tree-sitter to find containing symbols
- **Ancestor context**: Shows enclosing function/struct/etc.
- **Pagination**: 20 results per page with offset parameter
- **Privacy/exclusion**: Respects file_scan_exclusions and private_files
- **Case sensitivity option**: Explicit case_sensitive flag
- **Include patterns**: Glob patterns for filtering
- **Project search integration**: Uses project's search infrastructure
- **Buffer parsing**: Waits for syntax parsing to complete

### crow_agent Implementation (320 LOC)
- **No syntax awareness**: Just shows matching line with optional context
- **No pagination**: Returns up to 100 results, truncates rest
- **No privacy checks**: Searches any file in working_dir
- **Case insensitive only**: Via `(?i)` regex prefix
- **Extension filter**: Single extension, not glob patterns
- **Raw file walking**: WalkDir with manual filtering
- **No parsing**: Just regex match on lines

### Required Changes

```rust
// Add to GrepArgs
pub struct GrepArgs {
    pub pattern: String,  // regex -> regex
    pub include_pattern: Option<String>,  // NEW: glob for files
    pub offset: u32,  // NEW: pagination offset
    pub case_sensitive: bool,  // NEW: explicit flag
}

// Constants
const RESULTS_PER_PAGE: u32 = 20;

// Implementation changes:
1. Add pagination with offset/limit
2. Support glob patterns for include_pattern
3. Add exclusion pattern support
4. Optional: Add syntax tree context (requires tree-sitter integration)
5. Better output format with file headers and line ranges
```

---

## 4. LIST_DIRECTORY - Moderate Gap

### agent_crate Features (660 LOC)
- **Privacy/exclusion**: Respects file_scan_exclusions and private_files
- **Worktree awareness**: Shows project-relative paths with worktree prefix
- **Separate lists**: Folders and files in separate sections
- **Root handling**: Special case for "." returns worktree roots
- **Path style**: Uses platform-appropriate path separators

### crow_agent Implementation (175 LOC)
- **No privacy checks**: Lists everything
- **Raw paths**: Shows filesystem paths
- **Mixed output**: Files and dirs together with "/" suffix
- **Recursive**: Has depth parameter (agent_crate is non-recursive)

### Required Changes

```rust
1. Add exclusion pattern support
2. Separate folders and files in output
3. Consider removing depth parameter (match agent_crate behavior)
4. Better handling of "." and empty path
```

---

## 5. FIND_PATH - Different Approach

agent_crate doesn't have an equivalent tool. They use `grep` for content search and `list_directory` for structure exploration. Our `find_path` is a glob-based filename search which is useful but different.

### Recommended Improvements

```rust
1. Fix ReDoS vulnerability in glob_to_regex
2. Add option for case-sensitive search  
3. Show total count even when truncating results
4. Support character class globs [abc]
```

---

## 6. TERMINAL - OK (By Design)

Both implementations allow arbitrary command execution. This is intentional - the terminal is meant to be powerful.

agent_crate likely has similar or more restricted terminal, but the key insight is:
> "rm -rf with the terminal and do wild shit that's a given"

No changes needed for parity.

---

## Implementation Priority

### Phase 1: Critical (edit_file rewrite)
The edit_file tool needs a complete redesign. Options:

**Option A: Full Edit Agent (like agent_crate)**
- Requires: Sub-agent with its own model calls
- Complexity: High
- Benefit: Natural language edit instructions work

**Option B: Enhanced Search/Replace**
- Requires: Better modes, diff output, atomic writes
- Complexity: Medium  
- Benefit: Simpler, deterministic

**Recommendation**: Start with Option B, add Option A later.

### Phase 2: Important (read_file, grep)
```
1. read_file: Add size limits, binary detection, outline fallback
2. grep: Add pagination, include patterns, case sensitivity
```

### Phase 3: Polish (list_directory, find_path)
```
1. list_directory: Add exclusion support, clean up output
2. find_path: Fix ReDoS, add case sensitivity
```

---

## Detailed Changes for edit_file (Option B)

```rust
// New modes enum
#[derive(Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EditMode {
    Edit,      // Modify existing file (old_string must exist and be unique)
    Create,    // Create new file (must not exist)
    Overwrite, // Replace entire file contents
}

// Updated args
pub struct EditFileArgs {
    pub path: String,
    pub mode: EditMode,
    pub display_description: String,  // One-line description of the edit
    
    // For Edit mode:
    pub old_string: Option<String>,
    pub new_string: Option<String>,
    
    // For Create/Overwrite mode:
    pub content: Option<String>,
    
    pub replace_all: Option<bool>,  // Only for Edit mode
}

// New output
pub struct EditFileOutput {
    pub path: String,
    pub diff: String,  // Unified diff format
    pub message: String,
}

// Implementation:
1. Validate mode vs existing file
2. For Edit: require unique old_string match (or replace_all)
3. For Create: fail if file exists
4. For Overwrite: replace entire content
5. Generate unified diff
6. Atomic write (write to .tmp, rename)
7. Return diff in output
```

---

## Next Steps

1. [ ] Implement read_file size limits (quick win)
2. [ ] Implement grep pagination (quick win)  
3. [ ] Rewrite edit_file with modes and diff output
4. [ ] Add exclusion pattern support to all tools
5. [ ] Write tests for new functionality
6. [ ] Document final behavior
