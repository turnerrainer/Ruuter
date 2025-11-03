use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

#[derive(Debug, Clone)]
pub struct ExecutionContext {
    variables: Arc<RwLock<HashMap<String, Value>>>,
    request_body: HashMap<String, Value>,
    request_query: HashMap<String, Value>,
    request_headers: HashMap<String, String>,
    request_origin: String,
}

impl ExecutionContext {
    pub fn new(
        body: HashMap<String, Value>,
        query: HashMap<String, Value>,
        headers: HashMap<String, String>,
        origin: String,
    ) -> Self {
        Self {
            variables: Arc::new(RwLock::new(HashMap::new())),
            request_body: body,
            request_query: query,
            request_headers: headers,
            request_origin: origin,
        }
    }

    pub fn set_variable(&self, key: String, value: Value) {
        if let Ok(mut vars) = self.variables.write() {
            vars.insert(key, value);
        }
    }

    pub fn get_variable(&self, key: &str) -> Option<Value> {
        self.variables.read().ok()?.get(key).cloned()
    }

    pub fn get_all_variables(&self) -> HashMap<String, Value> {
        self.variables.read().ok().map(|v| v.clone()).unwrap_or_default()
    }

    pub fn request_body(&self) -> &HashMap<String, Value> {
        &self.request_body
    }

    pub fn request_query(&self) -> &HashMap<String, Value> {
        &self.request_query
    }

    pub fn request_headers(&self) -> &HashMap<String, String> {
        &self.request_headers
    }

    pub fn request_origin(&self) -> &str {
        &self.request_origin
    }
}
