use crate::steps::DslStep;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

pub mod loader;
pub mod parser;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Dsl {
    pub steps: IndexMap<String, DslStep>,
    #[serde(skip)]
    pub declaration: Option<DeclarationStep>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DeclarationStep {
    pub version: Option<String>,
    pub description: Option<String>,
    pub method: Option<String>,
    pub accepts: Option<String>,
    pub returns: Option<String>,
    pub namespace: Option<String>,
    pub allowed_body: Option<Vec<String>>,
    pub allowed_header: Option<Vec<String>>,
    pub allowed_params: Option<Vec<String>>,
}

impl Dsl {
    pub fn new(steps: IndexMap<String, DslStep>) -> Self {
        let declaration = steps
            .values()
            .find_map(|step| {
                if let DslStep::Declaration(decl) = step {
                    Some(decl.clone())
                } else {
                    None
                }
            });

        Self { steps, declaration }
    }

    pub fn get_step(&self, name: &str) -> Option<&DslStep> {
        self.steps.get(name)
    }

    pub fn step_names(&self) -> Vec<String> {
        self.steps.keys().cloned().collect()
    }
}
