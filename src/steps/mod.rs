use crate::context::ExecutionContext;
use crate::dsl::DeclarationStep;
use crate::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

pub mod assign;
pub mod http;
pub mod log;
pub mod return_step;
pub mod switch;
pub mod template;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub enum DslStep {
    Assign(AssignStep),
    Return(ReturnStep),
    Http(HttpStep),
    Switch(SwitchStep),
    Log(LogStep),
    Template(TemplateStep),
    Declaration(DeclarationStep),
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AssignStep {
    pub assign: HashMap<String, Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skip: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ReturnStep {
    #[serde(rename = "return")]
    pub return_value: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub headers: Option<HashMap<String, Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wrapper: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct HttpStep {
    pub call: String,
    pub args: HttpArgs,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout: Option<u64>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct HttpArgs {
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<HashMap<String, Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub query: Option<HashMap<String, Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub headers: Option<HashMap<String, Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SwitchStep {
    pub switch: Vec<Condition>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Condition {
    pub condition: String,
    pub next: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LogStep {
    pub log: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TemplateStep {
    pub template: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<HashMap<String, Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub query: Option<HashMap<String, Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub headers: Option<HashMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next: Option<String>,
}

pub trait StepExecutor {
    async fn execute(&self, context: &ExecutionContext) -> Result<StepResult>;
}

#[derive(Debug, Clone)]
pub struct StepResult {
    pub next_step: Option<String>,
    pub goto_step: Option<String>,
    pub should_return: bool,
    pub return_value: Option<Value>,
    pub return_status: Option<u16>,
    pub return_headers: Option<HashMap<String, String>>,
}

impl StepResult {
    pub fn new() -> Self {
        Self {
            next_step: None,
            goto_step: None,
            should_return: false,
            return_value: None,
            return_status: None,
            return_headers: None,
        }
    }

    pub fn with_next(next: String) -> Self {
        Self {
            next_step: Some(next),
            ..Self::new()
        }
    }

    pub fn with_return(value: Value, status: Option<u16>, headers: Option<HashMap<String, String>>) -> Self {
        Self {
            should_return: true,
            return_value: Some(value),
            return_status: status,
            return_headers: headers,
            ..Self::new()
        }
    }
}
