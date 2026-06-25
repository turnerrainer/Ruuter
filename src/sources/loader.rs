//! Scan `<DSL>/<project>/sources/*.yml` and parse each into a SourceConfig.

use crate::config::AppConfig;
use crate::sources::config::SourceConfig;
use crate::{Result, RuuterError};
use std::fs;
use std::path::Path;

/// Returns a list of (project, source_name, parsed_config) tuples.
pub fn load_all(app: &AppConfig) -> Result<Vec<(String, String, SourceConfig)>> {
    let mut out = Vec::new();
    let root = &app.config_path;

    if !root.exists() {
        return Ok(out);
    }

    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let project_path = entry.path();
        if !project_path.is_dir() {
            continue;
        }
        let project_name = project_path.file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| RuuterError::FileNotFound("Invalid project name".into()))?
            .to_string();

        let sources_dir = project_path.join("sources");
        if !sources_dir.exists() {
            continue;
        }
        load_project_sources(&sources_dir, &project_name, &mut out)?;
    }
    Ok(out)
}

fn load_project_sources(
    dir: &Path,
    project: &str,
    out: &mut Vec<(String, String, SourceConfig)>,
) -> Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let ext_ok = matches!(
            path.extension().and_then(|e| e.to_str()),
            Some("yml") | Some("yaml")
        );
        if !ext_ok {
            continue;
        }
        let name = path.file_stem()
            .and_then(|n| n.to_str())
            .ok_or_else(|| RuuterError::FileNotFound("Invalid source name".into()))?
            .to_string();

        let body = fs::read_to_string(&path)?;
        let cfg: SourceConfig = serde_yaml::from_str(&body)
            .map_err(|e| RuuterError::DslParse(format!(
                "source {}/{}: {}", project, name, e
            )))?;
        out.push((project.to_string(), name, cfg));
    }
    Ok(())
}
