//! Web search tool using SearXNG

use reqwest::Client;
use rig::tool::Tool;
use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
pub enum WebSearchError {
    #[error("HTTP request failed: {0}")]
    Request(#[from] reqwest::Error),
    #[error("Search failed: {0}")]
    SearchFailed(String),
}

/// Search the web using SearXNG
#[derive(Debug, Serialize, Deserialize)]
pub struct WebSearchInput {
    /// The search query
    pub query: String,
    /// Maximum number of results (default: 5)
    #[serde(default = "default_limit")]
    pub limit: usize,
}

fn default_limit() -> usize {
    5
}

#[derive(Clone)]
pub struct WebSearch {
    client: Client,
    searxng_url: String,
}

impl WebSearch {
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

impl Default for WebSearch {
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

impl Tool for WebSearch {
    const NAME: &'static str = "web_search";

    type Error = WebSearchError;
    type Args = WebSearchInput;
    type Output = String;

    async fn definition(&self, _prompt: String) -> rig::completion::ToolDefinition {
        rig::completion::ToolDefinition {
            name: Self::NAME.to_string(),
            description: r#"Search the web for information using SearXNG.
Use this when you need real-time information, facts, or data.
Results include snippets and links from relevant web pages.

Requires SEARXNG_URL environment variable (default: http://localhost:8082)"#.to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "The search query"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Maximum number of results (default: 5)",
                        "default": 5
                    }
                },
                "required": ["query"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let response = self
            .client
            .get(format!("{}/search", self.searxng_url))
            .query(&[("q", &args.query), ("format", &"json".to_string())])
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(WebSearchError::SearchFailed(response.status().to_string()));
        }

        let data: SearxResponse = response.json().await?;

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

        Ok(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_url() {
        let search = WebSearch::new();
        assert!(search.searxng_url.contains("localhost") || search.searxng_url.contains("8082"));
    }

    #[test]
    fn test_custom_url() {
        let search = WebSearch::with_url("http://example.com:9000");
        assert_eq!(search.searxng_url, "http://example.com:9000");
    }
}
