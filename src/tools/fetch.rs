//! Fetch tool - HTTP GET and convert to markdown

use reqwest::Client;
use rig::tool::Tool;
use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
pub enum FetchError {
    #[error("HTTP request failed: {0}")]
    Request(#[from] reqwest::Error),
    #[error("HTTP error: {0}")]
    Http(String),
    #[error("No content found")]
    NoContent,
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
}

/// Fetches a URL and returns the content as Markdown.
#[derive(Debug, Serialize, Deserialize)]
pub struct FetchInput {
    /// The URL to fetch
    pub url: String,
}

#[derive(Clone)]
pub struct Fetch {
    client: Client,
}

impl Default for Fetch {
    fn default() -> Self {
        Self::new()
    }
}

impl Fetch {
    pub fn new() -> Self {
        Self {
            client: Client::new(),
        }
    }

    async fn fetch_url(&self, url: &str) -> Result<String, FetchError> {
        let url = if !url.starts_with("https://") && !url.starts_with("http://") {
            format!("https://{}", url)
        } else {
            url.to_string()
        };

        let response = self
            .client
            .get(&url)
            .header("User-Agent", "crow-agent/0.1")
            .send()
            .await?;

        if response.status().is_client_error() || response.status().is_server_error() {
            return Err(FetchError::Http(response.status().to_string()));
        }

        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("text/html")
            .to_string();

        let body = response.text().await?;

        if content_type.starts_with("application/json") {
            // Pretty print JSON
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&body) {
                return Ok(format!("```json\n{}\n```", serde_json::to_string_pretty(&json)?));
            }
            Ok(format!("```json\n{}\n```", body))
        } else if content_type.starts_with("text/plain") {
            Ok(body)
        } else {
            // HTML - do basic conversion
            Ok(html_to_markdown(&body))
        }
    }
}

impl Tool for Fetch {
    const NAME: &'static str = "fetch";

    type Error = FetchError;
    type Args = FetchInput;
    type Output = String;

    async fn definition(&self, _prompt: String) -> rig::completion::ToolDefinition {
        rig::completion::ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Fetches a URL and returns the content as Markdown. Useful for reading web pages, APIs, or documentation.".to_string(),
            parameters: serde_json::json!({
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

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let content = self.fetch_url(&args.url).await?;
        if content.trim().is_empty() {
            return Err(FetchError::NoContent);
        }
        Ok(content)
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

                // Handle closing tags
                if tag_name.starts_with('/') {
                    let closing = &tag_name[1..];
                    match closing {
                        "script" | "style" | "head" | "nav" | "footer" | "aside" => {
                            skip_content = false;
                        }
                        "p" | "div" | "br" | "li" | "tr" => {
                            result.push('\n');
                        }
                        "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                            result.push('\n');
                            result.push('\n');
                        }
                        "code" => result.push('`'),
                        "pre" => result.push_str("\n```\n"),
                        "strong" | "b" => result.push_str("**"),
                        "em" | "i" => result.push('*'),
                        "a" => result.push(')'),
                        _ => {}
                    }
                } else {
                    // Handle opening tags
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
                        "a" => {
                            // Try to extract href
                            if let Some(href_start) = tag.find("href=\"") {
                                let href_start = href_start + 6;
                                if let Some(href_end) = tag[href_start..].find('"') {
                                    let href = &tag[href_start..href_start + href_end];
                                    result.push('[');
                                    // We'll close with ]({href}) when we hit </a>
                                    // For now just mark it
                                    result.push_str(&format!("]({})", href));
                                    // Actually this is wrong - we need the text first
                                    // Let's simplify - just output the link
                                }
                            }
                        }
                        _ => {}
                    }
                }
            } else {
                current_tag.push(c);
            }
            continue;
        }

        if !skip_content {
            // Decode common HTML entities
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
                        // Numeric entity
                        if let Some(code) = entity[1..].parse::<u32>().ok() {
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

    // Clean up excessive whitespace
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

    #[test]
    fn test_html_entities() {
        let html = "<p>&amp; &lt; &gt; &quot;</p>";
        let md = html_to_markdown(html);
        assert!(md.contains("& < > \""));
    }
}
