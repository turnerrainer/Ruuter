//! YAML schema for `<project>/sources/<source_name>.yml`.

use crate::{Result, RuuterError};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SourceConfig {
    #[serde(rename = "websocket")]
    WebSocket(WsSourceConfig),
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct WsSourceConfig {
    pub url: String,

    /// Headers sent on the WS upgrade request. Values run through the
    /// same `[#constant]` substitution as `url`. Required for
    /// auth-on-upgrade integrations like Andmela (`X-Andmela-Token`).
    #[serde(default)]
    pub headers: HashMap<String, String>,

    /// JSON frames sent immediately after the WS handshake completes.
    /// Replayed after every successful reconnect.
    #[serde(default)]
    pub on_connect: Vec<OnConnectAction>,

    /// How to derive `(channel, key)` from each inbound JSON message.
    pub dispatch: DispatchConfig,

    /// Reconnect / backoff policy. Applied uniformly to connection
    /// failures and post-connect read errors.
    #[serde(default)]
    pub reconnect: ReconnectPolicy,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub enum OnConnectAction {
    /// Send `value` serialized as a JSON text frame.
    #[serde(rename = "send_json")]
    SendJson(Value),
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DispatchConfig {
    /// Dot-path against the inbound JSON message, e.g. `"$.T"` selects
    /// the `T` field at the root. Missing paths produce an empty string
    /// and the message is logged at debug + dropped.
    pub channel: String,
    pub key: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ReconnectPolicy {
    #[serde(default = "default_initial_backoff_ms")]
    pub initial_backoff_ms: u64,
    #[serde(default = "default_max_backoff_ms")]
    pub max_backoff_ms: u64,
    #[serde(default = "default_jitter")]
    pub jitter: bool,
}

impl Default for ReconnectPolicy {
    fn default() -> Self {
        Self {
            initial_backoff_ms: default_initial_backoff_ms(),
            max_backoff_ms: default_max_backoff_ms(),
            jitter: default_jitter(),
        }
    }
}

fn default_initial_backoff_ms() -> u64 {
    500
}
fn default_max_backoff_ms() -> u64 {
    60_000
}
fn default_jitter() -> bool {
    true
}

/// Substitute `[#name]` references with values from `constants` in every
/// string-valued field of `cfg`. Returns a fresh `WsSourceConfig`.
/// Missing constants are an error — we don't want a typo'd `[#api_key]`
/// to silently become a literal string sent over the wire.
pub fn resolve_constants(
    cfg: &WsSourceConfig,
    constants: &HashMap<String, String>,
) -> Result<WsSourceConfig> {
    let mut out = cfg.clone();
    out.url = sub(&out.url, constants)?;
    out.dispatch.channel = sub(&out.dispatch.channel, constants)?;
    out.dispatch.key = sub(&out.dispatch.key, constants)?;

    let mut resolved_headers: HashMap<String, String> = HashMap::with_capacity(out.headers.len());
    for (k, v) in out.headers.into_iter() {
        let kk = sub(&k, constants)?;
        let vv = sub(&v, constants)?;
        resolved_headers.insert(kk, vv);
    }
    out.headers = resolved_headers;

    for action in &mut out.on_connect {
        match action {
            OnConnectAction::SendJson(value) => {
                *value = sub_value(value.clone(), constants)?;
            }
        }
    }
    Ok(out)
}

fn sub(s: &str, constants: &HashMap<String, String>) -> Result<String> {
    // Both `[#NAME]` and `#{NAME}` are accepted (task 067). Unlike the
    // DSL parser, this substitution is STRICT: a missing constant is
    // a config error, not a silent literal — a typo'd `[#api_key]`
    // that hit the wire would be a real problem for source configs.
    let mut missing: Option<String> = None;
    let result = crate::dsl::interpolate::substitute(s, |key| match constants.get(key) {
        Some(v) => Some(v.clone()),
        None => {
            if missing.is_none() {
                missing = Some(key.to_string());
            }
            None
        }
    });
    if let Some(k) = missing {
        return Err(RuuterError::Config(format!("undefined constant [#{}]", k)));
    }
    Ok(result)
}

fn sub_value(v: Value, constants: &HashMap<String, String>) -> Result<Value> {
    Ok(match v {
        Value::String(s) => Value::String(sub(&s, constants)?),
        Value::Array(arr) => {
            let mut out = Vec::with_capacity(arr.len());
            for item in arr {
                out.push(sub_value(item, constants)?);
            }
            Value::Array(out)
        }
        Value::Object(map) => {
            let mut out = serde_json::Map::with_capacity(map.len());
            for (k, val) in map {
                out.insert(k, sub_value(val, constants)?);
            }
            Value::Object(out)
        }
        other => other,
    })
}

/// Extract a value from `payload` using a dot-path like `"$.field.sub"`.
/// Returns `None` if any segment is missing. The leading `$.` is required
/// and stripped; an unprefixed expression is treated as a literal string.
pub fn extract_path(payload: &Value, expr: &str) -> Option<String> {
    let path = expr.strip_prefix("$.")?;
    let mut cur = payload;
    for segment in path.split('.') {
        cur = cur.get(segment)?;
    }
    Some(match cur {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Null => "null".to_string(),
        other => other.to_string(),
    })
}
