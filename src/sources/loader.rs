//! Scan `<DSL>/<project>/WS/outbound/*.yml` (preferred) or the legacy
//! `<DSL>/<project>/sources/*.yml` (deprecated) and parse each into a
//! `SourceConfig`. When both directories exist, the new layout wins and
//! the legacy layout is ignored with a WARN.

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
        let project_name = project_path
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| RuuterError::FileNotFound("Invalid project name".into()))?
            .to_string();

        // New canonical layout: <project>/WS/outbound/
        let ws_outbound_dir = project_path.join("WS").join("outbound");
        // Legacy layout: <project>/sources/
        let legacy_sources_dir = project_path.join("sources");

        let (dir_to_load, legacy_in_use) =
            match (ws_outbound_dir.exists(), legacy_sources_dir.exists()) {
                (true, true) => {
                    tracing::warn!(
                        project = %project_name,
                        "both WS/outbound/ and sources/ present; using WS/outbound/ \
                         and ignoring sources/ (rename or delete sources/ to \
                         silence)"
                    );
                    (Some(ws_outbound_dir), false)
                }
                (true, false) => (Some(ws_outbound_dir), false),
                (false, true) => {
                    tracing::warn!(
                        project = %project_name,
                        "sources/ layout is deprecated; rename to WS/outbound/ \
                         to match the inbound WS/inbound/ layout"
                    );
                    (Some(legacy_sources_dir), true)
                }
                (false, false) => (None, false),
            };

        let _ = legacy_in_use;
        if let Some(dir) = dir_to_load {
            load_project_sources(&dir, &project_name, &mut out)?;
        }
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
        let name = path
            .file_stem()
            .and_then(|n| n.to_str())
            .ok_or_else(|| RuuterError::FileNotFound("Invalid source name".into()))?
            .to_string();

        let body = fs::read_to_string(&path)?;
        let cfg: SourceConfig = serde_yaml_ng::from_str(&body)
            .map_err(|e| RuuterError::DslParse(format!("source {}/{}: {}", project, name, e)))?;
        out.push((project.to_string(), name, cfg));
    }
    Ok(())
}
