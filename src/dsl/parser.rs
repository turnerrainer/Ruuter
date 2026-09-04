use crate::dsl::interpolate;
use crate::dsl::Dsl;
use crate::steps::DslStep;
use crate::{Result, RuuterError};
use indexmap::IndexMap;
use serde_yaml_ng::Value as YamlValue;
use std::collections::HashMap;
use std::fs;
use std::path::Path;

pub struct DslParser {
    constants: HashMap<String, String>,
}

impl DslParser {
    pub fn new(constants: HashMap<String, String>) -> Self {
        Self { constants }
    }

    pub fn parse_file(&self, path: &Path) -> Result<Dsl> {
        // Issue #26 — every error bubbling out of parse_file MUST
        // include the file path, otherwise an operator with dozens
        // of DSLs sees `YAML error: did not find expected key at
        // line 55 column 39` with no way to tell which file. Wrap
        // once at the file-boundary; downstream errors (step-level,
        // JSON reserialisation) already name the offending step, so
        // path + step-name together give the operator a full pointer.
        let content = fs::read_to_string(path)
            .map_err(|e| RuuterError::DslParse(format!("{}: {}", path.display(), e)))?;
        self.parse_content(&content)
            .map_err(|e| RuuterError::DslParse(format!("{}: {}", path.display(), e)))
    }

    /// Parse a DSL from an in-memory YAML string. Constants
    /// substitution runs (using the parser's configured constants map)
    /// so tests / linters / IDE plugins that don't have a filesystem
    /// path can drive the parser the same way `parse_file` does.
    pub fn parse_content(&self, content: &str) -> Result<Dsl> {
        let replaced = self.replace_constants(content);
        // IndexMap preserves source order — the entry step is whatever
        // comes first in the YAML, as the Ruuter DSL contract requires.
        let yaml: IndexMap<String, YamlValue> = serde_yaml_ng::from_str(&replaced)?;
        let steps = self.parse_steps(yaml)?;
        Ok(Dsl::new(steps))
    }

    fn replace_constants(&self, content: &str) -> String {
        // Both `[#NAME]` and `#{NAME}` are accepted (task 067). Missing
        // keys are preserved as their original literal; the lint tool
        // surfaces them.
        interpolate::substitute(content, |k| self.constants.get(k).cloned())
    }

    fn parse_steps(&self, yaml: IndexMap<String, YamlValue>) -> Result<IndexMap<String, DslStep>> {
        let mut steps = IndexMap::with_capacity(yaml.len());

        for (name, value) in yaml {
            let step = self.parse_step(&name, value)?;
            steps.insert(name, step);
        }

        Ok(steps)
    }

    /// Audit finding 09: explicit discriminator-based dispatch,
    /// matching Java's `DslMappingHelper.convertJsonNodeToDslStep`.
    ///
    /// The pre-fix Rust parser used `#[serde(untagged)]` on `DslStep`,
    /// which relied on serde trying every variant in order and picking
    /// the first that parses. Two failure modes:
    ///
    /// 1. `DeclarationStep` has zero required fields, so any typo'd
    ///    step (`asign:` instead of `assign:`) parsed as an empty
    ///    declaration and ran as a silent no-op.
    /// 2. `call: reflect.mock` had no Rust match, so it also fell
    ///    through to Declaration and silently disappeared.
    ///
    /// This routing looks at the top-level keys of the YAML map and
    /// dispatches to the exact variant. Unknown shapes are a hard
    /// parse error at load time (matches Java's
    /// `throw new IllegalArgumentException("Invalid step type.")`).
    fn parse_step(&self, name: &str, value: YamlValue) -> Result<DslStep> {
        let mapping = match &value {
            YamlValue::Mapping(m) => m,
            _ => {
                return Err(RuuterError::DslParse(format!(
                    "step '{}' must be a YAML mapping, got {:?}",
                    name, value
                )));
            }
        };

        let key_present = |k: &str| mapping.contains_key(YamlValue::String(k.to_string()));
        let str_field = |k: &str| -> Option<String> {
            mapping
                .get(YamlValue::String(k.to_string()))
                .and_then(|v| v.as_str().map(|s| s.to_string()))
        };

        // Issue #56 — one step = one action. The parser used to
        // dispatch by if-ladder priority and let serde silently drop
        // every non-winning key, which meant a YAML step listing both
        // `log:` and `call:` would run the http call and silently
        // discard the log message. Reviewers couldn't tell from the
        // DSL what would actually execute. Reject at parse time
        // instead, naming every offending key, so authors either
        // split the step or fix the typo.
        const ACTION_KEYS: &[&str] = &[
            "call",
            "template",
            "assign",
            "return",
            "switch",
            "log",
            "state",
            "iterate",
            "ws_send",
            "ws_tag",
            "single_flight",
        ];
        let present: Vec<&str> = ACTION_KEYS
            .iter()
            .copied()
            .filter(|k| key_present(k))
            .collect();
        if present.len() > 1 {
            return Err(RuuterError::DslParse(format!(
                "step '{}': multiple action keys present ({}). Each step \
                 must contain exactly one action — split into separate \
                 steps and chain them with `next:`.",
                name,
                present.join(", ")
            )));
        }

        let variant_hint: &str = if let Some(call) = str_field("call") {
            match call.as_str() {
                "declare" => "declaration",
                "reflect.mock" => "http_mock",
                s if s.starts_with("http.") => "http",
                other => {
                    return Err(RuuterError::DslParse(format!(
                        "step '{}': unknown call '{}' (expected 'declare', 'reflect.mock', or 'http.<verb>')",
                        name, other
                    )));
                }
            }
        } else if key_present("template") {
            "template"
        } else if key_present("assign") {
            "assign"
        } else if key_present("return") {
            "return"
        } else if key_present("switch") {
            "switch"
        } else if key_present("log") {
            "log"
        } else if key_present("state") {
            "state"
        } else if key_present("iterate") {
            "iterate"
        } else if key_present("ws_send") {
            "ws_send"
        } else if key_present("ws_tag") {
            "ws_tag"
        } else if key_present("single_flight") {
            "single_flight"
        } else if
        // Rust-extension implicit declaration: a step whose body
        // is one of the Declaration-only fields. Java uses the
        // explicit `call: declare` form; Rust callers historically
        // wrote `declaration: { override_ancestors: true }` etc.
        // Both work. `method`, `accepts` were dropped in task 070
        // (dead struct fields); `returns` was repurposed as a
        // typed response schema; `strict` is a new task-070 flag.
        [
            "version",
            "description",
            "returns",
            "namespace",
            "allowed_body",
            "allowed_header",
            "allowed_params",
            "override_ancestors",
            "allowlist",
            "strict",
        ]
        .iter()
        .any(|k| key_present(k))
        {
            "declaration"
        } else if mapping.iter().all(|(k, _)| {
            matches!(
                k.as_str(),
                Some("next")
                    | Some("skip")
                    | Some("sleep")
                    | Some("maxRecursions")
                    | Some("max_recursions")
                    | Some("reloadDsl")
                    | Some("reloadDsls")
                    | Some("reload_dsl")
                    | Some("reload_dsls")
            )
        }) {
            // Bare control-flow step — only base fields, no action
            // body. Common shape in Rust tests: `skip: { next: end }`
            // where the step name is a jump target but performs no
            // action. Treat as an implicit Declaration (engine
            // no-op) so it can carry base fields uniformly.
            "declaration"
        } else {
            // No discriminator found. Java throws IllegalArgumentException;
            // we match by returning a hard parse error rather than
            // silently coercing to Declaration.
            return Err(RuuterError::DslParse(format!(
                "step '{}': no recognised step discriminator (expected one of \
                 call:, template:, assign:, return:, switch:, log:, state:, \
                 iterate:, ws_send:, single_flight:, or a declaration field)",
                name
            )));
        };

        // Route through serde_json::Value rather than serde_yaml's
        // native Value machinery. serde_yaml_ng's `from_value` on a
        // `Value::Mapping` cannot deserialise externally-tagged enums
        // that use the single-key-map form (`state: { get: {...} }`
        // → `StateOp::Get { … }`) — it expects a `Value::Tagged`
        // instead. Round-tripping through serde_json gives us the
        // permissive-external-tag behaviour DSL authors write.
        let json_value: serde_json::Value = serde_json::to_value(&value)
            .map_err(|e| RuuterError::DslParse(format!("step '{}': {}", name, e)))?;
        let step: DslStep = match variant_hint {
            "declaration" => serde_json::from_value::<crate::dsl::DeclarationStep>(json_value)
                .map(DslStep::Declaration)
                .map_err(|e| Self::parse_err_json(name, e))?,
            "http_mock" => serde_json::from_value::<crate::steps::HttpMockStep>(json_value)
                .map(DslStep::HttpMock)
                .map_err(|e| Self::parse_err_json(name, e))?,
            "http" => serde_json::from_value::<crate::steps::HttpStep>(json_value)
                .map(DslStep::Http)
                .map_err(|e| Self::parse_err_json(name, e))?,
            "template" => serde_json::from_value::<crate::steps::TemplateStep>(json_value)
                .map(DslStep::Template)
                .map_err(|e| Self::parse_err_json(name, e))?,
            "assign" => serde_json::from_value::<crate::steps::AssignStep>(json_value)
                .map(DslStep::Assign)
                .map_err(|e| Self::parse_err_json(name, e))?,
            "return" => serde_json::from_value::<crate::steps::ReturnStep>(json_value)
                .map(DslStep::Return)
                .map_err(|e| Self::parse_err_json(name, e))?,
            "switch" => serde_json::from_value::<crate::steps::SwitchStep>(json_value)
                .map(DslStep::Switch)
                .map_err(|e| Self::parse_err_json(name, e))?,
            "log" => serde_json::from_value::<crate::steps::LogStep>(json_value)
                .map(DslStep::Log)
                .map_err(|e| Self::parse_err_json(name, e))?,
            "state" => serde_json::from_value::<crate::steps::StateStep>(json_value)
                .map(DslStep::State)
                .map_err(|e| Self::parse_err_json(name, e))?,
            "iterate" => serde_json::from_value::<crate::steps::IterateStep>(json_value)
                .map(DslStep::Iterate)
                .map_err(|e| Self::parse_err_json(name, e))?,
            "ws_send" => serde_json::from_value::<crate::steps::WsSendStep>(json_value)
                .map(DslStep::WsSend)
                .map_err(|e| Self::parse_err_json(name, e))?,
            "ws_tag" => serde_json::from_value::<crate::steps::WsTagStep>(json_value)
                .map(DslStep::WsTag)
                .map_err(|e| Self::parse_err_json(name, e))?,
            "single_flight" => serde_json::from_value::<crate::steps::SingleFlightStep>(json_value)
                .map(DslStep::SingleFlight)
                .map_err(|e| Self::parse_err_json(name, e))?,
            _ => unreachable!(),
        };

        Ok(step)
    }

    #[allow(dead_code)]
    fn parse_err(name: &str, e: serde_yaml_ng::Error) -> RuuterError {
        RuuterError::DslParse(format!("Failed to parse step '{}': {}", name, e))
    }

    fn parse_err_json(name: &str, e: serde_json::Error) -> RuuterError {
        RuuterError::DslParse(format!("Failed to parse step '{}': {}", name, e))
    }
}
