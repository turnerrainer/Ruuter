use crate::config::AppConfig;
use crate::dsl::{parser::DslParser, Dsl};
use crate::{Result, RuuterError};
use arc_swap::ArcSwap;
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::Arc;

/// Project-name → method → DSL-key → Dsl (HTTP routes).
pub type HttpDsls = HashMap<String, HashMap<String, HashMap<String, Dsl>>>;
/// (project, channel) → trigger-key → Dsl (event-trigger pipelines).
pub type TriggerDsls = HashMap<(String, String), HashMap<String, Dsl>>;
/// Project-name → guard-key (`<METHOD>/<dir-stem>`) → guard DSL.
/// A guard at key `<METHOD>/<stem>` protects every DSL whose key
/// starts with `<METHOD>/<stem>/`. Multiple ancestor guards stack.
pub type GuardDsls = HashMap<String, HashMap<String, Dsl>>;

/// Atomically-swappable handle to the loaded HTTP DSL tree. Held by
/// `DslRouter` and `StepEngine` so the hot-reload watcher can swap the
/// tree without stopping in-flight requests. Reads (`.load()`) are
/// lock-free.
pub type SharedHttpDsls = Arc<ArcSwap<HttpDsls>>;
/// Atomically-swappable handle to the loaded guard tree. Same
/// motivation as `SharedHttpDsls`.
pub type SharedGuards = Arc<ArcSwap<GuardDsls>>;

/// Per-project load result: HTTP method → key → Dsl, triggers, and guards.
type ProjectLoad = (
    HashMap<String, HashMap<String, Dsl>>,
    TriggerDsls,
    HashMap<String, Dsl>,
);

/// Reserved per-project subdirectory names — NOT treated as HTTP methods.
/// `cronmanager-jobs` holds companion configs for the sibling CronManager
/// service; it is documented in samples but does not produce Ruuter routes.
const RESERVED_SUBDIRS: &[&str] = &["triggers", "sources", "cronmanager-jobs"];

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
                let project_name = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .ok_or_else(|| RuuterError::FileNotFound("Invalid project name".to_string()))?
                    .to_string();

                let (methods, triggers, guards) = self.load_project(&path, &project_name)?;
                out.http.insert(project_name.clone(), methods);
                for ((_, channel), per_key) in triggers {
                    out.triggers
                        .insert((project_name.clone(), channel), per_key);
                }
                if !guards.is_empty() {
                    out.guards.insert(project_name, guards);
                }
            }
        }

        Ok(out)
    }

    fn load_project(&self, project_path: &Path, project_name: &str) -> Result<ProjectLoad> {
        let mut methods = HashMap::new();
        let mut triggers: TriggerDsls = HashMap::new();
        let mut guards: HashMap<String, Dsl> = HashMap::new();

        for entry in fs::read_dir(project_path)? {
            let entry = entry?;
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }

            let name = path
                .file_name()
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
            let channel = path
                .file_name()
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
                let key = cp
                    .file_stem()
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

        self.scan_directory(
            method_path,
            method_path,
            method,
            &parser,
            &mut dsls,
            &mut guards,
        )?;

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
        // Accept the Java-Ruuter convention `<dir>/.guard` (no extension).
        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            if name == ".guard" || name == ".guard.yml" || name == ".guard.yaml" {
                return true;
            }
        }
        path.extension()
            .and_then(|e| e.to_str())
            .map(|ext| {
                self.config
                    .dsl
                    .processed_filetypes
                    .iter()
                    .any(|allowed| allowed == ext || allowed == &format!(".{}", ext))
            })
            .unwrap_or(false)
    }

    fn is_guard_file(&self, path: &Path) -> bool {
        path.file_name()
            .and_then(|n| n.to_str())
            .map(|name| {
                // In-folder Java convention: `.guard` (extension-less) or
                // `.guard.yml`, both in the folder they protect.
                name == ".guard"
                    || name == ".guard.yml"
                    || name == ".guard.yaml"
                    // Sibling Rust convention: `<stem>.guard.yml` next to
                    // the folder they protect.
                    || name.contains(".guard.")
            })
            .unwrap_or(false)
    }

    /// True for `.guard` / `.guard.yml` / `.guard.yaml` — the in-folder
    /// Java convention that keys off the containing directory rather than
    /// a filename stem.
    fn is_in_folder_guard(name: &str) -> bool {
        name == ".guard" || name == ".guard.yml" || name == ".guard.yaml"
    }

    fn build_dsl_key(&self, path: &Path, base_path: &Path, method: &str) -> Result<String> {
        let rel_path = path
            .strip_prefix(base_path)
            .map_err(|_| RuuterError::FileNotFound("Invalid path".to_string()))?;

        let path_str = rel_path
            .with_extension("")
            .to_str()
            .ok_or_else(|| RuuterError::FileNotFound("Invalid path string".to_string()))?
            .replace('\\', "/");

        Ok(format!("{}/{}", method, path_str))
    }

    /// Build a guard key. Two conventions are supported:
    ///
    /// - **Sibling** (Rust convention): `<method>/path/<stem>.guard.yml`
    ///   → key `<METHOD>/path/<stem>` (protects every DSL under the
    ///   same-named sibling folder).
    /// - **In-folder** (Java parity): `<method>/path/<dir>/.guard` (or
    ///   `.guard.yml` / `.guard.yaml`) → key `<METHOD>/path/<dir>`
    ///   (protects everything in and under `<dir>`).
    ///
    /// Both shapes produce the same `<METHOD>/path/<stem>` key so
    /// `applicable_guards`'s prefix match works identically for both.
    fn build_guard_key(&self, path: &Path, base_path: &Path, method: &str) -> Result<String> {
        let rel_path = path
            .strip_prefix(base_path)
            .map_err(|_| RuuterError::FileNotFound("Invalid path".to_string()))?;
        let file_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| RuuterError::FileNotFound("Invalid guard name".into()))?;

        let parent = rel_path.parent().and_then(|p| p.to_str()).unwrap_or("");
        let dir_part = parent.replace('\\', "/");

        // In-folder guard — key = containing directory. `parent` is what
        // the guard protects; nothing to append. When placed directly
        // under the method root (`POST/.guard.yml`) the key is just the
        // method (no trailing slash) so the prefix match works.
        if Self::is_in_folder_guard(file_name) {
            return if dir_part.is_empty() {
                Ok(method.to_string())
            } else {
                Ok(format!("{}/{}", method, dir_part))
            };
        }

        // Sibling guard — strip the trailing `.guard.<ext>` suffix from
        // the filename to get the protected sibling-folder stem.
        let stem = file_name
            .rsplitn(3, '.')
            .nth(2)
            .ok_or_else(|| RuuterError::FileNotFound("Malformed guard filename".into()))?;
        let path_str = if dir_part.is_empty() {
            stem.to_string()
        } else {
            format!("{}/{}", dir_part, stem)
        };
        Ok(format!("{}/{}", method, path_str))
    }
}
