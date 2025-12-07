//! Handlebars templates for system prompts
//!
//! Based on agent_crate/src/templates.rs

use anyhow::Result;
use handlebars::Handlebars;
use serde::Serialize;
use std::sync::Arc;

const SYSTEM_PROMPT_TEMPLATE: &str = include_str!("templates/system_prompt.hbs");

/// Holds the handlebars templates
pub struct Templates {
    handlebars: Handlebars<'static>,
}

impl Templates {
    pub fn new() -> Arc<Self> {
        let mut handlebars = Handlebars::new();
        handlebars.set_strict_mode(true);
        handlebars.register_helper("contains", Box::new(contains_helper));
        handlebars.register_helper("gt", Box::new(gt_helper));
        handlebars.register_helper("len", Box::new(len_helper));
        handlebars.register_helper("or", Box::new(or_helper));
        handlebars
            .register_template_string("system_prompt.hbs", SYSTEM_PROMPT_TEMPLATE)
            .expect("Failed to register system prompt template");

        Arc::new(Self { handlebars })
    }

    /// Render a template by name with the given data
    pub fn render<T: Serialize>(&self, template_name: &str, data: &T) -> Result<String> {
        Ok(self.handlebars.render(template_name, data)?)
    }
}

impl Default for Templates {
    fn default() -> Self {
        Self {
            handlebars: Handlebars::new(),
        }
    }
}

/// Context for a worktree (project root directory)
#[derive(Serialize, Clone, Debug)]
pub struct WorktreeContext {
    pub abs_path: String,
    pub root_name: String,
    pub rules_file: Option<RulesFile>,
}

#[derive(Serialize, Clone, Debug)]
pub struct RulesFile {
    pub path_in_worktree: String,
    pub text: String,
}

/// Project context for the system prompt template
#[derive(Serialize, Default, Clone, Debug)]
pub struct ProjectContext {
    pub worktrees: Vec<WorktreeContext>,
    pub os: String,
    pub shell: String,
    pub has_rules: bool,
    pub has_user_rules: bool,
    pub user_rules: Vec<UserRule>,
}

#[derive(Serialize, Clone, Debug)]
pub struct UserRule {
    pub title: Option<String>,
    pub contents: String,
}

/// Data for rendering the system prompt template
#[derive(Serialize)]
pub struct SystemPromptTemplate<'a> {
    #[serde(flatten)]
    pub project: &'a ProjectContext,
    pub available_tools: Vec<String>,
    pub model_name: Option<String>,
}

impl SystemPromptTemplate<'_> {
    pub fn render(&self, templates: &Templates) -> Result<String> {
        templates.render("system_prompt.hbs", self)
    }
}

// Use handlebars_helper! macro for subexpression-compatible helpers

/// Handlebars helper for checking if an item is in a list
fn contains_helper(
    h: &handlebars::Helper,
    _: &handlebars::Handlebars,
    _: &handlebars::Context,
    _: &mut handlebars::RenderContext,
    out: &mut dyn handlebars::Output,
) -> handlebars::HelperResult {
    use handlebars::RenderErrorReason;
    let list = h
        .param(0)
        .and_then(|v| v.value().as_array())
        .ok_or(RenderErrorReason::ParamNotFoundForIndex("contains", 0))?;
    let query = h
        .param(1)
        .map(|v| v.value())
        .ok_or(RenderErrorReason::ParamNotFoundForIndex("contains", 1))?;

    if list.contains(query) {
        out.write("true")?;
    }

    Ok(())
}

// Greater than comparison (works as subexpression)
handlebars::handlebars_helper!(gt_helper: |a: u64, b: u64| a > b);

// Length of array (works as subexpression)
handlebars::handlebars_helper!(len_helper: |arr: Json| {
    arr.as_array().map(|a| a.len() as u64).unwrap_or(0)
});

// Logical or
handlebars::handlebars_helper!(or_helper: |a: bool, b: bool| a || b);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_system_prompt_template() {
        let project = ProjectContext {
            worktrees: vec![WorktreeContext {
                abs_path: "/home/user/project".to_string(),
                root_name: "project".to_string(),
                rules_file: None,
            }],
            os: "Linux".to_string(),
            shell: "bash".to_string(),
            has_rules: false,
            has_user_rules: false,
            user_rules: vec![],
        };
        let template = SystemPromptTemplate {
            project: &project,
            available_tools: vec!["grep".to_string(), "read_file".to_string()],
            model_name: Some("test-model".to_string()),
        };
        let templates = Templates::new();
        let rendered = template.render(&templates).unwrap();
        assert!(rendered.contains("## Fixing Diagnostics"));
        assert!(rendered.contains("test-model"));
        assert!(rendered.contains("/home/user/project"));
    }
}
