# E2E Test Scripts

Each script tests one tool. Run individually and evaluate the output.

## Usage

```bash
# Set crow-agent path (or it defaults to ../../target/release/crow-agent)
export CROW_AGENT="/path/to/crow-agent"

# Run a single test
bash 01_bash.sh

# Or run and evaluate
bash 03_edit.sh
# Then check: Did file change? How many tool calls? Any red flags?
```

## Tests

| Script | Tool | What to Check |
|--------|------|---------------|
| 01_bash.sh | Bash | Output correct, 1 tool call |
| 02_read.sh | Read | Agent reports content correctly |
| 03_edit.sh | Edit | File changed on disk |
| 04_grep.sh | Grep | Correct files found |
| 05_find.sh | Find | Correct files, watch call count |
| 06_list.sh | List | Directory contents shown |
| 07_write.sh | Write | File created (via bash currently) |
| 08_errors.sh | Errors | No hallucination |
| 09_workflow.sh | Multi | Correct sequence |

## Agent Evaluation

After running each test, the script prints "AGENT: Verify" with specific things to check. Use judgment - a test can "pass" but still have issues (like too many tool calls).

## Red Flags

- Tool called 5+ times for simple task
- File unchanged after edit
- Agent uses bash when dedicated tool exists
- Agent hallucinates content
