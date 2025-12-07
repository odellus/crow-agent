# Edit Tool Implementation Plan

## Current State

### crow_agent (What We Have)

Our current `crow_agent/src/tools/edit_file.rs` is a **synchronous, exact-match only** implementation:

```rust
// Current approach - simple search/replace
impl Tool for EditFile {
    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        // Read file
        let old_content = std::fs::read_to_string(&path)?;
        
        // Exact string match only
        let count = old_content.matches(&old_string).count();
        if count == 0 { return Err(OldStringNotFound); }
        if count > 1 && !replace_all { return Err(MultipleMatches(count)); }
        
        // Simple replacement
        let new_content = old_content.replace(&old_string, &new_string);
        
        // Write atomically
        Self::atomic_write(&path, &new_content)?;
    }
}
```

**Problems:**
- No fuzzy matching - LLM must provide exact text (whitespace, indentation matter)
- No indentation normalization - LLM must match file's exact indentation
- Fails on minor LLM mistakes (extra space, wrong indent)

## Decision: Use OpenCode/crow-tauri Cascading Replacers

After analyzing claude-code-acp behavior, we confirmed:
- **Tool calls are NOT streamed in real-time** - they send "tool starting" then "tool complete with result"
- **LLM responses ARE streamed** - AgentMessageChunk for text output
- Zed's streaming edit visualization is overkill for our needs

We're choosing the **OpenCode cascading replacer approach** because:

1. **Simpler implementation** - No streaming infrastructure for edits needed
2. **Battle-tested** - OpenCode/Cline use this in production
3. **Robust fuzzy matching** - 9 different strategies handle LLM mistakes gracefully
4. **ACP-compatible** - Send ToolCall on start, ToolCallUpdate with Diff on completion

## Architecture Overview

```
┌─────────────────────────────────────────────────────────────────┐
│                         EditFileTool                             │
│  - Validates path                                                │
│  - Reads file content                                            │
│  - Calls replace() with cascading replacers                      │
│  - Writes result atomically                                      │
│  - Returns Diff for ACP ToolCallUpdate                          │
└─────────────────────────────────────────────────────────────────┘
                                │
                                ▼
┌─────────────────────────────────────────────────────────────────┐
│                    replace() function                            │
│  Tries each replacer in order until one succeeds:               │
│                                                                  │
│  1. SimpleReplacer         - Exact match                        │
│  2. LineTrimmedReplacer    - Trim whitespace per line           │
│  3. BlockAnchorReplacer    - First/last line anchors + Levenshtein│
│  4. WhitespaceNormalizedReplacer - Collapse whitespace          │
│  5. IndentationFlexibleReplacer  - Normalize indentation        │
│  6. EscapeNormalizedReplacer     - Handle \n, \t, etc.          │
│  7. TrimmedBoundaryReplacer      - Trim block boundaries        │
│  8. ContextAwareReplacer         - 50% middle line match        │
│  9. MultiOccurrenceReplacer      - All exact matches            │
└─────────────────────────────────────────────────────────────────┘
```

## The 9 Cascading Replacers (from OpenCode)

### 1. SimpleReplacer
Exact string match. The baseline.
```rust
fn simple_replace(content: &str, find: &str) -> Option<&str> {
    if content.contains(find) { Some(find) } else { None }
}
```

### 2. LineTrimmedReplacer
Trim whitespace from each line, then match.
```rust
// "  foo  " matches "foo" in "    foo    "
```

### 3. BlockAnchorReplacer
Use first and last lines as anchors, fuzzy match middle with Levenshtein.
- Single candidate: similarity threshold 0.0 (very permissive)
- Multiple candidates: similarity threshold 0.3
```rust
// First line: "fn foo() {"
// Last line: "}"
// Middle: fuzzy match with Levenshtein distance
```

### 4. WhitespaceNormalizedReplacer
Collapse all whitespace to single space, then match.
```rust
// "foo   bar\n  baz" matches "foo bar baz"
```

### 5. IndentationFlexibleReplacer
Remove minimum indentation from block, then match.
```rust
// "    foo\n        bar" normalized to "foo\n    bar"
```

### 6. EscapeNormalizedReplacer
Handle escape sequences: `\n`, `\t`, `\r`, `\'`, `\"`, etc.
```rust
// "foo\\nbar" matches "foo\nbar"
```

### 7. TrimmedBoundaryReplacer
Trim leading/trailing whitespace from entire block.
```rust
// "\n  foo\n  " matches "foo"
```

### 8. ContextAwareReplacer
Use first/last lines as anchors, require 50% of middle lines to match exactly.
```rust
// More strict than BlockAnchorReplacer for disambiguation
```

### 9. MultiOccurrenceReplacer
Find all exact matches (for `replace_all` mode).

## Implementation Plan

### Phase 1: Port Replacers to Rust

Create `crow_agent/src/tools/edit/replacers.rs`:

```rust
pub trait Replacer {
    /// Yields all possible matches for `find` in `content`
    fn find_matches<'a>(&self, content: &'a str, find: &str) -> Vec<&'a str>;
}

pub struct SimpleReplacer;
pub struct LineTrimmedReplacer;
pub struct BlockAnchorReplacer;
pub struct WhitespaceNormalizedReplacer;
pub struct IndentationFlexibleReplacer;
pub struct EscapeNormalizedReplacer;
pub struct TrimmedBoundaryReplacer;
pub struct ContextAwareReplacer;
pub struct MultiOccurrenceReplacer;

/// Try each replacer in order, return first successful replacement
pub fn replace(
    content: &str, 
    old_string: &str, 
    new_string: &str, 
    replace_all: bool
) -> Result<String, EditError>;
```

### Phase 2: Add Levenshtein Distance

Add `strsim` crate for Levenshtein distance:
```toml
[dependencies]
strsim = "0.11"
```

Used by `BlockAnchorReplacer` for fuzzy middle-line matching.

### Phase 3: Update EditFile Tool

Modify `crow_agent/src/tools/edit_file.rs`:
- Replace simple `content.matches()` with cascading `replace()`
- Keep existing atomic write logic
- Keep existing diff generation

### Phase 4: ACP Integration (Later)

When we add ACP support:
1. Send `SessionUpdate::ToolCall` when edit starts
2. Execute edit synchronously  
3. Send `SessionUpdate::ToolCallUpdate` with `Diff` content on completion

## File Structure

```
crow_agent/src/tools/
├── edit_file.rs              # Main tool (updated)
├── edit/
│   ├── mod.rs
│   ├── replacers.rs          # 9 cascading replacers
│   └── levenshtein.rs        # Or use strsim crate
```

## Testing Strategy

Port OpenCode's test cases:
- Exact match
- Whitespace variations
- Indentation differences  
- Escape sequences
- Multiple matches (with and without replace_all)
- Block anchor matching
- Edge cases (empty strings, single lines, etc.)

## Dependencies

```toml
[dependencies]
strsim = "0.11"  # Levenshtein distance
diff = "0.1"     # Already have for unified diff output
```

## What We're NOT Doing

- ❌ Streaming edit visualization (Zed's approach)
- ❌ Secondary LLM call for complex edits
- ❌ Real-time fuzzy match resolution UI
- ❌ Character-by-character diff streaming

These are nice-to-have but not needed. Claude-code-acp doesn't do them either.

## What We ARE Doing

- ✅ Robust fuzzy matching via cascading replacers
- ✅ Handle LLM whitespace/indentation mistakes
- ✅ Levenshtein-based block matching
- ✅ Stream LLM responses (AgentMessageChunk)
- ✅ Send tool completion with Diff (ToolCallUpdate)

## References

- `opencode/packages/opencode/src/tool/edit.ts` - The 9 replacers we're porting
- `crow-tauri` - Rust adaptation (if available)
- `claude-code-acp` - Reference for ACP tool call flow
