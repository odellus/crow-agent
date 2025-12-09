//! Crow Agent CLI (new agent system)
//!
//! Run with: cargo run --bin crow

use anyhow::Result;
use clap::{Parser, Subcommand};
use crow_agent::{
    agent::{AgentConfig, BaseAgent},
    events::{AgentEvent, TurnCompleteReason},
    provider::{ProviderClient, ProviderConfig},
    tools2, Telemetry,
};
use rustyline::error::ReadlineError;
use rustyline::DefaultEditor;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// Global cancellation token for signal handling
static SHUTDOWN: std::sync::OnceLock<CancellationToken> = std::sync::OnceLock::new();

fn get_shutdown_token() -> CancellationToken {
    SHUTDOWN.get_or_init(CancellationToken::new).clone()
}

/// Get the default data directory
fn default_data_dir() -> PathBuf {
    dirs::data_dir()
        .map(|d| d.join("crow"))
        .unwrap_or_else(|| PathBuf::from(".crow"))
}

#[derive(Parser)]
#[command(name = "crow")]
#[command(about = "Crow Agent - A standalone LLM agent with tools", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// Working directory for the agent
    #[arg(short = 'd', long, default_value = ".")]
    working_dir: PathBuf,

    /// LLM model to use
    #[arg(short, long, default_value = "qwen3-30b-a3b")]
    model: String,

    /// Provider name (for auth.json lookup)
    #[arg(short, long, default_value = "lm-studio")]
    provider: String,

    /// Base URL override (skips auth.json lookup)
    #[arg(long)]
    base_url: Option<String>,

    /// Verbose output (show thinking, usage)
    #[arg(short, long)]
    verbose: bool,
}

#[derive(Subcommand)]
enum Commands {
    /// Start an interactive REPL session
    Repl,

    /// Run a single prompt
    Prompt {
        /// The prompt to send to the agent
        message: String,
    },

    /// Show session statistics
    Stats {
        /// Number of recent sessions to show
        #[arg(short = 'n', long, default_value = "10")]
        limit: usize,
    },

    /// Show tool usage statistics
    Tools,

    /// Query the telemetry database with SQL
    Query {
        /// SQL query to run
        sql: String,
    },

    /// Show recent traces (full LLM calls)
    Traces {
        /// Number of traces to show
        #[arg(short = 'n', long, default_value = "10")]
        limit: usize,

        /// Output as JSON
        #[arg(short, long)]
        json: bool,
    },

    /// Show a specific trace's full details
    Trace {
        /// Trace ID (or prefix)
        id: String,

        /// Show full content (don't truncate)
        #[arg(short, long)]
        full: bool,

        /// Output as JSON
        #[arg(short, long)]
        json: bool,
    },

    /// Replay a session - show what happened and optionally continue
    Replay {
        /// Session ID (or prefix)
        session: String,

        /// Continue the session after replay
        #[arg(short, long)]
        continue_session: bool,
    },
}

struct CrowCli {
    agent: BaseAgent,
    registry: crow_agent::tool::ToolRegistry,
    tools: Vec<async_openai::types::ChatCompletionTool>,
    telemetry: Arc<Telemetry>,
    verbose: bool,
    data_dir: PathBuf,
    model: String,
}

impl CrowCli {
    fn new(
        provider: Arc<ProviderClient>,
        working_dir: PathBuf,
        telemetry: Arc<Telemetry>,
        verbose: bool,
        data_dir: PathBuf,
        model: String,
    ) -> Self {
        let config = AgentConfig::new("crow");
        let agent = BaseAgent::with_telemetry(config, provider, working_dir.clone(), telemetry.clone());
        let registry = tools2::create_registry(working_dir);
        let tools = registry.to_openai_tools();

        Self {
            agent,
            registry,
            tools,
            telemetry,
            verbose,
            data_dir,
            model,
        }
    }

    fn system_prompt(&self) -> String {
        r#"You are Crow, a helpful software engineering assistant.

You have access to tools to help accomplish tasks. When you're done with a task, call task_complete with a summary.

Be concise and direct. Focus on solving the user's problem efficiently."#
            .to_string()
    }

    async fn run_turn(
        &self,
        messages: &mut Vec<async_openai::types::ChatCompletionRequestMessage>,
        cancellation: CancellationToken,
    ) -> Result<TurnCompleteReason> {
        let (tx, mut rx) = mpsc::unbounded_channel();

        let tools = self.tools.clone();
        let registry = self.registry.clone();
        let verbose = self.verbose;
        let telemetry = self.telemetry.clone();

        // Run agent and event processing concurrently
        let agent_fut = self
            .agent
            .execute_turn(messages, &tools, &registry, &tx, cancellation);

        let event_fut = async {
            while let Some(event) = rx.recv().await {
                match &event {
                    AgentEvent::TextDelta { delta, .. } => {
                        print!("{}", delta);
                        std::io::stdout().flush().ok();
                    }
                    AgentEvent::ThinkingDelta { delta, .. } => {
                        if verbose {
                            print!("\x1b[90m{}\x1b[0m", delta);
                            std::io::stdout().flush().ok();
                        }
                    }
                    AgentEvent::ToolCallStart { tool, arguments, .. } => {
                        let args_preview = arguments.to_string();
                        let args_short = if args_preview.len() > 60 {
                            format!("{}...", &args_preview[..60])
                        } else {
                            args_preview
                        };
                        println!("\n\x1b[33m▶ {}: {}\x1b[0m", tool, args_short);
                    }
                    AgentEvent::ToolCallEnd {
                        tool,
                        output,
                        duration_ms,
                        is_error,
                        arguments,
                        ..
                    } => {
                        // Log to telemetry
                        let result = if *is_error {
                            Err(output.as_str())
                        } else {
                            Ok(output.as_str())
                        };
                        telemetry
                            .log_tool_call(tool, arguments, result, *duration_ms)
                            .await;

                        // Print to console
                        if *is_error {
                            println!("\x1b[31m✗ {} ({}ms): {}\x1b[0m", tool, duration_ms, output);
                        } else {
                            let preview = if output.len() > 100 {
                                format!("{}...", &output[..100])
                            } else {
                                output.clone()
                            };
                            println!("\x1b[32m✓ {} ({}ms): {}\x1b[0m", tool, duration_ms, preview);
                        }
                    }
                    AgentEvent::TurnComplete { .. } => {
                        println!();
                    }
                    AgentEvent::Usage {
                        input_tokens,
                        output_tokens,
                        reasoning_tokens,
                        ..
                    } => {
                        if verbose {
                            let reasoning = reasoning_tokens
                                .map(|r| format!(", {} reasoning", r))
                                .unwrap_or_default();
                            println!(
                                "\x1b[90m[{} in, {} out{}]\x1b[0m",
                                input_tokens, output_tokens, reasoning
                            );
                        }
                    }
                    AgentEvent::Error { error, .. } => {
                        eprintln!("\x1b[31mError: {}\x1b[0m", error);
                        telemetry.log_error("agent", error).await;
                    }
                    _ => {}
                }
            }
        };

        // Run both concurrently - agent produces events, we consume them
        let (result, _) = tokio::join!(agent_fut, event_fut);
        let result = result.map_err(|e| anyhow::anyhow!(e))?;
        Ok(result.reason)
    }

    async fn chat(&self, message: &str) -> Result<()> {
        // Log user message
        self.telemetry.log_user_message(message).await;

        let mut messages = vec![
            async_openai::types::ChatCompletionRequestSystemMessageArgs::default()
                .content(self.system_prompt())
                .build()?
                .into(),
            async_openai::types::ChatCompletionRequestUserMessageArgs::default()
                .content(message)
                .build()?
                .into(),
        ];

        let cancellation = get_shutdown_token();
        let start = std::time::Instant::now();

        loop {
            let reason = self.run_turn(&mut messages, cancellation.clone()).await?;

            match reason {
                TurnCompleteReason::TaskComplete { ref summary } => {
                    let duration = start.elapsed().as_millis() as u64;
                    self.telemetry
                        .log_response(summary, None, duration, Some(&self.model), None)
                        .await;
                    println!("\x1b[36m✓ Task complete: {}\x1b[0m", summary);
                    break;
                }
                TurnCompleteReason::TextResponse => {
                    break;
                }
                TurnCompleteReason::MaxIterations => {
                    println!("\x1b[33m⚠ Max iterations reached\x1b[0m");
                    break;
                }
                TurnCompleteReason::Cancelled => {
                    println!("\x1b[33m⚠ Cancelled\x1b[0m");
                    break;
                }
            }
        }

        Ok(())
    }

    async fn run_repl(&self) -> Result<()> {
        println!("Crow Agent REPL");
        println!("Working directory: {}", self.agent.working_dir().display());
        println!("Session: {}", self.telemetry.session_id());
        println!("Database: {}", self.telemetry.db_path().display());
        println!();
        println!("Commands: /quit, /clear, /stats, /help");
        println!();

        let mut rl = DefaultEditor::new()?;
        let history_path = self.data_dir.join("history.txt");
        let _ = rl.load_history(&history_path);

        let mut messages: Vec<async_openai::types::ChatCompletionRequestMessage> = vec![
            async_openai::types::ChatCompletionRequestSystemMessageArgs::default()
                .content(self.system_prompt())
                .build()?
                .into(),
        ];

        loop {
            let prompt = if messages.len() <= 1 {
                "crow> "
            } else {
                "crow>> "
            };

            match rl.readline(prompt) {
                Ok(line) => {
                    let line = line.trim();
                    if line.is_empty() {
                        continue;
                    }

                    rl.add_history_entry(line)?;

                    // Handle commands
                    match line {
                        "/quit" | "/exit" => {
                            println!("Goodbye!");
                            break;
                        }
                        "/clear" => {
                            messages.truncate(1);
                            println!("History cleared.");
                            continue;
                        }
                        "/stats" => {
                            let stats = self.telemetry.stats();
                            println!("{}", stats);
                            continue;
                        }
                        "/help" => {
                            println!("Commands:");
                            println!("  /quit, /exit  - Exit");
                            println!("  /clear        - Clear chat history");
                            println!("  /stats        - Show session stats");
                            println!("  /help         - Show this");
                            continue;
                        }
                        _ if line.starts_with('/') => {
                            println!("Unknown command: {}", line);
                            continue;
                        }
                        _ => {}
                    }

                    // Log and add user message
                    self.telemetry.log_user_message(line).await;
                    messages.push(
                        async_openai::types::ChatCompletionRequestUserMessageArgs::default()
                            .content(line)
                            .build()?
                            .into(),
                    );

                    let cancellation = get_shutdown_token();

                    match self.run_turn(&mut messages, cancellation).await {
                        Ok(reason) => {
                            match reason {
                                TurnCompleteReason::TaskComplete { summary } => {
                                    println!("\x1b[36m✓ {}\x1b[0m", summary);
                                }
                                TurnCompleteReason::Cancelled => {
                                    println!("\x1b[33m⚠ Cancelled\x1b[0m");
                                }
                                _ => {}
                            }
                        }
                        Err(e) => {
                            eprintln!("Error: {}", e);
                            self.telemetry.log_error("repl", &e.to_string()).await;
                        }
                    }
                    println!();
                }
                Err(ReadlineError::Interrupted) => {
                    // Ctrl+C with no active turn - just show prompt again
                    println!("^C");
                    continue;
                }
                Err(ReadlineError::Eof) => {
                    println!("Goodbye!");
                    break;
                }
                Err(e) => {
                    eprintln!("Error: {:?}", e);
                    break;
                }
            }
        }

        let _ = rl.save_history(&history_path);

        // Print final stats
        let stats = self.telemetry.stats();
        println!("\nSession: {}", stats);

        Ok(())
    }
}

fn build_provider(cli: &Cli) -> Result<Arc<ProviderClient>> {
    // If base_url is provided, use it directly
    if let Some(ref base_url) = cli.base_url {
        let config = ProviderConfig::custom(&cli.provider, base_url, "UNUSED_API_KEY_ENV", &cli.model);
        return Ok(Arc::new(
            ProviderClient::new(config).map_err(|e| anyhow::anyhow!(e))?,
        ));
    }

    // Otherwise, try to get base_url from auth.json
    let auth_path = default_data_dir().join("auth.json");
    if let Ok(content) = std::fs::read_to_string(&auth_path) {
        if let Ok(auth) = serde_json::from_str::<serde_json::Value>(&content) {
            if let Some(entry) = auth.get(&cli.provider) {
                if let Some(base_url) = entry.get("base_url").and_then(|v| v.as_str()) {
                    let config =
                        ProviderConfig::custom(&cli.provider, base_url, "UNUSED", &cli.model);
                    return Ok(Arc::new(
                        ProviderClient::new(config).map_err(|e| anyhow::anyhow!(e))?,
                    ));
                }
            }
        }
    }

    // Fall back to common providers
    let config = match cli.provider.as_str() {
        "openrouter" => ProviderConfig::openrouter(),
        "openai" => ProviderConfig::openai(),
        "anthropic" => ProviderConfig::anthropic(),
        "moonshot" => ProviderConfig::moonshot(),
        _ => {
            anyhow::bail!(
                "Provider '{}' not found in auth.json and no base_url provided",
                cli.provider
            );
        }
    };

    Ok(Arc::new(
        ProviderClient::new(config).map_err(|e| anyhow::anyhow!(e))?,
    ))
}

fn show_stats(telemetry: &Telemetry, limit: usize) -> Result<()> {
    println!("Database: {}\n", telemetry.db_path().display());

    let sessions = telemetry.recent_sessions(limit)?;
    if sessions.is_empty() {
        println!("No sessions found.");
        return Ok(());
    }

    println!("Recent Sessions:");
    println!("{:-<80}", "");
    for s in sessions {
        println!(
            "  {} | {} | {} interactions | {} tokens",
            &s.id[..8],
            s.started_at,
            s.interaction_count,
            s.total_tokens
        );
        if let Some(ref model) = s.model {
            println!("    Model: {}", model);
        }
    }
    Ok(())
}

fn show_tools(telemetry: &Telemetry) -> Result<()> {
    let tools = telemetry.tool_stats()?;
    if tools.is_empty() {
        println!("No tool calls recorded.");
        return Ok(());
    }

    println!("Tool Usage:");
    println!("{:-<80}", "");
    for t in tools {
        let success_rate = if t.call_count > 0 {
            (t.success_count as f64 / t.call_count as f64) * 100.0
        } else {
            0.0
        };
        println!(
            "  {:20} | {:5} calls | {:6.1}ms avg | {:.0}% success",
            t.tool_name, t.call_count, t.avg_duration_ms, success_rate
        );
    }
    Ok(())
}

fn run_query(telemetry: &Telemetry, sql: &str) -> Result<()> {
    use rusqlite::Connection;

    let conn = Connection::open(telemetry.db_path())?;
    let mut stmt = conn.prepare(sql)?;
    let column_count = stmt.column_count();
    let column_names: Vec<String> = stmt.column_names().iter().map(|s| s.to_string()).collect();

    println!("{}", column_names.join(" | "));
    println!("{:-<80}", "");

    let rows = stmt.query_map([], |row| {
        let mut values = Vec::new();
        for i in 0..column_count {
            let value: String = row
                .get::<_, rusqlite::types::Value>(i)
                .map(|v| match v {
                    rusqlite::types::Value::Null => "NULL".to_string(),
                    rusqlite::types::Value::Integer(i) => i.to_string(),
                    rusqlite::types::Value::Real(f) => format!("{:.2}", f),
                    rusqlite::types::Value::Text(s) => {
                        if s.len() > 50 {
                            format!("{}...", &s[..50])
                        } else {
                            s
                        }
                    }
                    rusqlite::types::Value::Blob(_) => "[BLOB]".to_string(),
                })
                .unwrap_or_else(|_| "?".to_string());
            values.push(value);
        }
        Ok(values.join(" | "))
    })?;

    for row in rows {
        println!("{}", row?);
    }
    Ok(())
}

async fn replay_session(telemetry: &Telemetry, session_prefix: &str, _continue_session: bool) -> Result<()> {
    use rusqlite::Connection;

    let conn = Connection::open(telemetry.db_path())?;

    // Find session matching prefix
    let session_id: String = conn.query_row(
        "SELECT DISTINCT session_id FROM traces WHERE session_id LIKE ?1 ORDER BY started_at DESC LIMIT 1",
        [format!("{}%", session_prefix)],
        |row| row.get(0),
    ).map_err(|_| anyhow::anyhow!("Session not found: {}", session_prefix))?;

    println!("Replaying session: {}", session_id);
    println!("{:=<80}", "");
    println!();

    // Get all traces for this session in order
    let mut stmt = conn.prepare(
        r#"SELECT started_at, latency_ms, request_messages, response_content, response_tool_calls, error
           FROM traces
           WHERE session_id = ?1
           ORDER BY started_at ASC"#
    )?;

    let rows = stmt.query_map([&session_id], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, Option<i64>>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, Option<String>>(3)?,
            row.get::<_, Option<String>>(4)?,
            row.get::<_, Option<String>>(5)?,
        ))
    })?;

    for (i, row) in rows.enumerate() {
        let (started_at, latency_ms, request_messages, response_content, tool_calls, error) = row?;

        let time = started_at
            .split('T')
            .nth(1)
            .and_then(|t| t.split('.').next())
            .unwrap_or(&started_at);

        let latency = latency_ms
            .map(|ms| format!("{}ms", ms))
            .unwrap_or_else(|| "?".to_string());

        println!("\x1b[36m--- Turn {} ({} @ {}) ---\x1b[0m", i + 1, latency, time);

        // Parse and show the last user message from request_messages
        if let Ok(messages) = serde_json::from_str::<Vec<serde_json::Value>>(&request_messages) {
            // Find last user message
            for msg in messages.iter().rev() {
                if msg.get("role").and_then(|r| r.as_str()) == Some("user") {
                    if let Some(content) = msg.get("content").and_then(|c| c.as_str()) {
                        println!("\x1b[33mUser:\x1b[0m {}", content);
                        break;
                    }
                }
            }
        }

        // Show response
        if let Some(ref content) = response_content {
            // Check if there's thinking
            if content.starts_with("<thinking>") {
                if let Some(end) = content.find("</thinking>") {
                    let thinking = &content[10..end].trim();
                    let rest = content[end + 11..].trim();
                    println!("\x1b[90mThinking: {}...\x1b[0m", &thinking[..thinking.len().min(100)]);
                    if !rest.is_empty() {
                        println!("\x1b[32mAssistant:\x1b[0m {}", rest);
                    }
                } else {
                    println!("\x1b[90mThinking: {}\x1b[0m", content);
                }
            } else {
                println!("\x1b[32mAssistant:\x1b[0m {}", content);
            }
        }

        // Show tool calls
        if let Some(ref tc) = tool_calls {
            if let Ok(calls) = serde_json::from_str::<Vec<serde_json::Value>>(tc) {
                for call in calls {
                    let name = call.get("name").and_then(|n| n.as_str()).unwrap_or("?");
                    let args = call.get("arguments").and_then(|a| a.as_str()).unwrap_or("{}");
                    let args_short = if args.len() > 50 {
                        format!("{}...", &args[..50])
                    } else {
                        args.to_string()
                    };
                    println!("\x1b[35mTool:\x1b[0m {} {}", name, args_short);
                }
            }
        }

        // Show error if any
        if let Some(ref err) = error {
            println!("\x1b[31mError:\x1b[0m {}", err);
        }

        println!();
    }

    println!("{:=<80}", "");
    println!("End of session {}", &session_id[..8]);

    // TODO: if continue_session, reconstruct messages and start REPL

    Ok(())
}

fn show_traces(telemetry: &Telemetry, limit: usize) -> Result<()> {
    use rusqlite::Connection;

    let conn = Connection::open(telemetry.db_path())?;
    let mut stmt = conn.prepare(&format!(
        r#"SELECT id, session_id, started_at, latency_ms, model_id,
                  substr(response_content, 1, 60) as preview
           FROM traces
           ORDER BY started_at DESC
           LIMIT {}"#,
        limit
    ))?;

    println!("Recent Traces:");
    println!("{:-<100}", "");

    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, Option<i64>>(3)?,
            row.get::<_, Option<String>>(4)?,
            row.get::<_, Option<String>>(5)?,
        ))
    })?;

    for row in rows {
        let (id, session_id, started_at, latency_ms, model, preview) = row?;
        let latency = latency_ms
            .map(|ms| format!("{}ms", ms))
            .unwrap_or_else(|| "?".to_string());
        let model = model.unwrap_or_else(|| "?".to_string());
        let preview = preview.unwrap_or_default();

        let time = started_at
            .split('T')
            .nth(1)
            .and_then(|t| t.split('.').next())
            .unwrap_or(&started_at);

        println!(
            "{} {} {:>8} [{}] {} {}",
            time,
            &id[..8],
            latency,
            &session_id[..8],
            model,
            preview
        );
    }
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    // Set up signal handling for graceful shutdown
    let shutdown = get_shutdown_token();
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        shutdown.cancel();
    });

    let cli = Cli::parse();

    // Resolve working directory
    let working_dir = if cli.working_dir.is_absolute() {
        cli.working_dir.clone()
    } else {
        std::env::current_dir()?.join(&cli.working_dir)
    }
    .canonicalize()?;

    let data_dir = default_data_dir();
    std::fs::create_dir_all(&data_dir)?;

    // Initialize telemetry
    let telemetry = Arc::new(Telemetry::init(
        data_dir.clone(),
        cli.verbose,
        None, // No OTel endpoint for now
        Some(&working_dir.display().to_string()),
        Some(&cli.model),
        Some(&cli.provider),
    )?);

    // Handle non-agent commands first (don't need provider)
    match &cli.command {
        Some(Commands::Stats { limit }) => {
            return show_stats(&telemetry, *limit);
        }
        Some(Commands::Tools) => {
            return show_tools(&telemetry);
        }
        Some(Commands::Query { sql }) => {
            return run_query(&telemetry, sql);
        }
        Some(Commands::Traces { limit }) => {
            return show_traces(&telemetry, *limit);
        }
        Some(Commands::Replay { session, continue_session }) => {
            return replay_session(&telemetry, session, *continue_session).await;
        }
        _ => {}
    }

    // Build provider for agent commands
    let provider = build_provider(&cli)?;

    if cli.verbose {
        println!("Provider: {} ({})", cli.provider, cli.model);
        println!("Session: {}", telemetry.session_id());
        println!("Database: {}", telemetry.db_path().display());
        println!();
    }

    // Create CLI
    let crow = CrowCli::new(
        provider,
        working_dir,
        telemetry.clone(),
        cli.verbose,
        data_dir,
        cli.model.clone(),
    );

    match cli.command {
        Some(Commands::Prompt { message }) => {
            crow.chat(&message).await?;
        }
        Some(Commands::Repl) | None => {
            crow.run_repl().await?;
        }
        _ => unreachable!(),
    }

    Ok(())
}
