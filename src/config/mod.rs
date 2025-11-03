use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AppConfig {
    #[serde(default = "default_config_path")]
    pub config_path: PathBuf,

    #[serde(default = "default_port")]
    pub port: u16,

    #[serde(default)]
    pub stop_in_case_of_exception: bool,

    #[serde(default)]
    pub http_codes_allow_list: Vec<u16>,

    #[serde(default)]
    pub max_step_recursions: Option<u32>,

    #[serde(default)]
    pub http_response_size_limit: Option<usize>,

    #[serde(default = "default_http_request_timeout")]
    pub http_request_timeout: u64,

    #[serde(default)]
    pub cors: CorsConfig,

    #[serde(default)]
    pub dsl: DslConfig,

    #[serde(default)]
    pub logging: LoggingConfig,

    #[serde(default)]
    pub incoming_requests: IncomingRequestsConfig,

    #[serde(default)]
    pub response_default_headers: HashMap<String, String>,

    #[serde(default)]
    pub internal_requests: InternalRequestsConfig,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct CorsConfig {
    pub allowed_origins: Vec<String>,
    #[serde(default)]
    pub allow_credentials: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DslConfig {
    #[serde(default = "default_allowed_filetypes")]
    pub allowed_filetypes: Vec<String>,

    #[serde(default = "default_processed_filetypes")]
    pub processed_filetypes: Vec<String>,

    #[serde(default)]
    pub allow_dsl_reloading: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct LoggingConfig {
    #[serde(default)]
    pub display_request_content: bool,

    #[serde(default)]
    pub display_response_content: bool,

    #[serde(default)]
    pub print_stack_trace: bool,

    #[serde(default)]
    pub meaningful_errors: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct IncomingRequestsConfig {
    #[serde(default = "default_allowed_methods")]
    pub allowed_method_types: Vec<String>,

    #[serde(default)]
    pub headers: HashMap<String, String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct InternalRequestsConfig {
    #[serde(default)]
    pub disabled: bool,

    #[serde(default)]
    pub allowed_ips: Vec<String>,

    #[serde(default)]
    pub allowed_urls: Vec<String>,
}

fn default_config_path() -> PathBuf {
    PathBuf::from("./DSL")
}

fn default_port() -> u16 {
    8080
}

fn default_http_request_timeout() -> u64 {
    15000
}

fn default_allowed_filetypes() -> Vec<String> {
    vec![".yml".to_string(), ".yaml".to_string()]
}

fn default_processed_filetypes() -> Vec<String> {
    vec![".yml".to_string(), ".yaml".to_string()]
}

fn default_allowed_methods() -> Vec<String> {
    vec!["GET".to_string(), "POST".to_string()]
}

impl Default for DslConfig {
    fn default() -> Self {
        Self {
            allowed_filetypes: default_allowed_filetypes(),
            processed_filetypes: default_processed_filetypes(),
            allow_dsl_reloading: false,
        }
    }
}

impl Default for IncomingRequestsConfig {
    fn default() -> Self {
        Self {
            allowed_method_types: default_allowed_methods(),
            headers: HashMap::new(),
        }
    }
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            config_path: default_config_path(),
            port: default_port(),
            stop_in_case_of_exception: true,
            http_codes_allow_list: vec![200, 201, 202],
            max_step_recursions: Some(10),
            http_response_size_limit: Some(256 * 1024),
            http_request_timeout: default_http_request_timeout(),
            cors: CorsConfig::default(),
            dsl: DslConfig::default(),
            logging: LoggingConfig::default(),
            incoming_requests: IncomingRequestsConfig::default(),
            response_default_headers: HashMap::new(),
            internal_requests: InternalRequestsConfig::default(),
        }
    }
}

pub fn load_constants(path: &str) -> crate::Result<HashMap<String, String>> {
    use std::fs::File;
    use std::io::{BufRead, BufReader};

    let file = File::open(path).map_err(|_| {
        crate::RuuterError::Config(format!("Cannot open constants file: {}", path))
    })?;

    let reader = BufReader::new(file);
    let mut constants = HashMap::new();
    let mut current_section = String::new();

    for line in reader.lines() {
        let line = line?;
        let trimmed = line.trim();

        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            current_section = trimmed[1..trimmed.len() - 1].to_string();
            continue;
        }

        if let Some(pos) = trimmed.find('=') {
            let key = trimmed[..pos].trim();
            let value = trimmed[pos + 1..].trim();
            constants.insert(key.to_string(), value.to_string());
        }
    }

    Ok(constants)
}
