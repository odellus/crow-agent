# Crow Agent Architecture: ACP Rewrite Specification

## Executive Summary

This document specifies the architecture for a complete rewrite of crow-agent's internals while preserving the ACP (Agent Control Protocol) interface. The core insight is that **multiple internal agents should present as a single ACP agent** to the IDE/frontend.

The rewrite introduces a two-level agent abstraction:
1. **Base Agents**: The primitive react loops with tools, sessions, and telemetry
2. **ACP Agents**: Compositions of base agents with defined control flow patterns

This separation allows for sophisticated multi-agent coordination (pair programming, discriminator patterns, subagent spawning) while maintaining a clean, simple interface to ACP consumers.

---

## Table of Contents

1. [Motivation](#motivation)
2. [Current State Analysis](#current-state-analysis)
3. [Architecture Overview](#architecture-overview)
4. [Base Agent Specification](#base-agent-specification)
5. [ACP Agent Specification](#acp-agent-specification)
6. [Control Flow Patterns](#control-flow-patterns)
7. [Tool System](#tool-system)
8. [Session and History Management](#session-and-history-management)
9. [Telemetry Architecture](#telemetry-architecture)
10. [Streaming Model](#streaming-model)
11. [Configuration System](#configuration-system)
12. [Migration Path](#migration-path)
13. [Future Considerations](#future-considerations)

---

## Motivation

### The Problem

The current crow-agent implementation suffers from several architectural issues:

1. **Rig owns too much**: The rig framework controls the react loop, history management, and tool execution. We're fighting its abstractions at every layer.

2. **ACP does too much**: Logic that belongs in the agent layer (history reconstruction, snapshot management, cancellation handling) has leaked into the ACP module.

3. **No first-class multi-agent support**: Dual-agent patterns are bolted on rather than designed in. The discriminator/coagent concept exists but isn't a core primitive.

4. **Tools lack context**: Rig's `Tool` trait doesn't pass execution context (cancellation tokens, snapshot managers, session info). We have to inject these awkwardly.

5. **History is batched, not interleaved**: Rig builds history by batching all tool calls, then all tool results. This obscures the actual execution timeline and makes debugging/persistence harder.

### The Vision

**Multiple internal agents → Single ACP Agent**

From ACP's perspective, there is one agent that:
- Receives prompts
- Streams responses (text, tool calls, tool results)
- Signals completion via `task_complete`

Behind that facade, we might have:
- A primary agent doing the work
- A coagent reviewing and potentially fixing
- Subagents spawned for specific tasks
- All coordinating through well-defined control flow

ACP doesn't know or care about this internal structure. It's a dumb pipe.

---

## Current State Analysis

### What We Have (crow-agent with rig)

**Strengths:**
- ACP interface works well
- Tools from zed-native-agent function correctly
- Edit tool from opencode with fuzzy matching
- Basic streaming to ACP

**Weaknesses:**
- Rig's agent loop is a black box we can't customize
- History management duplicated between rig and ACP
- Snapshot logic lives in ACP layer instead of tools
- No proper `ToolContext` - tools can't access session, cancellation, etc.
- Dual-agent is hacked together, not architected

### What We Had (crow-old)

**Strengths:**
- Own react loop with full control
- `ToolContext` with cancellation, session info, paths
- `Tool` trait that receives context
- `ToolRegistry` for discovery
- Dual-agent patterns (dual.rs, primary_dual.rs)
- Raw HTTP streaming for reasoning token support
- Doom loop detection

**Weaknesses:**
- Dual-agent was Level 4 thinking (discriminator judged but didn't do)
- Tangled implementation, not cleanly abstracted
- No ACP interface

### What We Can Learn From (opencode)

**Strengths:**
- Clean agent definitions
- Primary vs subagent distinction
- Session management patterns
- XDG configuration
- Project-aware prompts

---

## Architecture Overview

```
┌─────────────────────────────────────────────────────────────────┐
│                         ACP Interface                           │
│  (Protocol layer - receives prompts, streams events, done)      │
└─────────────────────────────────────────────────────────────────┘
                                │
                                ▼
┌─────────────────────────────────────────────────────────────────┐
│                         ACP Agent                               │
│  (Composition layer - defines control flow between base agents) │
│                                                                 │
│  name: "BUILD-SMART"                                           │
│  primary: BaseAgent (builder)                                   │
│  coagent: Option<BaseAgent> (reviewer)                         │
│  control_flow: Smart                                            │
└─────────────────────────────────────────────────────────────────┘
                                │
                ┌───────────────┴───────────────┐
                ▼                               ▼
┌───────────────────────────┐   ┌───────────────────────────┐
│       Base Agent          │   │       Base Agent          │
│       (Primary)           │   │       (Coagent)           │
│                           │   │                           │
│  - React loop             │   │  - React loop             │
│  - Tools + ToolContext    │   │  - Tools + ToolContext    │
│  - Internal session       │   │  - Internal session       │
│  - Streaming              │   │  - Streaming              │
│  - Telemetry              │   │  - Telemetry              │
└───────────────────────────┘   └───────────────────────────┘
                │                               │
                └───────────────┬───────────────┘
                                ▼
┌─────────────────────────────────────────────────────────────────┐
│                      Tool Registry                              │
│  (Shared tools with ToolContext injection)                      │
└─────────────────────────────────────────────────────────────────┘
                                │
                                ▼
┌─────────────────────────────────────────────────────────────────┐
│                    Provider Layer                               │
│  (LLM completions - anthropic, openai, etc.)                   │
│  (Keep from rig or build minimal)                              │
└─────────────────────────────────────────────────────────────────┘
```

---

## Base Agent Specification

A Base Agent is the fundamental primitive. It is a complete, self-contained react loop that can:
- Receive a prompt
- Think and act (tool calls)
- Stream its output
- Manage its own session/history
- Report telemetry

### Base Agent Definition

```rust
pub struct BaseAgent {
    /// Unique identifier for this agent type
    pub name: String,
    
    /// Role determines invocation patterns
    pub role: AgentRole,
    
    /// System prompt (may include template variables)
    pub system_prompt: String,
    
    /// Tools available to this agent
    pub tools: Vec<Arc<dyn Tool>>,
    
    /// Optional coagent for review/coordination
    pub coagent: Option<Arc<BaseAgent>>,
    
    /// Model configuration
    pub model_config: ModelConfig,
    
    /// Maximum react iterations before forced stop
    pub max_iterations: usize,
    
    /// Doom loop detection threshold
    pub doom_loop_threshold: usize,
}

pub enum AgentRole {
    /// Invoked by human through ACP
    Primary,
    
    /// Invoked by another agent through Task tool
    Subagent,
    
    /// Note: Coagent is not a role - it's a relationship.
    /// A coagent is just a BaseAgent that another agent hands off to.
    /// The coagent itself might be Primary or Subagent role.
}

pub struct ModelConfig {
    pub provider: String,      // "anthropic", "openai", etc.
    pub model: String,         // "claude-sonnet-4-20250514", "gpt-4", etc.
    pub temperature: Option<f32>,
    pub max_tokens: Option<usize>,
}
```

### Coagent Relationship

The coagent relationship is **orthogonal to role**:

- A `Primary` agent can have a coagent (discriminator reviews human-initiated work)
- A `Subagent` can have a coagent (spawned task gets reviewed too)
- A coagent doesn't know who spawned its partner - it just reviews

```
Human → Primary (builder) → Coagent (reviewer)
              ↓
           Task tool
              ↓
        Subagent (builder) → Coagent (reviewer)  // Can be same or different reviewer
```

### React Loop Ownership

Each Base Agent owns its react loop completely:

```rust
impl BaseAgent {
    pub async fn run(&self, prompt: &str, ctx: &AgentContext) -> impl Stream<Item = AgentEvent> {
        async_stream::stream! {
            let mut history = ctx.session.history().await;
            history.push(UserMessage(prompt));
            
            for iteration in 0..self.max_iterations {
                // Check cancellation
                if ctx.cancellation.is_cancelled() {
                    yield AgentEvent::Cancelled;
                    break;
                }
                
                // Call LLM
                let response = self.completion(&history).await;
                
                // Process response - streaming text and tool calls
                for content in response {
                    match content {
                        Text(t) => {
                            history.push(AssistantText(t.clone()));
                            yield AgentEvent::TextDelta(t);
                        }
                        ToolCall(tc) => {
                            yield AgentEvent::ToolCallStart(tc.clone());
                            
                            // Execute with full context
                            let result = self.execute_tool(&tc, ctx).await;
                            
                            // History captures the real timeline
                            history.push(AssistantToolCall(tc.clone()));
                            history.push(ToolResult(tc.id, result.clone()));
                            
                            yield AgentEvent::ToolCallEnd(tc.id, result);
                        }
                    }
                }
                
                // Check if we should continue
                if !response.has_tool_calls() {
                    yield AgentEvent::TurnComplete;
                    break;
                }
                
                // Doom loop detection
                if self.detect_doom_loop(&history) {
                    yield AgentEvent::DoomLoopDetected;
                    break;
                }
            }
        }
    }
}
```

---

## ACP Agent Specification

An ACP Agent is a **composition** of Base Agents with a defined control flow. It presents as a single agent to the ACP interface.

### ACP Agent Definition

```rust
pub struct ACPAgent {
    /// Name shown in ACP agent list (e.g., "BUILD-SMART")
    pub name: String,
    
    /// Description for UI
    pub description: String,
    
    /// The primary base agent (receives human prompts)
    pub primary: Arc<BaseAgent>,
    
    /// Optional coagent for review/coordination
    pub coagent: Option<Arc<BaseAgent>>,
    
    /// Control flow pattern
    pub control_flow: ControlFlow,
    
    /// Prompt used when handing off to coagent
    pub handoff_prompt: HandoffPrompt,
}

pub enum ControlFlow {
    /// Human-in-the-loop: Primary does work, returns to human
    /// No coagent involvement
    HITL,
    
    /// Fixed iterations: Primary → Coagent → Primary → ... (n times)
    Loop { max_iterations: usize },
    
    /// Smart: Coagent decides whether to continue, do work itself, or complete
    Smart,
    
    /// Chat: Back and forth until coagent calls task_complete
    Chat { max_turns: usize },
    
    /// Judge: Coagent only evaluates (cannot do work), approves or sends back
    Judge,
}

pub enum HandoffPrompt {
    /// Static prompt text
    Static(String),
    
    /// Generate based on task context
    Generated {
        generator_prompt: String,  // Prompt to generate the handoff prompt
    },
    
    /// Template with variables (todo_state, task_description, etc.)
    Template(String),
}
```

### Control Flow Levels (from whiteboard)

The control flow patterns map to the levels discussed:

| Level | Pattern | Who Controls task_complete | Coagent Capabilities |
|-------|---------|---------------------------|---------------------|
| 0 | (no agent) | N/A | N/A |
| 1-3 | HITL | Executor | N/A |
| 4 | Judge | Coagent | Evaluate only |
| 5 | Smart/Chat | Coagent | Full (can do work) |

### Example ACP Agent Definitions

```rust
// Level 1-3: Simple single agent
ACPAgent {
    name: "BUILD-HITL",
    description: "Builder agent with human review after each turn",
    primary: builder_agent,
    coagent: None,
    control_flow: ControlFlow::HITL,
    handoff_prompt: HandoffPrompt::Static("".into()),  // Not used
}

// Level 4: Coagent judges but doesn't do
ACPAgent {
    name: "BUILD-JUDGE", 
    description: "Builder with reviewer that evaluates but doesn't modify",
    primary: builder_agent,
    coagent: Some(reviewer_agent),  // reviewer has limited tools
    control_flow: ControlFlow::Judge,
    handoff_prompt: HandoffPrompt::Template(
        "Review the work above. If acceptable, call task_complete. \
         If not, explain what needs to change."
    ),
}

// Level 5: Full pair programming
ACPAgent {
    name: "BUILD-SMART",
    description: "Builder with smart reviewer that can fix issues",
    primary: builder_agent,
    coagent: Some(reviewer_agent),  // reviewer has full tools
    control_flow: ControlFlow::Smart,
    handoff_prompt: HandoffPrompt::Template(
        "Review the work above. You may:\n\
         1. Call task_complete if the work is acceptable\n\
         2. Fix any issues yourself using the available tools\n\
         3. Provide feedback for another iteration\n\
         Current todo state: {{todo_state}}"
    ),
}

// Subagent with coagent (for Task tool spawning)
ACPAgent {
    name: "TASK-SMART",
    description: "Subagent for Task tool with smart review",
    primary: task_executor_agent,  // role: Subagent
    coagent: Some(task_reviewer_agent),
    control_flow: ControlFlow::Smart,
    handoff_prompt: HandoffPrompt::Generated {
        generator_prompt: "Generate a review prompt for this specific task..."
    },
}
```

### ACP Agent Execution

```rust
impl ACPAgent {
    pub async fn run(&self, prompt: &str, ctx: &ACPContext) -> impl Stream<Item = ACPEvent> {
        async_stream::stream! {
            match &self.control_flow {
                ControlFlow::HITL => {
                    // Simple: just run primary, forward events
                    let mut stream = self.primary.run(prompt, &ctx.agent_context()).await;
                    while let Some(event) = stream.next().await {
                        yield self.to_acp_event(event);
                        if matches!(event, AgentEvent::TurnComplete) {
                            yield ACPEvent::TaskComplete;
                        }
                    }
                }
                
                ControlFlow::Smart => {
                    let mut current_agent = &self.primary;
                    let mut current_prompt = prompt.to_string();
                    
                    loop {
                        // Run current agent
                        let mut stream = current_agent.run(&current_prompt, &ctx.agent_context()).await;
                        let mut turn_output = String::new();
                        
                        while let Some(event) = stream.next().await {
                            yield self.to_acp_event(event.clone());
                            turn_output.push_str(&event.to_text());
                        }
                        
                        // If primary just finished, hand to coagent
                        if Arc::ptr_eq(current_agent, &self.primary) {
                            if let Some(ref coagent) = self.coagent {
                                current_agent = coagent;
                                current_prompt = self.render_handoff_prompt(&turn_output, ctx);
                                continue;
                            }
                        }
                        
                        // If coagent finished, check if task_complete was called
                        if self.task_complete_called(ctx) {
                            yield ACPEvent::TaskComplete;
                            break;
                        }
                        
                        // Otherwise hand back to primary with coagent's feedback
                        current_agent = &self.primary;
                        current_prompt = turn_output;
                    }
                }
                
                // ... other control flows
            }
        }
    }
}
```

---

## Control Flow Patterns

### HITL (Human-in-the-Loop)

```
Human → Primary → [react loop] → task_complete → Human
```

The simplest pattern. Primary agent does its work, signals completion, returns to human. No coagent involved.

**Use case**: Quick tasks, when human wants direct control, debugging.

### Loop(n)

```
Human → Primary → [react] → Coagent → [react] → Primary → ... → task_complete → Human
                     └──────────────────────────────────┘
                                  (max n iterations)
```

Fixed number of back-and-forth iterations. Useful when you want guaranteed review cycles.

**Use case**: Code review workflows, iterative refinement with known bounds.

### Judge

```
Human → Primary → [react] → Coagent → [evaluate only]
                                ├── approve → task_complete → Human
                                └── reject → feedback → Primary → ...
```

Coagent can only evaluate, not modify. It either approves (task_complete) or sends feedback.

**Use case**: When you want review but don't want the reviewer modifying code. Audit trails.

### Smart

```
Human → Primary → [react] → Coagent → [evaluate + optionally do]
                                ├── approve → task_complete → Human  
                                ├── fix it myself → [react] → task_complete → Human
                                └── needs work → feedback → Primary → ...
```

Coagent decides: approve, fix it themselves, or send back. Full Level 5 capability.

**Use case**: Pair programming, complex tasks where reviewer insight might be faster than another round-trip.

### Chat

```
Human → Primary → [react] → Coagent → [react] → Primary → [react] → ...
                     └─────────────────────────────────────────────┘
                        (continues until coagent calls task_complete)
```

True back-and-forth conversation between agents. No fixed iteration limit (but has max_turns safety).

**Use case**: Design discussions, exploratory work, complex problem-solving.

---

## Tool System

### Tool Trait

```rust
#[async_trait]
pub trait Tool: Send + Sync {
    /// Tool name for LLM function calling
    fn name(&self) -> &str;
    
    /// Description for LLM
    fn description(&self) -> &str;
    
    /// JSON schema for parameters
    fn parameters_schema(&self) -> Value;
    
    /// Execute with full context
    async fn execute(&self, input: Value, ctx: &ToolContext) -> ToolResult;
}
```

### ToolContext

Every tool receives full execution context:

```rust
pub struct ToolContext {
    // Identity
    pub session_id: String,
    pub agent_name: String,
    pub call_id: String,
    
    // Paths
    pub working_dir: PathBuf,
    pub project_root: PathBuf,
    
    // Cancellation
    pub cancellation: CancellationToken,
    
    // Snapshots (for tools that modify state)
    pub snapshot_manager: Arc<SnapshotManager>,
    
    // Telemetry
    pub span: tracing::Span,
    
    // Model info (for tools that might need it)
    pub provider: String,
    pub model: String,
}

impl ToolContext {
    /// Check if execution should abort
    pub fn should_abort(&self) -> bool {
        self.cancellation.is_cancelled()
    }
    
    /// Async wait for cancellation (for select!)
    pub async fn cancelled(&self) {
        self.cancellation.cancelled().await
    }
    
    /// Take a snapshot before modifying state
    pub async fn snapshot(&self) -> Result<String, SnapshotError> {
        self.snapshot_manager.track().await
    }
    
    /// Get diff since snapshot
    pub async fn diff_since(&self, snapshot: &str) -> Result<Patch, SnapshotError> {
        self.snapshot_manager.patch(snapshot).await
    }
}
```

### ToolResult

```rust
pub struct ToolResult {
    pub status: ToolStatus,
    pub output: String,
    pub error: Option<String>,
    pub metadata: ToolMetadata,
}

pub enum ToolStatus {
    Success,
    Error,
    Cancelled,
}

pub struct ToolMetadata {
    /// Files modified (for edit/write/bash)
    pub files_modified: Vec<PathBuf>,
    
    /// Snapshot before (if state-modifying tool)
    pub snapshot_before: Option<String>,
    
    /// Diff produced (if state-modifying tool)
    pub diff: Option<Patch>,
    
    /// Execution duration
    pub duration_ms: u64,
    
    /// Custom metadata per tool
    pub extra: Value,
}
```

### Tools Own Their Side Effects

State-modifying tools (edit, write, bash, etc.) handle their own snapshots:

```rust
impl Tool for EditTool {
    async fn execute(&self, input: Value, ctx: &ToolContext) -> ToolResult {
        let args: EditArgs = serde_json::from_value(input)?;
        
        // Take snapshot before modification
        let snapshot_before = ctx.snapshot().await?;
        
        // Check cancellation
        if ctx.should_abort() {
            return ToolResult::cancelled();
        }
        
        // Do the edit
        let result = self.perform_edit(&args, ctx).await;
        
        // Compute diff
        let diff = ctx.diff_since(&snapshot_before).await?;
        
        ToolResult {
            status: ToolStatus::Success,
            output: format!("Edited {} (+{} -{} lines)", args.path, diff.additions, diff.deletions),
            error: None,
            metadata: ToolMetadata {
                files_modified: vec![args.path.into()],
                snapshot_before: Some(snapshot_before),
                diff: Some(diff),
                duration_ms: elapsed,
                extra: json!({"match_type": result.match_type}),
            },
        }
    }
}
```

### Tool Registry

```rust
pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        let mut registry = Self { tools: HashMap::new() };
        
        // File operations
        registry.register(Arc::new(ReadTool));
        registry.register(Arc::new(EditTool));
        registry.register(Arc::new(WriteTool));
        registry.register(Arc::new(BashTool));
        registry.register(Arc::new(GlobTool));
        registry.register(Arc::new(GrepTool));
        registry.register(Arc::new(ListTool));
        
        // Agent control
        registry.register(Arc::new(TaskCompleteTool));
        registry.register(Arc::new(TaskTool));  // For spawning subagents
        registry.register(Arc::new(TodoWriteTool));
        registry.register(Arc::new(TodoReadTool));
        
        // Web
        registry.register(Arc::new(WebFetchTool));
        registry.register(Arc::new(WebSearchTool));
        
        // Thinking (for extended thinking models)
        registry.register(Arc::new(ThinkTool));
        
        registry
    }
    
    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.get(name).cloned()
    }
    
    pub fn definitions(&self) -> Vec<ToolDefinition> {
        self.tools.values().map(|t| t.definition()).collect()
    }
}
```

---

## Session and History Management

### Internal Session (per Base Agent)

Each Base Agent has its own session with full history:

```rust
pub struct InternalSession {
    pub id: String,
    pub agent_name: String,
    pub history: Vec<HistoryEntry>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

pub enum HistoryEntry {
    /// User/human message
    User { 
        content: String,
        timestamp: DateTime<Utc>,
    },
    
    /// Assistant text output
    AssistantText { 
        content: String,
        timestamp: DateTime<Utc>,
    },
    
    /// Tool call (captures the real timeline)
    ToolCall {
        id: String,
        name: String,
        arguments: Value,
        timestamp: DateTime<Utc>,
    },
    
    /// Tool result (immediately follows its ToolCall)
    ToolResult {
        call_id: String,
        status: ToolStatus,
        output: String,
        metadata: ToolMetadata,
        timestamp: DateTime<Utc>,
    },
    
    /// Handoff to/from coagent
    Handoff {
        from_agent: String,
        to_agent: String,
        prompt: String,
        timestamp: DateTime<Utc>,
    },
    
    /// System events
    System {
        event: SystemEvent,
        timestamp: DateTime<Utc>,
    },
}

pub enum SystemEvent {
    SessionStart,
    Cancelled,
    DoomLoopDetected,
    MaxIterationsReached,
    TaskComplete { final_response: String },
}
```

### History Interleaving

Unlike rig's batched approach, history is **interleaved** to reflect actual execution:

```
User: "Fix the bug in auth.rs"
AssistantText: "Let me look at that file"
ToolCall: read_file { path: "auth.rs" }
ToolResult: <file contents>
AssistantText: "I see the issue, the token validation is wrong"
ToolCall: edit_file { path: "auth.rs", ... }
ToolResult: "Edited auth.rs (+3 -2 lines)"
AssistantText: "Fixed. Let me run the tests"
ToolCall: bash { command: "cargo test" }
ToolResult: "All tests passed"
AssistantText: "The bug is fixed and tests pass."
System: TaskComplete
```

### Converting to LLM Format

When calling the LLM, we convert interleaved history to the batched format APIs expect:

```rust
impl InternalSession {
    /// Convert to LLM message format (batched as APIs require)
    pub fn to_llm_messages(&self) -> Vec<LLMMessage> {
        let mut messages = vec![];
        let mut current_assistant_content = vec![];
        let mut pending_tool_results = vec![];
        
        for entry in &self.history {
            match entry {
                HistoryEntry::User { content, .. } => {
                    // Flush any pending assistant content
                    self.flush_assistant(&mut messages, &mut current_assistant_content);
                    self.flush_tool_results(&mut messages, &mut pending_tool_results);
                    
                    messages.push(LLMMessage::User { content: content.clone() });
                }
                
                HistoryEntry::AssistantText { content, .. } => {
                    current_assistant_content.push(AssistantContent::Text(content.clone()));
                }
                
                HistoryEntry::ToolCall { id, name, arguments, .. } => {
                    current_assistant_content.push(AssistantContent::ToolCall {
                        id: id.clone(),
                        name: name.clone(),
                        arguments: arguments.clone(),
                    });
                }
                
                HistoryEntry::ToolResult { call_id, output, .. } => {
                    // Flush assistant content before tool results
                    self.flush_assistant(&mut messages, &mut current_assistant_content);
                    
                    pending_tool_results.push(ToolResultMessage {
                        call_id: call_id.clone(),
                        content: output.clone(),
                    });
                }
                
                // Handoffs and system events don't go to LLM
                _ => {}
            }
        }
        
        self.flush_assistant(&mut messages, &mut current_assistant_content);
        self.flush_tool_results(&mut messages, &mut pending_tool_results);
        
        messages
    }
}
```

### ACP Session (wraps internal sessions)

```rust
pub struct ACPSession {
    pub id: String,
    pub acp_agent_name: String,
    
    /// Sessions for each involved base agent
    pub agent_sessions: HashMap<String, InternalSession>,
    
    /// Snapshot manager for undo support
    pub snapshot_manager: SnapshotManager,
    
    /// Current snapshot (for ACP-level undo)
    pub current_snapshot: Option<String>,
}
```

---

## Telemetry Architecture

Telemetry operates at **two levels** with different consumers:

### Level 1: Internal Agent Telemetry

For debugging, optimization, and understanding what actually happened:

```rust
// Span hierarchy:
// agent_turn
//   ├── llm_completion
//   │     ├── gen_ai.operation.name = "chat"
//   │     ├── gen_ai.provider.name = "anthropic"
//   │     ├── gen_ai.request.model = "claude-sonnet-4-20250514"
//   │     ├── gen_ai.usage.input_tokens = 1234
//   │     └── gen_ai.usage.output_tokens = 567
//   │
//   ├── tool_execution (per tool)
//   │     ├── tool.name = "edit_file"
//   │     ├── tool.arguments = {...}
//   │     ├── tool.result.status = "success"
//   │     ├── tool.result.duration_ms = 45
//   │     └── tool.result.files_modified = ["auth.rs"]
//   │
//   └── handoff (if coagent involved)
//         ├── handoff.from = "builder"
//         ├── handoff.to = "reviewer"  
//         └── handoff.prompt = "Review the work..."

pub trait InternalTelemetry {
    fn record_llm_call(&self, request: &LLMRequest, response: &LLMResponse);
    fn record_tool_execution(&self, call: &ToolCall, result: &ToolResult);
    fn record_handoff(&self, from: &str, to: &str, prompt: &str);
    fn record_turn_complete(&self, agent: &str, iteration: usize);
}
```

### Level 2: ACP Telemetry

For the IDE/frontend - what the user experienced:

```rust
// Span hierarchy:
// acp_prompt
//   ├── acp_agent.name = "BUILD-SMART"
//   ├── acp_session.id = "sess_123"
//   ├── acp.prompt = "Fix the bug..."
//   ├── acp.total_tokens = 5678
//   ├── acp.total_duration_ms = 12345
//   ├── acp.tool_calls_count = 4
//   └── acp.control_flow.iterations = 2

pub trait ACPTelemetry {
    fn record_prompt_start(&self, session_id: &str, prompt: &str);
    fn record_prompt_complete(&self, session_id: &str, usage: &AggregatedUsage);
    fn record_stream_event(&self, event: &ACPEvent);
}
```

### Telemetry Integration

```rust
impl BaseAgent {
    async fn run(&self, prompt: &str, ctx: &AgentContext) -> impl Stream<Item = AgentEvent> {
        let agent_span = info_span!(
            "agent_turn",
            agent.name = %self.name,
            agent.role = ?self.role,
            gen_ai.usage.input_tokens = tracing::field::Empty,
            gen_ai.usage.output_tokens = tracing::field::Empty,
        );
        
        async_stream::stream! {
            // ... react loop ...
            
            // Record LLM call
            let llm_span = info_span!(parent: &agent_span, "llm_completion");
            let response = self.completion(&history)
                .instrument(llm_span)
                .await;
            
            // Record tool execution
            for tool_call in response.tool_calls() {
                let tool_span = info_span!(
                    parent: &agent_span,
                    "tool_execution",
                    tool.name = %tool_call.name,
                );
                
                let result = self.execute_tool(&tool_call, ctx)
                    .instrument(tool_span)
                    .await;
            }
        }
        .instrument(agent_span)
    }
}
```

---

## Streaming Model

### Event Types

```rust
/// Events emitted by Base Agents
pub enum AgentEvent {
    /// Text chunk from LLM
    TextDelta(String),
    
    /// Reasoning/thinking (for extended thinking models)
    ThinkingDelta(String),
    
    /// Tool call starting
    ToolCallStart {
        id: String,
        name: String,
        arguments: Value,
    },
    
    /// Tool call completed
    ToolCallEnd {
        id: String,
        result: ToolResult,
    },
    
    /// React iteration complete (but more may come)
    IterationComplete { iteration: usize },
    
    /// Agent's turn is complete
    TurnComplete,
    
    /// Handoff to another agent
    HandoffTo { agent: String, prompt: String },
    
    /// Errors
    Error(AgentError),
    
    /// Cancellation
    Cancelled,
    
    /// Doom loop detected
    DoomLoopDetected,
}

/// Events emitted to ACP
pub enum ACPEvent {
    /// Maps to SessionUpdate::AgentMessageChunk
    MessageChunk(ContentChunk),
    
    /// Maps to SessionUpdate::AgentThoughtChunk
    ThoughtChunk(ContentChunk),
    
    /// Maps to SessionUpdate::ToolCall (in progress)
    ToolCallStart(ToolCallInfo),
    
    /// Maps to SessionUpdate::ToolCall (completed)
    ToolCallEnd(ToolCallInfo),
    
    /// Maps to SessionUpdate::Plan (from todo_write)
    Plan(PlanUpdate),
    
    /// Prompt complete, return to human
    TaskComplete,
    
    /// Error
    Error(String),
}
```

### Stream Translation

ACP Agent translates internal events to ACP events:

```rust
impl ACPAgent {
    fn to_acp_event(&self, event: AgentEvent) -> Option<ACPEvent> {
        match event {
            AgentEvent::TextDelta(text) => Some(ACPEvent::MessageChunk(
                ContentChunk::new(ContentBlock::Text(TextContent::new(text)))
            )),
            
            AgentEvent::ThinkingDelta(text) => Some(ACPEvent::ThoughtChunk(
                ContentChunk::new(ContentBlock::Text(TextContent::new(text)))
            )),
            
            AgentEvent::ToolCallStart { id, name, arguments } => Some(ACPEvent::ToolCallStart(
                ToolCallInfo {
                    id: id.into(),
                    name,
                    arguments,
                    status: ToolCallStatus::InProgress,
                    result: None,
                }
            )),
            
            AgentEvent::ToolCallEnd { id, result } => Some(ACPEvent::ToolCallEnd(
                ToolCallInfo {
                    id: id.into(),
                    status: if result.status == ToolStatus::Success {
                        ToolCallStatus::Completed
                    } else {
                        ToolCallStatus::Error
                    },
                    result: Some(result.output),
                    ..
                }
            )),
            
            // Handoffs are internal, don't surface to ACP
            AgentEvent::HandoffTo { .. } => None,
            
            // Internal iterations don't surface
            AgentEvent::IterationComplete { .. } => None,
            
            AgentEvent::TurnComplete => None,  // Handled by control flow
            
            AgentEvent::Cancelled => Some(ACPEvent::TaskComplete),  // End the ACP turn
            
            AgentEvent::Error(e) => Some(ACPEvent::Error(e.to_string())),
            
            AgentEvent::DoomLoopDetected => Some(ACPEvent::Error(
                "Agent detected repetitive behavior and stopped".into()
            )),
        }
    }
}
```

---

## Configuration System

Configuration lives in XDG directories, following opencode/crow-old patterns.

### Directory Structure

```
~/.config/crow-agent/
├── config.toml           # Global configuration
├── agents/
│   ├── base/
│   │   ├── builder.toml
│   │   ├── reviewer.toml
│   │   ├── planner.toml
│   │   └── ...
│   └── acp/
│       ├── BUILD-HITL.toml
│       ├── BUILD-SMART.toml
│       ├── BUILD-JUDGE.toml
│       ├── PLAN.toml
│       └── ...
├── prompts/
│   ├── system/
│   │   ├── builder.md
│   │   ├── reviewer.md
│   │   └── ...
│   └── handoff/
│       ├── builder-to-reviewer.md
│       └── ...
└── providers/
    ├── anthropic.toml
    ├── openai.toml
    └── ...

~/.local/share/crow-agent/
├── sessions/             # Persisted sessions
├── snapshots/            # Git-based snapshots per project
└── telemetry/            # Local telemetry data
```

### Base Agent Configuration

```toml
# ~/.config/crow-agent/agents/base/builder.toml
[agent]
name = "builder"
role = "primary"  # or "subagent"

[model]
provider = "anthropic"
model = "claude-sonnet-4-20250514"
temperature = 0.7
max_tokens = 8192

[tools]
enabled = [
    "read_file",
    "edit_file",
    "write_file",
    "bash",
    "glob",
    "grep",
    "list_directory",
    "web_fetch",
    "web_search",
    "todo_write",
    "todo_read",
    "think",
]

[react]
max_iterations = 20
doom_loop_threshold = 3

[prompt]
# Can be inline or file reference
system = { file = "prompts/system/builder.md" }
```

### ACP Agent Configuration

```toml
# ~/.config/crow-agent/agents/acp/BUILD-SMART.toml
[agent]
name = "BUILD-SMART"
description = "Builder with smart review - reviewer can evaluate and fix issues"

[composition]
primary = "builder"
coagent = "reviewer"

[control_flow]
type = "smart"
# For Loop type: max_iterations = 3
# For Chat type: max_turns = 10

[handoff]
type = "template"  # or "static" or "generated"
template = """
Review the work completed above.

Current task state:
{{todo_state}}

You may:
1. Call task_complete if the work is acceptable
2. Fix any issues yourself using the available tools
3. Provide specific feedback for another iteration

Focus on correctness, completeness, and code quality.
"""
```

### Provider Configuration

```toml
# ~/.config/crow-agent/providers/anthropic.toml
[provider]
name = "anthropic"
type = "anthropic"

[auth]
# Can be env var reference or direct (not recommended)
api_key = { env = "ANTHROPIC_API_KEY" }

[defaults]
model = "claude-sonnet-4-20250514"
max_tokens = 8192

[models.claude-sonnet-4-20250514]
context_window = 200000
supports_tools = true
supports_vision = true

[models.claude-opus-4-20250514]
context_window = 200000
supports_tools = true
supports_vision = true
supports_extended_thinking = true
```

### CLI Configuration Commands

For settings that can't be modified via ACP:

```bash
# Set default model for an agent
crow-agent config set agents.base.builder.model.model claude-opus-4-20250514

# Set react loop parameters
crow-agent config set agents.base.builder.react.max_iterations 30

# Set control flow iterations
crow-agent config set agents.acp.BUILD-LOOP.control_flow.max_iterations 5

# List available agents
crow-agent agents list

# Show agent configuration
crow-agent agents show BUILD-SMART

# Validate configuration
crow-agent config validate
```

---

## Migration Path

### Phase 1: Foundation

1. **Define core traits**: `Tool`, `ToolContext`, `ToolResult`
2. **Port tools from current crow-agent**: Preserve functionality, add context support
3. **Implement `InternalSession`**: Interleaved history model
4. **Implement `SnapshotManager`**: (Already exists, integrate with `ToolContext`)

### Phase 2: Base Agent

1. **Implement `BaseAgent`**: React loop, streaming, telemetry
2. **Port from crow-old**: Executor loop logic, doom loop detection
3. **Integrate tools via `ToolContext`**
4. **Test single-agent scenarios**

### Phase 3: ACP Agent

1. **Implement `ACPAgent`**: Composition, control flow patterns
2. **Implement control flows**: Start with HITL, then Smart
3. **Event translation**: `AgentEvent` → `ACPEvent`
4. **Preserve ACP interface**: Should be drop-in replacement

### Phase 4: Configuration

1. **Implement config loading**: XDG directories, TOML parsing
2. **Agent discovery**: Load base agents, compose ACP agents
3. **CLI commands**: Config management
4. **Migration tool**: Convert existing sessions if needed

### Phase 5: Advanced Patterns

1. **Task tool integration**: Subagent spawning with coagent support
2. **Chat control flow**: Multi-turn agent conversations
3. **Generated handoff prompts**: Dynamic prompt generation
4. **Telemetry export**: OpenTelemetry integration

---

## Future Considerations

### Swarm Patterns

The current design focuses on pairs (primary + coagent). Future work might include:

- **N-way coordination**: More than two agents collaborating
- **Specialist routing**: Route subtasks to specialized agents
- **Consensus mechanisms**: Multiple agents must agree

The two-agent pattern is foundational - swarms can be built as compositions of pairs.

### Long-Running Subagents with Coagents

From the roadmap: "subagent versions of PAIR PROGRAM that very long running agents call"

This is supported by the current design:
- Task tool spawns a subagent
- That subagent has its own coagent
- They run their Smart/Chat control flow
- Return consolidated result to parent

### IDE Integration (Crow/Zed Fork)

The ACP interface is designed to be IDE-agnostic. For deeper Zed integration:
- Agent could access LSP information
- Editor state could influence prompts
- File watching could trigger agent actions

These are additive - the core architecture doesn't change.

### Model-Specific Optimizations

Different models have different strengths:
- Extended thinking models for complex reasoning
- Fast models for simple coagent checks
- Vision models when images involved

The per-agent model configuration supports this. Future work might include:
- Automatic model selection based on task
- Fallback chains when models fail
- Cost optimization routing

---

## Summary

The crow-agent rewrite introduces a clean two-level architecture:

1. **Base Agents**: Self-contained react loops with tools, sessions, and telemetry
2. **ACP Agents**: Compositions of base agents with control flow patterns

Key principles:
- **Multiple internal agents → Single ACP agent**
- **Tools own their side effects** (snapshots, cancellation)
- **History is interleaved** (reflects real timeline)
- **Control flow is configurable** (HITL through full pair programming)
- **Telemetry at both levels** (internal debugging + external metrics)
- **ACP is a dumb pipe** (just translates events)

This architecture supports the Level 5 vision: fully capable agent pairs where the coagent can do work, not just judge, while maintaining a simple interface to the IDE/frontend.
