//! Fetch tool - HTTP GET and convert to markdown
//!
//! Ported from tools/fetch.rs

use crate::tool::{Tool, ToolContext, ToolDefinition, ToolResult};
use async_trait::async_trait;
use reqwest::Client;
use serde::Deserialize;
use serde_json::json;

#[derive(Debug, Deserialize)]
struct Args {
    url: String,
}

pub struct FetchTool {
    client: Client,
}

impl FetchTool {
    pub fn new() -> Self {
        Self {
            client: Client::new(),
        }
    }
}

impl Default for FetchTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for FetchTool {
    fn name(&self) -> &str {
        "fetch"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "fetch".to_string(),
            description: "Fetch a URL and return content as Markdown. Useful for reading web pages, APIs, or documentation.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "url": {
                        "type": "string",
                        "description": "The URL to fetch"
                    }
                },
                "required": ["url"]
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

        let url = if !args.url.starts_with("https://") && !args.url.starts_with("http://") {
            format!("https://{}", args.url)
        } else {
            args.url
        };

        let response = match self
            .client
            .get(&url)
            .header("User-Agent", "crow-agent/0.1")
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => return ToolResult::error(format!("Request failed: {}", e)),
        };

        if response.status().is_client_error() || response.status().is_server_error() {
            return ToolResult::error(format!("HTTP error: {}", response.status()));
        }

        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("text/html")
            .to_string();

        let body = match response.text().await {
            Ok(b) => b,
            Err(e) => return ToolResult::error(format!("Failed to read response: {}", e)),
        };

        let content = if content_type.starts_with("application/json") {
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&body) {
                format!("```json\n{}\n```", serde_json::to_string_pretty(&json).unwrap_or(body))
            } else {
                format!("```json\n{}\n```", body)
            }
        } else if content_type.starts_with("text/plain") {
            body
        } else {
            html_to_markdown(&body)
        };

        if content.trim().is_empty() {
            ToolResult::error("No content found")
        } else {
            ToolResult::success(content)
        }
    }
}

/// Basic HTML to markdown conversion
fn html_to_markdown(html: &str) -> String {
    let mut result = String::new();
    let mut in_tag = false;
    let mut current_tag = String::new();
    let mut skip_content = false;
    let mut chars = html.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '<' {
            in_tag = true;
            current_tag.clear();
            continue;
        }

        if in_tag {
            if c == '>' {
                in_tag = false;
                let tag = current_tag.to_lowercase();
                let tag_name = tag.split_whitespace().next().unwrap_or("");

                if tag_name.starts_with('/') {
                    let closing = &tag_name[1..];
                    match closing {
                        "script" | "style" | "head" | "nav" | "footer" | "aside" => {
                            skip_content = false;
                        }
                        "p" | "div" | "br" | "li" | "tr" => result.push('\n'),
                        "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                            result.push_str("\n\n");
                        }
                        "code" => result.push('`'),
                        "pre" => result.push_str("\n```\n"),
                        "strong" | "b" => result.push_str("**"),
                        "em" | "i" => result.push('*'),
                        _ => {}
                    }
                } else {
                    match tag_name {
                        "script" | "style" | "head" | "nav" | "footer" | "aside" | "svg" => {
                            skip_content = true;
                        }
                        "br" => result.push('\n'),
                        "p" | "div" => {
                            if !result.ends_with('\n') && !result.is_empty() {
                                result.push('\n');
                            }
                        }
                        "h1" => result.push_str("\n\n# "),
                        "h2" => result.push_str("\n\n## "),
                        "h3" => result.push_str("\n\n### "),
                        "h4" => result.push_str("\n\n#### "),
                        "h5" => result.push_str("\n\n##### "),
                        "h6" => result.push_str("\n\n###### "),
                        "li" => result.push_str("\n- "),
                        "code" => result.push('`'),
                        "pre" => result.push_str("\n```\n"),
                        "strong" | "b" => result.push_str("**"),
                        "em" | "i" => result.push('*'),
                        _ => {}
                    }
                }
            } else {
                current_tag.push(c);
            }
            continue;
        }

        if !skip_content {
            if c == '&' {
                let mut entity = String::new();
                while let Some(&next) = chars.peek() {
                    if next == ';' {
                        chars.next();
                        break;
                    }
                    if next.is_alphanumeric() || next == '#' {
                        entity.push(chars.next().unwrap());
                    } else {
                        break;
                    }
                }
                match entity.as_str() {
                    "amp" => result.push('&'),
                    "lt" => result.push('<'),
                    "gt" => result.push('>'),
                    "quot" => result.push('"'),
                    "apos" => result.push('\''),
                    "nbsp" => result.push(' '),
                    _ if entity.starts_with('#') => {
                        if let Ok(code) = entity[1..].parse::<u32>() {
                            if let Some(ch) = char::from_u32(code) {
                                result.push(ch);
                            }
                        }
                    }
                    _ => {
                        result.push('&');
                        result.push_str(&entity);
                        result.push(';');
                    }
                }
            } else {
                result.push(c);
            }
        }
    }

    // Clean up whitespace
    let lines: Vec<&str> = result.lines().collect();
    let mut cleaned = String::new();
    let mut prev_empty = false;

    for line in lines {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            if !prev_empty {
                cleaned.push('\n');
                prev_empty = true;
            }
        } else {
            cleaned.push_str(trimmed);
            cleaned.push('\n');
            prev_empty = false;
        }
    }

    cleaned.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_html_to_markdown_basic() {
        let html = "<h1>Title</h1><p>Hello <strong>world</strong>!</p>";
        let md = html_to_markdown(html);
        assert!(md.contains("# Title"));
        assert!(md.contains("**world**"));
    }

    #[test]
    fn test_html_to_markdown_strips_script() {
        let html = "<p>Before</p><script>alert('bad')</script><p>After</p>";
        let md = html_to_markdown(html);
        assert!(md.contains("Before"));
        assert!(md.contains("After"));
        assert!(!md.contains("alert"));
    }
}
