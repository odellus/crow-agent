//! Web search tool using SearXNG
//!
//! Ported from tools/web_search.rs

use crate::tool::{Tool, ToolContext, ToolDefinition, ToolResult};
use async_trait::async_trait;
use reqwest::Client;
use serde::Deserialize;
use serde_json::json;

#[derive(Debug, Deserialize)]
struct Args {
    query: String,
    #[serde(default = "default_limit")]
    limit: usize,
}

fn default_limit() -> usize {
    5
}

pub struct WebSearchTool {
    client: Client,
    searxng_url: String,
}

impl WebSearchTool {
    pub fn new() -> Self {
        let searxng_url = std::env::var("SEARXNG_URL")
            .unwrap_or_else(|_| "http://localhost:8082".to_string());
        Self {
            client: Client::new(),
            searxng_url,
        }
    }

    pub fn with_url(url: &str) -> Self {
        Self {
            client: Client::new(),
            searxng_url: url.to_string(),
        }
    }
}

impl Default for WebSearchTool {
    fn default() -> Self {
        Self::new()
    }
}

// SearXNG response types
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct SearxResponse {
    query: String,
    number_of_results: i64,
    results: Vec<SearchResult>,
    #[serde(default)]
    infoboxes: Vec<Infobox>,
}

#[derive(Debug, Deserialize)]
struct SearchResult {
    url: String,
    title: String,
    content: String,
}

#[derive(Debug, Deserialize)]
struct Infobox {
    infobox: String,
    id: String,
    content: String,
}

#[async_trait]
impl Tool for WebSearchTool {
    fn name(&self) -> &str {
        "web_search"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "web_search".to_string(),
            description: r#"Search the web using SearXNG.
Use for real-time information, facts, or current data.
Requires SEARXNG_URL environment variable (default: http://localhost:8082)"#.to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "The search query"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Maximum results (default: 5)"
                    }
                },
                "required": ["query"]
            }),
        }
    }

    async fn execute(&self, args_value: serde_json::Value, ctx: &ToolContext) -> ToolResult {
        if ctx.is_cancelled() {
            return ToolResult::error("Cancelled");
        }

        let args: Args = match serde_json::from_value(args_value) {
            Ok(a) => a,
            Err(e) => return ToolResult::error(format!("Invalid arguments: {}", e)),
        };

        let response = match self
            .client
            .get(format!("{}/search", self.searxng_url))
            .query(&[("q", &args.query), ("format", &"json".to_string())])
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => return ToolResult::error(format!("Search request failed: {}", e)),
        };

        if !response.status().is_success() {
            return ToolResult::error(format!("Search failed: {}", response.status()));
        }

        let data: SearxResponse = match response.json().await {
            Ok(d) => d,
            Err(e) => return ToolResult::error(format!("Failed to parse response: {}", e)),
        };

        let mut text = String::new();

        // Add infoboxes first
        for infobox in &data.infoboxes {
            text.push_str(&format!("## Infobox: {}\n", infobox.infobox));
            text.push_str(&format!("ID: {}\n", infobox.id));
            text.push_str(&format!("{}\n\n", infobox.content));
        }

        if data.results.is_empty() {
            text.push_str("No results found.\n");
        } else {
            for (i, result) in data.results.iter().enumerate() {
                if i >= args.limit {
                    break;
                }
                text.push_str(&format!("### {}\n", result.title));
                text.push_str(&format!("URL: {}\n", result.url));
                text.push_str(&format!("{}\n\n", result.content));
            }
        }

        ToolResult::success(text)
    }
}
