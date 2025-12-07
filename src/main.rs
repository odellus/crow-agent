//! Crow Agent CLI
//!
//! A command-line interface for the Crow agent with REPL support.

use anyhow::Result;
use clap::{Parser, Subcommand};
use crow_agent::{AuthConfig, Config, CrowAgent, Telemetry};
use rig::message::Message;
use rustyline::error::ReadlineError;
use rustyline::DefaultEditor;
use std::path::PathBuf;
use std::sync::Arc;

/// Get the default data directory for crow-agent
/// Uses XDG_DATA_HOME if set, otherwise ~/.crow_agent
fn default_data_dir() -> PathBuf {
    if let Ok(xdg_data) = std::env::var("XDG_DATA_HOME") {
        PathBuf::from(xdg_data).join("crow_agent")
    } else if let Ok(home) = std::env::var("HOME") {
        PathBuf::from(home).join(".crow_agent")
    } else {
        PathBuf::from(".crow_agent")
    }
}

/// Build configuration from CLI args and auth.json
/// Priority: CLI flags > auth.json > environment variables
fn build_config(cli: &Cli, working_dir: PathBuf) -> Result<Config> {
    // If base_url is provided on CLI, use custom config
    if let Some(ref base_url) = cli.base_url {
        let api_key = cli.api_key.clone().unwrap_or_else(|| "lm-studio".to_string());
        return Ok(Config::lm_studio(base_url, &cli.model, working_dir)
            .with_api_key(api_key));
    }

    // Try to find provider in auth.json based on model prefix
    // e.g., "anthropic/claude-3.5-sonnet" -> look for "anthropic" provider
    let auth = AuthConfig::load().unwrap_or_default();

    // Extract provider hint from model name (before the slash)
    let provider_hint = cli.model.split('/').next().unwrap_or("");

    // Look for matching provider in auth.json
    if let Some(entry) = auth.get(provider_hint) {
        let api_key = cli.api_key.clone()
            .unwrap_or_else(|| entry.api_key().to_string());

        if let Some(base_url) = entry.base_url() {
            return Ok(Config::lm_studio(base_url, &cli.model, working_dir)
                .with_api_key(api_key));
        } else {
            // Provider without base_url - use as OpenRouter-style
            return Ok(Config::openrouter(&cli.model, working_dir)
                .with_api_key(api_key));
        }
    }

    // Also check for exact model name match or common aliases
    for provider_name in ["openrouter", "lm-studio", "lmstudio"] {
        if let Some(entry) = auth.get(provider_name) {
            let api_key = cli.api_key.clone()
                .unwrap_or_else(|| entry.api_key().to_string());

            if let Some(base_url) = entry.base_url() {
                return Ok(Config::lm_studio(base_url, &cli.model, working_dir)
                    .with_api_key(api_key));
            } else {
                return Ok(Config::openrouter(&cli.model, working_dir)
                    .with_api_key(api_key));
            }
        }
    }

    // Fall back to environment variable
    Ok(Config::openrouter(&cli.model, working_dir))
}

#[derive(Parser)]
#[command(name = "crow-agent")]
#[command(about = "Crow Agent - A standalone LLM agent with tools", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// Working directory for the agent
    #[arg(short = 'd', long, default_value = ".")]
    working_dir: PathBuf,

    /// LLM model to use
    #[arg(short, long, default_value = "glm-4.5-air@q4_k_m")]
    model: String,

    /// Base URL for custom LLM endpoint (e.g., LM Studio) - overrides auth.json
    #[arg(long)]
    base_url: Option<String>,

    /// API key - overrides auth.json and env vars
    #[arg(long)]
    api_key: Option<String>,

    /// Data directory (default: ~/.crow_agent or $XDG_DATA_HOME/crow_agent)
    #[arg(long)]
    data_dir: Option<PathBuf>,

    /// OpenTelemetry collector endpoint (e.g., http://localhost:4318)
    #[arg(long)]
    otel_endpoint: Option<String>,

    /// Verbose logging
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

    /// Run as an ACP server over stdio (for Zed integration)
    Acp,

    /// Show telemetry statistics
    Stats {
        /// Show recent sessions
        #[arg(long, default_value = "10")]
        sessions: usize,

        /// Show tool usage stats
        #[arg(long)]
        tools: bool,
    },

    /// Query the telemetry database
    Query {
        /// SQL query to run against the telemetry database
        sql: String,
    },

    /// Telemetry commands - view traces, run SQL queries
    Telemetry {
        #[command(subcommand)]
        command: TelemetryCommands,
    },
}

#[derive(Subcommand)]
enum TelemetryCommands {
    /// List recent LLM call traces
    Traces {
        /// Maximum number of traces to show
        #[arg(short = 'n', long, default_value = "20")]
        limit: usize,

        /// Filter by session ID
        #[arg(short, long)]
        session: Option<String>,

        /// Output as JSON
        #[arg(short, long)]
        json: bool,
    },

    /// Show a specific trace's full details
    Trace {
        /// Trace ID (can be partial)
        trace_id: String,

        /// Output as JSON
        #[arg(short, long)]
        json: bool,
    },

    /// Run arbitrary SQL query
    Sql {
        /// SQL query to execute
        query: String,
    },

    /// Show database schema
    Schema,

    /// Show available tables
    Tables,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Resolve working directory
    let working_dir = if cli.working_dir.is_absolute() {
        cli.working_dir.clone()
    } else {
        std::env::current_dir()?.join(&cli.working_dir)
    }
    .canonicalize()?;

    // Resolve data directory (for telemetry DB, logs, etc.)
    let data_dir = cli.data_dir.clone().unwrap_or_else(default_data_dir);

    // Ensure data directory exists
    std::fs::create_dir_all(&data_dir)?;

    // Build configuration - priority: CLI flags > auth.json > env vars
    let config = build_config(&cli, working_dir.clone())?
        .with_verbose(cli.verbose)
        .with_log_dir(data_dir.clone());

    let provider_name = config.llm.provider.as_str();

    // Initialize telemetry - use stderr for ACP mode (stdout is for JSON-RPC)
    let is_acp_mode = matches!(cli.command, Some(Commands::Acp));
    let telemetry = if is_acp_mode {
        Arc::new(Telemetry::init_for_serve(
            data_dir.clone(),
            cli.verbose,
            cli.otel_endpoint.as_deref(),
            Some(&working_dir.display().to_string()),
            Some(&cli.model),
            Some(provider_name),
        )?)
    } else {
        Arc::new(Telemetry::init(
            data_dir.clone(),
            cli.verbose,
            cli.otel_endpoint.as_deref(),
            Some(&working_dir.display().to_string()),
            Some(&cli.model),
            Some(provider_name),
        )?)
    };

    match cli.command {
        Some(Commands::Acp) => {
            // Run as ACP server - use LocalSet for !Send futures
            let local = tokio::task::LocalSet::new();
            local
                .run_until(crow_agent::run_stdio_server(config, telemetry))
                .await?;
        }
        _ => {
            // Create agent for interactive modes
            let agent = CrowAgent::new(config, telemetry.clone());

            match cli.command {
                Some(Commands::Prompt { message }) => {
                    run_single_prompt(&agent, &message).await?;
                }
                Some(Commands::Stats { sessions, tools }) => {
                    show_stats(&telemetry, sessions, tools)?;
                }
                Some(Commands::Query { sql }) => {
                    run_query(&telemetry, &sql)?;
                }
                Some(Commands::Telemetry { command }) => {
                    run_telemetry_command(&data_dir, command)?;
                }
                Some(Commands::Repl) | None => {
                    run_repl(&agent, &data_dir).await?;
                }
                Some(Commands::Acp) => unreachable!(),
            }
        }
    }

    Ok(())
}

fn show_stats(telemetry: &Telemetry, session_limit: usize, show_tools: bool) -> Result<()> {
    println!("Telemetry Database: {}\n", telemetry.db_path().display());

    // Show recent sessions
    println!("Recent Sessions (last {}):", session_limit);
    println!("{:-<80}", "");

    let sessions = telemetry.recent_sessions(session_limit)?;
    if sessions.is_empty() {
        println!("  No sessions found.");
    } else {
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
            if let Some(ref wd) = s.working_dir {
                println!("    Dir: {}", wd);
            }
        }
    }

    // Show tool stats if requested
    if show_tools {
        println!("\nTool Usage Statistics:");
        println!("{:-<80}", "");

        let tools = telemetry.tool_stats()?;
        if tools.is_empty() {
            println!("  No tool calls recorded.");
        } else {
            for t in tools {
                let success_rate = if t.call_count > 0 {
                    (t.success_count as f64 / t.call_count as f64) * 100.0
                } else {
                    0.0
                };
                println!(
                    "  {:20} | {:5} calls | {:6.1}ms avg | {:.0}% success",
                    t.tool_name,
                    t.call_count,
                    t.avg_duration_ms,
                    success_rate
                );
            }
        }
    }

    Ok(())
}

fn run_query(telemetry: &Telemetry, sql: &str) -> Result<()> {
    use rusqlite::Connection;

    let db_path = telemetry.db_path();
    let conn = Connection::open(&db_path)?;

    let mut stmt = conn.prepare(sql)?;
    let column_count = stmt.column_count();
    let column_names: Vec<String> = stmt.column_names().iter().map(|s| s.to_string()).collect();

    // Print header
    println!("{}", column_names.join(" | "));
    println!("{:-<80}", "");

    // Print rows
    let rows = stmt.query_map([], |row| {
        let mut values = Vec::new();
        for i in 0..column_count {
            let value: String = row.get::<_, rusqlite::types::Value>(i)
                .map(|v| match v {
                    rusqlite::types::Value::Null => "NULL".to_string(),
                    rusqlite::types::Value::Integer(i) => i.to_string(),
                    rusqlite::types::Value::Real(f) => format!("{:.2}", f),
                    rusqlite::types::Value::Text(s) => s,
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

async fn run_single_prompt(agent: &CrowAgent, message: &str) -> Result<()> {
    println!("Working directory: {}", agent.working_dir().display());
    println!("Database: {}", agent.telemetry().db_path().display());
    println!("---");

    match agent.prompt(message).await {
        Ok(response) => {
            println!("{}", response);
        }
        Err(e) => {
            eprintln!("Error: {}", e);
        }
    }

    // Print stats
    let stats = agent.telemetry().stats();
    println!("---");
    println!("{}", stats);

    Ok(())
}

async fn run_repl(agent: &CrowAgent, data_dir: &PathBuf) -> Result<()> {
    println!("Crow Agent REPL");
    println!("Working directory: {}", agent.working_dir().display());
    println!("Session: {}", agent.telemetry().session_id());
    println!("Database: {}", agent.telemetry().db_path().display());
    println!();
    println!("Commands:");
    println!("  /quit, /exit  - Exit the REPL");
    println!("  /clear        - Clear chat history");
    println!("  /stats        - Show session statistics");
    println!("  /tools        - Show tool usage stats");
    println!("  /sessions     - Show recent sessions");
    println!("  /query <sql>  - Run SQL query on telemetry DB");
    println!("  /help         - Show this help");
    println!();

    let mut rl = DefaultEditor::new()?;
    let history_path = data_dir.join("history.txt");

    let _ = rl.load_history(&history_path);

    let mut chat_history: Vec<Message> = Vec::new();

    loop {
        let prompt = if chat_history.is_empty() {
            "crow-agent> "
        } else {
            "crow-agent>> "
        };

        let readline = rl.readline(prompt);

        match readline {
            Ok(line) => {
                let line = line.trim();

                if line.is_empty() {
                    continue;
                }

                rl.add_history_entry(line)?;

                // Handle commands
                if line.starts_with('/') {
                    let parts: Vec<&str> = line.splitn(2, ' ').collect();
                    let cmd = parts[0];
                    let arg = parts.get(1).map(|s| *s);

                    match cmd {
                        "/quit" | "/exit" => {
                            println!("Goodbye!");
                            break;
                        }
                        "/clear" => {
                            chat_history.clear();
                            println!("Chat history cleared.");
                            continue;
                        }
                        "/stats" => {
                            let stats = agent.telemetry().stats();
                            println!("{}", stats);
                            continue;
                        }
                        "/tools" => {
                            match agent.telemetry().tool_stats() {
                                Ok(tools) => {
                                    println!("Tool Usage:");
                                    for t in tools {
                                        println!(
                                            "  {:20} | {:5} calls | {:6.1}ms avg",
                                            t.tool_name, t.call_count, t.avg_duration_ms
                                        );
                                    }
                                }
                                Err(e) => eprintln!("Error: {}", e),
                            }
                            continue;
                        }
                        "/sessions" => {
                            match agent.telemetry().recent_sessions(10) {
                                Ok(sessions) => {
                                    println!("Recent Sessions:");
                                    for s in sessions {
                                        println!(
                                            "  {} | {} | {} interactions",
                                            &s.id[..8], s.started_at, s.interaction_count
                                        );
                                    }
                                }
                                Err(e) => eprintln!("Error: {}", e),
                            }
                            continue;
                        }
                        "/query" => {
                            if let Some(sql) = arg {
                                if let Err(e) = run_query(agent.telemetry(), sql) {
                                    eprintln!("Query error: {}", e);
                                }
                            } else {
                                println!("Usage: /query <sql>");
                            }
                            continue;
                        }
                        "/help" => {
                            println!("Commands:");
                            println!("  /quit, /exit  - Exit the REPL");
                            println!("  /clear        - Clear chat history");
                            println!("  /stats        - Show session statistics");
                            println!("  /tools        - Show tool usage stats");
                            println!("  /sessions     - Show recent sessions");
                            println!("  /query <sql>  - Run SQL query on telemetry DB");
                            println!("  /help         - Show this help");
                            continue;
                        }
                        _ => {
                            println!("Unknown command: {}", cmd);
                            continue;
                        }
                    }
                }

                // Send to agent
                print!("\n");
                match agent.chat(line, &mut chat_history).await {
                    Ok(response) => {
                        println!("{}\n", response);
                    }
                    Err(e) => {
                        eprintln!("Error: {}\n", e);
                    }
                }
            }
            Err(ReadlineError::Interrupted) => {
                println!("^C");
                continue;
            }
            Err(ReadlineError::Eof) => {
                println!("Goodbye!");
                break;
            }
            Err(err) => {
                eprintln!("Error: {:?}", err);
                break;
            }
        }
    }

    let _ = rl.save_history(&history_path);

    // Final stats
    let stats = agent.telemetry().stats();
    println!("\nSession summary: {}", stats);

    Ok(())
}

fn run_telemetry_command(data_dir: &PathBuf, command: TelemetryCommands) -> Result<()> {
    use rusqlite::Connection;

    let db_path = data_dir.join("telemetry.db");
    let conn = Connection::open(&db_path)?;

    match command {
        TelemetryCommands::Tables => {
            println!("Tables in {}:\n", db_path.display());
            let mut stmt = conn.prepare(
                "SELECT name FROM sqlite_master WHERE type='table' ORDER BY name"
            )?;
            let tables: Vec<String> = stmt
                .query_map([], |row| row.get(0))?
                .filter_map(|r| r.ok())
                .collect();
            for table in tables {
                println!("  {}", table);
            }
        }

        TelemetryCommands::Schema => {
            println!("Schema for {}:\n", db_path.display());
            let mut stmt = conn.prepare(
                "SELECT sql FROM sqlite_master WHERE type='table' AND sql IS NOT NULL ORDER BY name"
            )?;
            let schemas: Vec<String> = stmt
                .query_map([], |row| row.get(0))?
                .filter_map(|r| r.ok())
                .collect();
            for schema in schemas {
                println!("{}\n", schema);
            }
        }

        TelemetryCommands::Sql { query } => {
            let mut stmt = conn.prepare(&query)?;
            let column_count = stmt.column_count();
            let column_names: Vec<String> = stmt.column_names().iter().map(|s| s.to_string()).collect();

            // Print header
            println!("{}", column_names.join(" | "));
            println!("{:-<80}", "");

            // Print rows
            let rows = stmt.query_map([], |row| {
                let mut values = Vec::new();
                for i in 0..column_count {
                    let value: String = row.get::<_, rusqlite::types::Value>(i)
                        .map(|v| match v {
                            rusqlite::types::Value::Null => "NULL".to_string(),
                            rusqlite::types::Value::Integer(i) => i.to_string(),
                            rusqlite::types::Value::Real(f) => format!("{:.2}", f),
                            rusqlite::types::Value::Text(s) => {
                                if s.len() > 60 {
                                    format!("{}...", &s[..60])
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
        }

        TelemetryCommands::Traces { limit, session, json } => {
            let query = if let Some(ref sid) = session {
                format!(
                    "SELECT id, session_id, started_at, latency_ms, model_provider,
                            substr(response_content, 1, 50) as response_preview
                     FROM traces
                     WHERE session_id LIKE '{}%'
                     ORDER BY started_at DESC LIMIT {}",
                    sid, limit
                )
            } else {
                format!(
                    "SELECT id, session_id, started_at, latency_ms, model_provider,
                            substr(response_content, 1, 50) as response_preview
                     FROM traces
                     ORDER BY started_at DESC LIMIT {}",
                    limit
                )
            };

            let mut stmt = conn.prepare(&query)?;

            if json {
                let traces: Vec<serde_json::Value> = stmt
                    .query_map([], |row| {
                        Ok(serde_json::json!({
                            "id": row.get::<_, String>(0)?,
                            "session_id": row.get::<_, String>(1)?,
                            "started_at": row.get::<_, String>(2)?,
                            "latency_ms": row.get::<_, Option<i64>>(3)?,
                            "model_provider": row.get::<_, Option<String>>(4)?,
                            "response_preview": row.get::<_, Option<String>>(5)?
                        }))
                    })?
                    .filter_map(|r| r.ok())
                    .collect();
                println!("{}", serde_json::to_string_pretty(&traces)?);
            } else {
                println!("Recent Traces");
                println!("{:-<100}", "");

                let rows: Vec<_> = stmt
                    .query_map([], |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, Option<i64>>(3)?,
                            row.get::<_, Option<String>>(4)?,
                            row.get::<_, Option<String>>(5)?
                        ))
                    })?
                    .filter_map(|r| r.ok())
                    .collect();

                for (id, session_id, started_at, latency_ms, provider, preview) in &rows {
                    let latency = latency_ms.map(|ms| format!("{}ms", ms)).unwrap_or_else(|| "?".to_string());
                    let prov = provider.as_deref().unwrap_or("unknown");
                    let prev = preview.as_deref().unwrap_or("");

                    // Parse and format timestamp
                    let time = started_at.split('T').nth(1)
                        .and_then(|t| t.split('.').next())
                        .unwrap_or(&started_at[..19]);

                    println!(
                        "{} {} {} {:>8} [{}] {}",
                        time,
                        &id[..8],
                        prov,
                        latency,
                        &session_id[..8],
                        prev
                    );
                }

                println!("\n{} traces", rows.len());
            }
        }

        TelemetryCommands::Trace { trace_id, json } => {
            let query = format!(
                "SELECT * FROM traces WHERE id LIKE '{}%' LIMIT 1",
                trace_id
            );

            let mut stmt = conn.prepare(&query)?;
            let column_names: Vec<String> = stmt.column_names().iter().map(|s| s.to_string()).collect();

            let trace: Option<Vec<(String, String)>> = stmt
                .query_map([], |row| {
                    let mut pairs = Vec::new();
                    for (i, name) in column_names.iter().enumerate() {
                        let value: String = row.get::<_, rusqlite::types::Value>(i)
                            .map(|v| match v {
                                rusqlite::types::Value::Null => "null".to_string(),
                                rusqlite::types::Value::Integer(i) => i.to_string(),
                                rusqlite::types::Value::Real(f) => f.to_string(),
                                rusqlite::types::Value::Text(s) => s,
                                rusqlite::types::Value::Blob(_) => "[BLOB]".to_string(),
                            })
                            .unwrap_or_else(|_| "?".to_string());
                        pairs.push((name.clone(), value));
                    }
                    Ok(pairs)
                })?
                .next()
                .transpose()?;

            match trace {
                Some(pairs) => {
                    if json {
                        let obj: serde_json::Map<String, serde_json::Value> = pairs
                            .into_iter()
                            .map(|(k, v)| {
                                // Try to parse JSON fields
                                let val = serde_json::from_str(&v)
                                    .unwrap_or_else(|_| serde_json::Value::String(v));
                                (k, val)
                            })
                            .collect();
                        println!("{}", serde_json::to_string_pretty(&obj)?);
                    } else {
                        println!("Trace Details");
                        println!("{:-<80}", "");
                        for (name, value) in pairs {
                            if value.len() > 100 {
                                println!("\n{}:", name);
                                println!("{}", value);
                            } else {
                                println!("{}: {}", name, value);
                            }
                        }
                    }
                }
                None => {
                    eprintln!("Trace not found: {}", trace_id);
                }
            }
        }
    }

    Ok(())
}
