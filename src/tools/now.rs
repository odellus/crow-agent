//! Now tool - Returns current date and time

use rig::completion::ToolDefinition;
use rig::tool::Tool;
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Debug, thiserror::Error)]
#[error("Now tool error: {0}")]
pub struct NowError(String);

#[derive(Debug, Deserialize)]
pub struct NowArgs {
    /// Optional timezone (e.g., "UTC", "America/New_York"). Defaults to local time.
    #[serde(default)]
    pub timezone: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Now;

impl Tool for Now {
    const NAME: &'static str = "now";

    type Error = NowError;
    type Args = NowArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: "now".to_string(),
            description: "Get the current date and time. Useful for time-sensitive operations or logging.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "timezone": {
                        "type": "string",
                        "description": "Optional timezone. Defaults to local time if not specified."
                    }
                },
                "required": []
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let now = chrono::Local::now();

        let formatted = match args.timezone.as_deref() {
            Some("UTC") | Some("utc") => {
                let utc = chrono::Utc::now();
                format!(
                    "{}\nTimezone: UTC",
                    utc.format("%Y-%m-%d %H:%M:%S %Z")
                )
            }
            Some(tz) => {
                // For simplicity, just note the requested timezone but use local
                // Full timezone support would require chrono-tz crate
                format!(
                    "{}\nTimezone: Local (requested: {})",
                    now.format("%Y-%m-%d %H:%M:%S %Z"),
                    tz
                )
            }
            None => {
                format!(
                    "{}\nTimezone: Local",
                    now.format("%Y-%m-%d %H:%M:%S %Z")
                )
            }
        };

        Ok(formatted)
    }
}
