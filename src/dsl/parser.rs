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
        let content = fs::read_to_string(path)?;
        let replaced = self.replace_constants(&content);

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

    fn parse_step(&self, name: &str, value: YamlValue) -> Result<DslStep> {
        let step = serde_yaml_ng::from_value(value).map_err(|e| {
            RuuterError::DslParse(format!("Failed to parse step '{}': {}", name, e))
        })?;

        Ok(step)
    }
}
