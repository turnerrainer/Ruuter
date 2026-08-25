use thiserror::Error;

pub type Result<T> = std::result::Result<T, RuuterError>;

#[derive(Error, Debug)]
pub enum RuuterError {
    #[error("DSL parsing error: {0}")]
    DslParse(String),

    #[error("DSL execution error in step '{step}': {message}")]
    DslExecution { step: String, message: String },

    #[error("Script evaluation error: {0}")]
    ScriptEvaluation(String),

    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("HTTP request rejected: {0}")]
    HttpRequest(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("YAML error: {0}")]
    Yaml(#[from] serde_yaml_ng::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Invalid DSL step: {0}")]
    InvalidStep(String),

    #[error("Guard failed: {0}")]
    GuardFailed(String),

    #[error("File not found: {0}")]
    FileNotFound(String),

    #[error("Timeout: {0}")]
    Timeout(String),

    /// Task 070 — client-input rejection that must surface as 400
    /// Bad Request rather than the generic 500 that `DslExecution`
    /// maps to. Used today by `declaration.strict: true` to reject
    /// unknown request fields; extensible to other client-input
    /// gates as they land.
    #[error("Bad request: {0}")]
    BadRequest(String),
}
