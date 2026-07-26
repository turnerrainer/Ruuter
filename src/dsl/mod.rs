use crate::steps::DslStep;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

pub mod hot_reload;
pub mod interpolate;
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
    /// Task 020 — when `Some(true)` on a guard DSL, this guard REPLACES
    /// all ancestor guards for the routes it protects (rather than
    /// stacking on top of them). Used when a specific endpoint has
    /// materially different privilege than its siblings — e.g. a
    /// stricter admin gate that shouldn't be additive to a folder-wide
    /// "authenticated" check.
    pub override_ancestors: Option<bool>,
}

impl Dsl {
    pub fn new(steps: IndexMap<String, DslStep>) -> Self {
        let declaration = steps.values().find_map(|step| {
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
