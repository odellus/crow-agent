# Plan: Agent Tools, Coagents, and Control Flow

## Goal

Agents defined entirely in config (YAML/TOML + handlebars prompts). Code Just Works.

- System prompt, tools, permissions, autonomy level, model - all declarative
- Built-in agents: general, build, plan, executor, arbiter, planner, architect
- Custom agents via `~/.config/crow/agents/` or `.crow/agents/`

## Current State Analysis

### crow-old vs crow_agent

**crow-old** (the reference):
- 17 tools including Task (64KB) for subagent spawning
- AgentInfo with full permissions model (edit, bash patterns, webfetch, doom_loop)
- AgentMode: Primary/Subagent/All
- Built-in agents: general, build, plan, executor, arbiter, planner, architect
- Dual-agent system (executor + arbiter) with shared todo state
- Rich bash descriptions with git/PR workflows

**crow_agent** (what we're building):
- 15 tools in src/tools/ (rig-based), only 2 in tools2 (our trait)
- AgentConfig exists but simpler than crow-old's AgentInfo
- BaseAgent + ACPAgent architecture (good foundation)
- ControlFlow enum for autonomy levels
- Missing: Task tool, agent loading from config, permission system

### Two Tool Systems in crow_agent

**src/tools/** - Uses `rig::tool::Tool` trait
- 14 tools with rich implementations
- edit_file has 9 cascading fuzzy matchers (gold!)
- Structured output types

**src/tools2/** - Uses our own `Tool` trait (no rig)
- Only 2 tools: read_file, task_complete
- Needs remaining tools ported

### Agent Architecture (crow_agent - good foundation)

**BaseAgent** (`src/agent/base.rs`)
- Internal ReAct loop, streams events, executes tools
- Uses `ToolExecutor` trait (implemented by `ToolRegistry`)

**ACPAgent** (`src/agent/control_flow.rs`)  
- External orchestration, coagent handoff
- ControlFlow enum exists but autonomy should come from agent config

**AgentConfig** (`src/agent/config.rs`)
- Has: name, description, mode, model overrides, ToolPermissions
- Missing: full permissions model from crow-old (bash patterns, edit deny, etc.)

### crow-old Agent System (what we want)

**AgentInfo** - full agent definition:
```rust
struct AgentInfo {
    name: String,
    description: Option<String>,
    mode: AgentMode,  // Primary/Subagent/All
    built_in: bool,
    temperature: Option<f32>,
    top_p: Option<f32>,
    color: Option<String>,
    permission: AgentPermissions,  // edit, bash patterns, webfetch, doom_loop
    model: Option<AgentModel>,
    prompt: Option<String>,  // system prompt override
    tools: HashMap<String, bool>,  // tool enable/disable
}
```

**AgentPermissions** - granular control:
```rust
struct AgentPermissions {
    edit: Permission,  // Allow/Deny/Ask
    bash: HashMap<String, Permission>,  // "git *" -> Allow, "*" -> Ask
    webfetch: Option<Permission>,
    doom_loop: Option<Permission>,
    external_directory: Option<Permission>,
}
```

**Built-in Agents** (from crow-old):
- `general` - Subagent for research, no todos
- `build` - Primary, full tools, no task_complete
- `plan` - Primary, read-only bash whitelist, no edit
- `executor` - Subagent for dual-agent, no task/task_complete
- `arbiter` - Subagent, verifies executor, CAN task_complete
- `planner` - Primary for dual-agent mode
- `architect` - Primary verifier, CAN task_complete

---

## What Needs to Be Done

### Phase 1: Port Tools to tools2

1. **terminal/bash** - with crow-old's rich description (git workflows)
2. **edit_file** - preserve 9 cascading fuzzy matchers
3. **grep** - with pagination
4. **list_directory, find_path** 
5. **todo_read, todo_write** - separate tools, shared state for dual-agent
6. **thinking, now**
7. **fetch, web_search**
8. **diagnostics**
9. **Task** - subagent spawning (big one)

### Phase 2: Upgrade AgentConfig

Merge crow-old's AgentInfo into crow_agent's AgentConfig:
- Add `AgentPermissions` with bash pattern matching
- Add `prompt` field for system prompt override/template
- Support handlebars templates for prompts

### Phase 3: Agent Loading from Config

```
~/.config/crow/agents/
  researcher.yaml
  reviewer.yaml
  
.crow/agents/
  project-specific.yaml
```

Agent YAML:
```yaml
name: researcher
description: "Research agent with read-only access"
mode: subagent
temperature: 0.7
permissions:
  edit: deny
  bash:
    "git log*": allow
    "git diff*": allow
    "rg*": allow
    "*": ask
tools:
  task_complete: false
  todowrite: false
prompt: |
  You are a research agent. You can read files and search code
  but cannot make changes. Report findings clearly.
```

### Phase 4: Built-in Agents

Port crow-old's built-in agents as default configs:
- general, build, plan, executor, arbiter, planner, architect
- Embed as const YAML or load from bundled files

### Phase 5: Task Tool for Subagents

Port crow-old's Task tool:
- Spawns subagent with isolated context
- Supports `verified` mode (executor + arbiter dual-agent)
- Shared todo state between executor/arbiter

### Phase 6: Wire Up in crow-agent-dev

- `--agent <name>` flag to select agent
- Load built-in + custom agents
- Apply permissions when executing tools

---

## Key Insight

The autonomy level isn't a number - it's defined by the agent config:
- Which tools are enabled/disabled
- Bash command permissions
- Whether edit is allowed
- System prompt behavior

A "level 5" agent is just one with full permissions + coagent verification.
A "level 0" agent is one that loops until task_complete.
Define it in config, not code.
