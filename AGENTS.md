# Agent Guidelines for crow_agent

## Build and Test
- Always do full release build: `cargo build --release`
- Test against the release binary, not debug
- Kill zombie processes before testing
- Test session continuation, not just single-shot prompts

## Code Quality
- Think carefully before modifying/editing code
- Always have a mental model of how the code works before changing it
- Look at how existing code (Zed, rig, opencode) does something before implementing
- Don't add unnecessary abstractions or duplicate existing functionality
- Don't assume - verify by reading the code and checking the database

## Data Structures
- Use `BTreeMap` or `IndexMap` for deterministic ordering
- Never use `HashMap` when iteration order matters (e.g., tools, messages)

## Debugging
- Use the built-in SQL query function: `crow-agent query "SELECT ..."`
- Don't use Python for parsing/processing when there's a built-in tool
- When something isn't working, check the actual data being sent (query the traces table)

## HTTP/Networking
- Use `Connection: close` header for requests that need clean termination
- Signal handlers need to actually exit the process, not just set a flag

## General
- Don't be incompetent
