use crate::config::AppConfig;
use crate::dsl::{parser::DslParser, Dsl};
use crate::{Result, RuuterError};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

/// Project-name → method → DSL-key → Dsl (HTTP routes).
pub type HttpDsls = HashMap<String, HashMap<String, HashMap<String, Dsl>>>;
/// (project, channel) → trigger-key → Dsl (event-trigger pipelines).
pub type TriggerDsls = HashMap<(String, String), HashMap<String, Dsl>>;
/// Project-name → guard-key (`<METHOD>/<dir-stem>`) → guard DSL.
/// A guard at key `<METHOD>/<stem>` protects every DSL whose key
/// starts with `<METHOD>/<stem>/`. Multiple ancestor guards stack.
pub type GuardDsls = HashMap<String, HashMap<String, Dsl>>;

/// Reserved per-project subdirectory names — NOT treated as HTTP methods.
const RESERVED_SUBDIRS: &[&str] = &["triggers", "sources"];

#[derive(Default)]
pub struct LoadedProjects {
    pub http: HttpDsls,
    pub triggers: TriggerDsls,
    pub guards: GuardDsls,
}

pub struct DslLoader {
    config: AppConfig,
    constants: HashMap<String, String>,
}

impl DslLoader {
    pub fn new(config: AppConfig, constants: HashMap<String, String>) -> Self {
        Self { config, constants }
    }

    /// Backwards-compatible wrapper for callers that only want the HTTP tree.
    pub fn load_all(&self) -> Result<HttpDsls> {
        Ok(self.load_everything()?.http)
    }

    /// Load both HTTP routes and event triggers, side by side.
    pub fn load_everything(&self) -> Result<LoadedProjects> {
        let mut out = LoadedProjects::default();

        for entry in fs::read_dir(&self.config.config_path)? {
            let entry = entry?;
            let path = entry.path();

            if path.is_dir() {
                let project_name = path.file_name()
                    .and_then(|n| n.to_str())
                    .ok_or_else(|| RuuterError::FileNotFound("Invalid project name".to_string()))?
                    .to_string();

                let (methods, triggers, guards) = self.load_project(&path, &project_name)?;
                out.http.insert(project_name.clone(), methods);
                for ((_, channel), per_key) in triggers {
                    out.triggers.insert((project_name.clone(), channel), per_key);
                }
                if !guards.is_empty() {
                    out.guards.insert(project_name, guards);
                }
            }
        }

        Ok(out)
    }

    fn load_project(
        &self,
        project_path: &Path,
        project_name: &str,
    ) -> Result<(HashMap<String, HashMap<String, Dsl>>, TriggerDsls, HashMap<String, Dsl>)> {
        let mut methods = HashMap::new();
        let mut triggers: TriggerDsls = HashMap::new();
        let mut guards: HashMap<String, Dsl> = HashMap::new();

        for entry in fs::read_dir(project_path)? {
            let entry = entry?;
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }

            let name = path.file_name()
                .and_then(|n| n.to_str())
                .ok_or_else(|| RuuterError::FileNotFound("Invalid dir name".to_string()))?
                .to_string();

            if name == "triggers" {
                self.load_triggers(&path, project_name, &mut triggers)?;
            } else if RESERVED_SUBDIRS.contains(&name.as_str()) {
                // sources/ etc. — handled by the source supervisor, not here.
                continue;
            } else {
                let (dsls, method_guards) = self.load_method_dsls(&path, &name)?;
                methods.insert(name, dsls);
                guards.extend(method_guards);
            }
        }

        Ok((methods, triggers, guards))
    }

    fn load_triggers(
        &self,
        triggers_dir: &Path,
        project: &str,
        out: &mut TriggerDsls,
    ) -> Result<()> {
        // Layout: triggers/<channel>/<key>.yml
        let parser = DslParser::new(self.constants.clone());
        for entry in fs::read_dir(triggers_dir)? {
            let entry = entry?;
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let channel = path.file_name()
                .and_then(|n| n.to_str())
                .ok_or_else(|| RuuterError::FileNotFound("Invalid channel name".to_string()))?
                .to_string();

            let mut per_key: HashMap<String, Dsl> = HashMap::new();
            for child in fs::read_dir(&path)? {
                let child = child?;
                let cp = child.path();
                if !self.is_processable_file(&cp) || self.is_guard_file(&cp) {
                    continue;
                }
                let key = cp.file_stem()
                    .and_then(|n| n.to_str())
                    .ok_or_else(|| RuuterError::FileNotFound("Invalid trigger key".to_string()))?
                    .to_string();
                let dsl = parser.parse_file(&cp)?;
                per_key.insert(key, dsl);
            }
            if !per_key.is_empty() {
                out.insert((project.to_string(), channel), per_key);
            }
        }
        Ok(())
    }

    fn load_method_dsls(
        &self,
        method_path: &Path,
        method: &str,
    ) -> Result<(HashMap<String, Dsl>, HashMap<String, Dsl>)> {
        let mut dsls = HashMap::new();
        let mut guards = HashMap::new();
        let parser = DslParser::new(self.constants.clone());

        self.scan_directory(method_path, method_path, method, &parser, &mut dsls, &mut guards)?;

        Ok((dsls, guards))
    }

    fn scan_directory(
        &self,
        dir: &Path,
        base_path: &Path,
        method: &str,
        parser: &DslParser,
        dsls: &mut HashMap<String, Dsl>,
        guards: &mut HashMap<String, Dsl>,
    ) -> Result<()> {
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();

            if path.is_dir() {
                self.scan_directory(&path, base_path, method, parser, dsls, guards)?;
            } else if self.is_processable_file(&path) {
                if self.is_guard_file(&path) {
                    let key = self.build_guard_key(&path, base_path, method)?;
                    let dsl = parser.parse_file(&path)?;
                    guards.insert(key, dsl);
                } else {
                    let key = self.build_dsl_key(&path, base_path, method)?;
                    let dsl = parser.parse_file(&path)?;
                    dsls.insert(key, dsl);
                }
            }
        }

        Ok(())
    }

    fn is_processable_file(&self, path: &Path) -> bool {
        path.extension()
            .and_then(|e| e.to_str())
            .map(|ext| {
                self.config.dsl.processed_filetypes
                    .iter()
                    .any(|allowed| allowed == ext || allowed == &format!(".{}", ext))
            })
            .unwrap_or(false)
    }

    fn is_guard_file(&self, path: &Path) -> bool {
        path.file_name()
            .and_then(|n| n.to_str())
            .map(|name| name.contains(".guard."))
            .unwrap_or(false)
    }

    fn build_dsl_key(&self, path: &Path, base_path: &Path, method: &str) -> Result<String> {
        let rel_path = path.strip_prefix(base_path)
            .map_err(|_| RuuterError::FileNotFound("Invalid path".to_string()))?;

        let path_str = rel_path.with_extension("")
            .to_str()
            .ok_or_else(|| RuuterError::FileNotFound("Invalid path string".to_string()))?
            .replace('\\', "/");

        Ok(format!("{}/{}", method, path_str))
    }

    /// Build a guard key from a `<stem>.guard.yml` file. The guard at
    /// `<method>/path/<stem>.guard.yml` becomes key `<METHOD>/path/<stem>`
    /// — the same shape as a normal DSL key, identifying the
    /// directory the guard protects.
    fn build_guard_key(&self, path: &Path, base_path: &Path, method: &str) -> Result<String> {
        let rel_path = path.strip_prefix(base_path)
            .map_err(|_| RuuterError::FileNotFound("Invalid path".to_string()))?;
        let file_name = path.file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| RuuterError::FileNotFound("Invalid guard name".into()))?;
        // Strip the trailing `.guard.<ext>` suffix.
        let stem = file_name
            .rsplitn(3, '.')
            .nth(2)
            .ok_or_else(|| RuuterError::FileNotFound("Malformed guard filename".into()))?;
        let parent = rel_path.parent()
            .and_then(|p| p.to_str())
            .unwrap_or("");
        let dir_part = parent.replace('\\', "/");
        let path_str = if dir_part.is_empty() {
            stem.to_string()
        } else {
            format!("{}/{}", dir_part, stem)
        };
        Ok(format!("{}/{}", method, path_str))
    }
}
