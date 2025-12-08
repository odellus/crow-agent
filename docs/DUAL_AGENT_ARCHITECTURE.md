# Dual Agent Architecture

## Overview

This document captures the design decisions for crow_agent's multi-agent architecture. The core insight: **DualAgent is a first-class agent type, not a hack on top of sessions.**

We want:
- Internal telemetry with full fidelity (every LLM call, both agents)
- External presentation as a single ACP session (one long assistant response)
- Agent selection via ACP modes (same mechanism as permissions)
- No ad-hoc structures - this is baked into the architecture from day one

## The Problem

Single-agent ReAct loops have a fundamental flaw: **the agent decides when it's done**.

This leads to:
- Premature termination ("I've completed the task" when it hasn't)
- No verification of claims ("tests pass" without running them)  
- No recovery from stalls (agent gives up, user has to re-prompt)
- False confidence (agent sounds certain, is wrong)

The current approach (ReAct loop, MAX_TOOL_TURNS, hope for the best) is fundamentally limited.

## Autonomy Levels

We define five levels of agent autonomy, each addressing the "what happens when the loop stops?" question differently:

| Level | Name | Description | Exit Condition |
|-------|------|-------------|----------------|
| L1 | React | Single agent, stops when no tools called | Agent stops calling tools |
| L2 | Prompted | React + static "did you finish?" follow-up | Agent confirms completion |
| L3 | Judge | React + LLM-as-judge review (no tools) | Judge approves |
| L4 | Verifier | React + read-only verifier agent | Verifier approves |
| L5 | Dual | Full executor + discriminator, both have tools | Discriminator calls `task_complete` |

Each level builds on the previous. L1 is what most agents do today. L5 is full autonomous coding with verification.

## The Dual Agent Pattern

At L5 (and partially L4), we have two agents:

### Executor
- Does the actual work (writes code, runs commands, etc.)
- Has full tool access
- Cannot declare task complete
- Summarizes work when prompted

### Discriminator (Arbiter)
- Reviews executor's work
- Can have tools (L5) or be read-only (L4)
- **Only agent that can call `task_complete`**
- Sees executor's output as "user" messages (role flip)
- Skeptical by default: "verify claims, don't trust summaries"

### The Loop

```
User Request
    ↓
┌─────────────────────────────────────────┐
│  Executor Turn                          │
│  - Receives task/feedback               │
│  - Works until tool loop exhausts       │
│  - Summarizes work when prompted        │
└─────────────────────────────────────────┘
    ↓
┌─────────────────────────────────────────┐
│  Discriminator Turn                     │
│  - Receives executor's summary          │
│  - Verifies claims (runs tests, checks) │
│  - Either:                              │
│    → task_complete (done!)              │
│    → feedback summary (back to executor)│
└─────────────────────────────────────────┘
    ↓
[Loop until task_complete or max iterations]
```

### Key Design Decisions

1. **`task_complete` as the only exit gate** - Discriminator controls termination. No more "agent thinks it's done but isn't."

2. **Role flipping** - Discriminator sees executor's work as "user" messages. This reframes verification naturally - discriminator is responding to a user (executor) claiming completion.

3. **Summarization checkpoints** - Instead of passing full history between agents, each summarizes their work. Keeps context manageable, avoids explosion.

4. **Shared state** - Todo list is shared between both agents. They see the same task state.

5. **Cancel = discriminator feedback** - If user cancels mid-execution, treat it like discriminator giving feedback. Don't lose work.

## Telemetry Model

### The Problem

We need full telemetry for debugging/replay, but ACP clients should see one session.

### Solution: Internal vs External Sessions

```
┌─────────────────────────────────────────────────────────────┐
│  External (ACP) Session                                     │
│  session_id: "acp-123"                                      │
│  agent_type: "dual"                                         │
│  mode: "L5"                                                 │
│                                                             │
│  Appears as: User → [long assistant response] → User → ...  │
│  All internal turns compressed into assistant blocks        │
└─────────────────────────────────────────────────────────────┘
          │
          │ links to
          ▼
┌─────────────────────────┐    ┌─────────────────────────┐
│  Internal: Executor     │    │  Internal: Discriminator│
│  trace_id: "exec-789"   │◄──►│  trace_id: "disc-012"   │
│  parent: "acp-123"      │    │  parent: "acp-123"      │
│  role: "executor"       │    │  role: "discriminator"  │
│  sibling: "disc-012"    │    │  sibling: "exec-789"    │
│                         │    │                         │
│  Full message history   │    │  Full message history   │
│  All tool calls         │    │  All tool calls         │
│  Token counts           │    │  Token counts           │
│  Latency per turn       │    │  Latency per turn       │
└─────────────────────────┘    └─────────────────────────┘
```

### What Gets Stored

**External trace (ACP-facing):**
- Session ID, timestamps
- Agent type and mode
- Links to internal traces
- Compressed message stream (what client sees)

**Internal traces (per-agent):**
- Full message history with roles
- Every tool call and result
- Token counts (input/output)
- Latency per LLM call
- Model ID and provider
- Prompt template ID (if applicable)
- Parent ACP session link
- Sibling trace link

### CLI Access

```bash
# See external sessions
crow-cli telemetry external

# Drill into internal traces
crow-cli telemetry external-trace <id>
# Shows: linked internal traces

crow-cli telemetry trace <internal-id>
# Shows: full executor or discriminator history
```

## ACP Integration

### Modes as Agent Architecture

ACP already has modes (permissions in Claude Code). We extend this for agent architecture:

```json
{
  "modes": {
    "currentModeId": "dual",
    "availableModes": [
      {
        "id": "react",
        "name": "React",
        "description": "Single agent, stops when no tools called"
      },
      {
        "id": "prompted", 
        "name": "Prompted",
        "description": "Single agent with completion confirmation"
      },
      {
        "id": "judge",
        "name": "Judge",
        "description": "Single agent with LLM review"
      },
      {
        "id": "verified",
        "name": "Verified", 
        "description": "Executor with read-only verifier"
      },
      {
        "id": "dual",
        "name": "Dual Agent",
        "description": "Full executor + discriminator with tools"
      }
    ]
  }
}
```

Mode selection happens via `session/set_mode` - same as switching permissions in Claude Code.

### Model Configuration

Dual-agent needs two model configs. Options:

**Option A: Mode-specific model slots**
```json
{
  "modes": {
    "currentModeId": "dual",
    "availableModes": [
      { "id": "react", "modelSlots": ["primary"] },
      { "id": "dual", "modelSlots": ["executor", "discriminator"] }
    ]
  },
  "models": {
    "primary": { "currentModelId": "opus", "available": [...] },
    "executor": { "currentModelId": "local-llama", "available": [...] },
    "discriminator": { "currentModelId": "sonnet", "available": [...] }
  }
}
```

**Option B: Extend setSessionModel**
```
setSessionModel(sessionId, modelId, role?: "primary" | "executor" | "discriminator")
```

**Option C: Mode carries defaults, override via config**
```json
{
  "mode": "dual",
  "config": {
    "executor_model": "local-llama-70b",
    "discriminator_model": "claude-sonnet"
  }
}
```

For now, we go with **Option C** - modes have sensible defaults, users can override via session config. This works with existing ACP spec and we can propose spec extensions later.

### Local Model Support

Critical requirement: different providers for each agent.

```rust
struct DualAgentConfig {
    executor: AgentConfig {
        provider: "lmstudio",
        model: "llama-3.1-70b",
        endpoint: "http://localhost:1234",
    },
    discriminator: AgentConfig {
        provider: "anthropic", 
        model: "claude-sonnet",
        api_key: env("ANTHROPIC_API_KEY"),
    },
}
```

Use case: Big local model for coding (executor), cloud model for judgment (discriminator).

## Implementation in crow_agent

### Agent Trait

```rust
pub trait Agent {
    async fn run(&self, request: &str, session: &mut Session) -> Result<AgentResult>;
}

// Single agent (L1-L3)
pub struct ReactAgent { ... }

// Dual agent (L4-L5)  
pub struct DualAgent {
    executor: Box<dyn Agent>,
    discriminator: Box<dyn Agent>,
    config: DualAgentConfig,
}

impl Agent for DualAgent {
    // Orchestrates the loop, presents as single response
}
```

### Session Structure

```rust
pub struct Session {
    // ACP-facing
    pub id: SessionId,
    pub mode: AgentMode,
    
    // Internal (optional, only for dual modes)
    pub internal_sessions: Option<InternalSessions>,
}

pub struct InternalSessions {
    pub executor: InternalSession,
    pub discriminator: InternalSession,
    pub pair_id: String,  // Links them together
}

pub struct InternalSession {
    pub trace_id: String,
    pub messages: Vec<Message>,
    pub tool_calls: Vec<ToolCall>,
    pub token_usage: TokenUsage,
}
```

### The Orchestration Loop

```rust
impl DualAgent {
    async fn run(&self, request: &str, session: &mut Session) -> Result<AgentResult> {
        let mut iteration = 0;
        
        loop {
            // Executor turn
            let exec_result = self.executor.run(
                &self.format_executor_input(request, feedback),
                &mut session.internal_sessions.executor,
            ).await?;
            
            // Request summary from executor
            let summary = self.request_summary(&exec_result).await?;
            
            // Discriminator turn
            let disc_result = self.discriminator.run(
                &self.format_discriminator_input(summary),
                &mut session.internal_sessions.discriminator,
            ).await?;
            
            // Check for task_complete
            if disc_result.has_tool_call("task_complete") {
                return Ok(self.finalize(exec_result, disc_result));
            }
            
            // Get feedback for next iteration
            feedback = self.extract_feedback(&disc_result).await?;
            
            iteration += 1;
            if iteration >= self.config.max_iterations {
                return Ok(self.finalize_incomplete(exec_result, disc_result));
            }
        }
    }
}
```

## Existing Code Reference

We have prior art in the codebase:

- `agent_crate/src/dual.rs` - DualAgentOrchestrator with ACP integration
- `crow-old/core/src/agent/dual.rs` - DualAgentRuntime with full telemetry
- `crow-old/core/src/agent/primary_dual.rs` - Human-in-the-loop variant
- `discriminator_prompt.hbs` - Discriminator system prompt template

Key patterns from existing code:
- `link_siblings()` connects executor and discriminator sessions
- `tool_registry.share_todo_sessions()` shares state between agents
- Role setup: Discriminator sees `USER: "How can I help?"` then `ASSISTANT: <request>` then `USER: <executor work>`

## Open Questions

1. **Prompt management** - How do we version/track the discriminator prompt? Link to telemetry?

2. **Tool compression** - For passing history, how aggressively do we compress tool calls?

3. **Interrupt handling** - User cancels mid-dual-agent. Do we:
   - Treat as discriminator feedback?
   - Save state for resume?
   - Both?

4. **ACP spec proposals** - Do we propose multi-model support upstream, or keep it crow-specific?

## Summary

- DualAgent is a first-class agent type implementing the Agent trait
- Internal sessions have full telemetry, external ACP session sees compressed stream
- Modes map to autonomy levels (L1-L5), selected via ACP `setSessionMode`
- Each agent in dual mode can have different model/provider
- `task_complete` tool is the only exit gate for discriminator
- Telemetry links everything: ACP session → internal traces → sibling traces
