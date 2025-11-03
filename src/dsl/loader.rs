use crate::config::AppConfig;
use crate::dsl::{parser::DslParser, Dsl};
use crate::{Result, RuuterError};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

pub struct DslLoader {
    config: AppConfig,
    constants: HashMap<String, String>,
}

impl DslLoader {
    pub fn new(config: AppConfig, constants: HashMap<String, String>) -> Self {
        Self { config, constants }
    }

    pub fn load_all(&self) -> Result<HashMap<String, HashMap<String, HashMap<String, Dsl>>>> {
        let mut projects = HashMap::new();

        for entry in fs::read_dir(&self.config.config_path)? {
            let entry = entry?;
            let path = entry.path();

            if path.is_dir() {
                let project_name = path.file_name()
                    .and_then(|n| n.to_str())
                    .ok_or_else(|| RuuterError::FileNotFound("Invalid project name".to_string()))?
                    .to_string();

                let methods = self.load_project(&path)?;
                projects.insert(project_name, methods);
            }
        }

        Ok(projects)
    }

    fn load_project(&self, project_path: &Path) -> Result<HashMap<String, HashMap<String, Dsl>>> {
        let mut methods = HashMap::new();

        for entry in fs::read_dir(project_path)? {
            let entry = entry?;
            let path = entry.path();

            if path.is_dir() {
                let method = path.file_name()
                    .and_then(|n| n.to_str())
                    .ok_or_else(|| RuuterError::FileNotFound("Invalid method name".to_string()))?
                    .to_string();

                let dsls = self.load_method_dsls(&path, &method)?;
                methods.insert(method, dsls);
            }
        }

        Ok(methods)
    }

    fn load_method_dsls(&self, method_path: &Path, method: &str) -> Result<HashMap<String, Dsl>> {
        let mut dsls = HashMap::new();
        let parser = DslParser::new(self.constants.clone());

        self.scan_directory(method_path, method_path, method, &parser, &mut dsls)?;

        Ok(dsls)
    }

    fn scan_directory(
        &self,
        dir: &Path,
        base_path: &Path,
        method: &str,
        parser: &DslParser,
        dsls: &mut HashMap<String, Dsl>,
    ) -> Result<()> {
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();

            if path.is_dir() {
                self.scan_directory(&path, base_path, method, parser, dsls)?;
            } else if self.is_processable_file(&path) && !self.is_guard_file(&path) {
                let key = self.build_dsl_key(&path, base_path, method)?;
                let dsl = parser.parse_file(&path)?;
                dsls.insert(key, dsl);
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
}
