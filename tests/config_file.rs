//! Integration test for `AppConfig::load_or_default` — covers the
//! priority order (env → file → default) and proves a real config file
//! is actually consulted at boot.

use ruuter_rs::config::AppConfig;
use std::sync::Mutex;

fn uuid() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    format!(
        "{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )
}

// `AppConfig::load_or_default` reads process-wide `std::env::args`
// and `std::env::var`. Serialise the tests that mutate env so they
// don't stomp on each other.
static ENV_LOCK: Mutex<()> = Mutex::new(());

#[test]
fn default_when_no_file_exists() {
    let _g = ENV_LOCK.lock().unwrap();
    std::env::remove_var("RUUTER_CONFIG");
    // Guarantee no ruuter.yaml in cwd — chdir to a tmp dir.
    let tmp = std::env::temp_dir().join(format!("ruuter-cfg-empty-{}", uuid()));
    std::fs::create_dir_all(&tmp).unwrap();
    let orig = std::env::current_dir().unwrap();
    std::env::set_current_dir(&tmp).unwrap();

    let (cfg, source) = AppConfig::load_or_default().expect("load");
    std::env::set_current_dir(orig).unwrap();

    assert!(source.is_none(), "no file should have been found");
    assert_eq!(cfg.port, 8080);
    assert!(cfg.cors.allowed_origins.is_empty());
}

#[test]
fn env_var_points_at_file() {
    let _g = ENV_LOCK.lock().unwrap();
    let tmp = std::env::temp_dir().join(format!("ruuter-cfg-{}.yaml", uuid()));
    std::fs::write(
        &tmp,
        "port: 9090\ncors:\n  allowed_origins: [\"https://x.example.com\"]\n",
    )
    .unwrap();

    std::env::set_var("RUUTER_CONFIG", &tmp);
    let (cfg, source) = AppConfig::load_or_default().expect("load");
    std::env::remove_var("RUUTER_CONFIG");

    assert_eq!(source, Some(tmp.clone()));
    assert_eq!(cfg.port, 9090);
    assert_eq!(cfg.cors.allowed_origins, vec!["https://x.example.com"]);
    // Fields not present in the file inherit AppConfig::default().
    assert_eq!(cfg.http_request_timeout, 15000);

    std::fs::remove_file(&tmp).ok();
}

#[test]
fn malformed_yaml_is_a_hard_error() {
    let _g = ENV_LOCK.lock().unwrap();
    let tmp = std::env::temp_dir().join(format!("ruuter-cfg-bad-{}.yaml", uuid()));
    std::fs::write(&tmp, ":\n  not: valid: yaml: at: all:\n").unwrap();

    std::env::set_var("RUUTER_CONFIG", &tmp);
    let result = AppConfig::load_or_default();
    std::env::remove_var("RUUTER_CONFIG");
    std::fs::remove_file(&tmp).ok();

    assert!(result.is_err(), "malformed config should not fall back to defaults silently");
}

#[test]
fn shipped_example_config_parses_cleanly() {
    // Prove the DSL/samples/ruuter.yaml.example file mentioned in the
    // README actually parses — otherwise operators copying it get a
    // startup panic and lose trust in the docs.
    let _g = ENV_LOCK.lock().unwrap();
    let path = std::path::PathBuf::from("DSL/samples/ruuter.yaml.example");
    if !path.exists() {
        // Test runs from crate root under `cargo test`; skip silently if
        // invoked from an unusual cwd.
        return;
    }
    std::env::set_var("RUUTER_CONFIG", &path);
    let result = AppConfig::load_or_default();
    std::env::remove_var("RUUTER_CONFIG");
    let (cfg, source) = result.expect("example config must parse");
    assert!(source.is_some());
    // Sanity: sample must not silently disable outbound HTTP.
    assert!(!cfg.internal_requests.disabled);
}
