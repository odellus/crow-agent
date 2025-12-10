# Agent Configuration Architecture

## Overview

There are two layers of agents:

1. **User-facing agents** (`mode: primary`) - Selected via `--agent NAME`
2. **Internal agents** (`mode: coagent` or `mode: subagent`) - Never selected directly

The user-facing agent config defines:
- The agent's behavior (tools, permissions, system prompt)
- The control flow (what happens between turns)
- If using coagent control flow, which coagent to use

## Control Flow Types

| Control Flow | Description | Requires |
|--------------|-------------|----------|
| `passthrough` | Return to user after each turn (HITL) | Nothing |
| `loop` | Keep going until `task_complete` | `task_complete` tool |
| `static` | Inject static message between turns | `static_message` field |
| `generated` | Generate acceptance criteria once, inject each turn | `generate_prompt` field |
| `coagent` | Dual-agent with oversight | `coagent` field pointing to coagent config |

## Agent Modes

| Mode | Selectable by User | Purpose |
|------|-------------------|---------|
| `primary` | Yes (`--agent NAME`) | User-facing agents |
| `coagent` | No | Used by primary agents with `control_flow: coagent` |
| `subagent` | No | Spawned by Task tool |

## Example Configurations

### Primary Agents (User Selects These)

```yaml
# build.yaml - Basic HITL mode
name: build
mode: primary
control_flow: passthrough
description: Implementation agent, returns after each turn
# ... tools, permissions, prompt
```

```yaml
# build-loop.yaml - Autonomous until task_complete
name: build-loop
mode: primary
control_flow: loop
description: Implementation agent, runs until task_complete
# ... tools, permissions, prompt
```

```yaml
# build-static.yaml - Inject static message between turns
name: build-static
mode: primary
control_flow: static
static_message: "Continue with the task. Call task_complete when done."
description: Implementation agent with static prompting
# ... tools, permissions, prompt
```

```yaml
# build-prompt.yaml - Generate AC once, inject each turn
name: build-prompt
mode: primary
control_flow: generated
generate_prompt: prompts/acceptance_criteria.md
description: Implementation agent with generated acceptance criteria
# ... tools, permissions, prompt
```

```yaml
# build-chat.yaml - Dual-agent with chat coagent (no tools)
name: build-chat
mode: primary
control_flow: coagent
coagent: chat
description: Implementation agent with conversational oversight
# ... tools, permissions, prompt
```

```yaml
# build-judge.yaml - Dual-agent with judge coagent (has task_complete)
name: build-judge
mode: primary
control_flow: coagent
coagent: judge
description: Implementation agent with verification oversight
# ... tools, permissions, prompt (NO task_complete - judge has it)
```

```yaml
# build-build.yaml - Pair programming (both have tools)
name: build-build
mode: primary
control_flow: coagent
coagent: cobuild
description: Pair programming mode
# ... tools, permissions, prompt
```

### Coagents (Internal, Not User-Selectable)

```yaml
# chat.yaml - Conversational coagent, no tools
name: chat
mode: coagent
description: Conversational oversight, guides without tools
# NO tools - just talks
# ... prompt for reviewing and providing feedback
```

```yaml
# judge.yaml - Verification coagent, has task_complete
name: judge
mode: coagent
description: Verification oversight, can complete task
tools:
  task_complete: true
  # read-only tools for verification
  read_file: true
  grep: true
  bash: true  # for running tests
permissions:
  bash:
    "cargo test*": allow
    "npm test*": allow
    "*": deny
# ... prompt for verification
```

```yaml
# cobuild.yaml - Pair programming coagent, full tools
name: cobuild
mode: coagent
description: Pair programming partner
# Full tools including task_complete
# ... prompt for collaborative coding
```

### Subagents (Spawned by Task Tool)

```yaml
# general.yaml - Research subagent
name: general
mode: subagent
description: General-purpose research agent
control_flow: loop  # Subagents run until task_complete
# ... tools, permissions, prompt
```

```yaml
# verified.yaml - Dual-agent subagent (executor + arbiter)
name: verified
mode: subagent
control_flow: coagent
coagent: arbiter
description: Verified execution with arbiter oversight
# ... tools (NO task_complete), permissions, prompt
```

## How It Works

### Single Agent Flow (passthrough/loop/static/generated)

```
CLI/ACP
   │
   └─> Agent (primary config)
          │
          └─> BaseAgent (ReAct loop)
                 │
                 └─> Tools
```

### Dual-Agent Flow (coagent)

```
CLI/ACP  ─────────────────────────────────────────────────>
   │                    (sees one unified assistant)
   │
   └─> Agent (primary config + coagent config)
          │
          ├─> Primary BaseAgent (ReAct loop)
          │      │
          │      └─> Tools (primary's tools)
          │
          └─> Coagent BaseAgent (ReAct loop)
                 │
                 └─> Tools (coagent's tools)
                 
Orchestration:
1. Primary executes turn
2. Primary's turn humanized -> sent as "user" to coagent
3. Coagent executes turn
4. Coagent's turn humanized -> sent as "user" to primary
5. Repeat until task_complete or max turns
```

### Key Points

1. **CLI/ACP sees ONE agent** - all streaming is `role: assistant`
2. **Shared TodoStore** - primary and coagent see same todo state
3. **Coagent session is inverted** - primary's messages become user messages for coagent
4. **Control flow is on the primary** - the primary config determines which coagent (if any)
5. **Coagents can't be selected directly** - `mode: coagent` prevents `--agent NAME`

## File Locations

- Global: `~/.config/crow/agents/*.yaml`
- Project: `.crow/agents/*.yaml` (overrides global)

Project configs override global configs with the same name.
