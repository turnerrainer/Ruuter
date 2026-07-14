//! Integration tests for `[#KEY]` constants handling — both the
//! `constants.ini` loader and its substitution into DSL parse-time
//! and WS source-config paths.

use ruuter_rs::config::{load_constants, AppConfig};
use ruuter_rs::dsl::loader::DslLoader;
use ruuter_rs::sources::config::{resolve_constants, DispatchConfig, ReconnectPolicy, WsSourceConfig};
use std::collections::HashMap;

fn uuid() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    format!("{}", SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos())
}

#[test]
fn loader_parses_sectioned_ini_file() {
    let tmp = std::env::temp_dir().join(format!("ruuter-constants-{}.ini", uuid()));
    std::fs::write(
        &tmp,
        "# a comment\n\n[DSL]\nDOMAIN_URL=https://example.com\nPORT=8080\n\n[other]\nSECRET=hunter2\n",
    )
    .unwrap();

    let c = load_constants(tmp.to_str().unwrap()).expect("load");
    std::fs::remove_file(&tmp).ok();

    // Section headers are documented as decorative — keys are flat.
    assert_eq!(c.get("DOMAIN_URL"), Some(&"https://example.com".to_string()));
    assert_eq!(c.get("PORT"), Some(&"8080".to_string()));
    assert_eq!(c.get("SECRET"), Some(&"hunter2".to_string()));
    assert_eq!(c.len(), 3);
}

#[test]
fn loader_skips_comments_and_blank_lines() {
    let tmp = std::env::temp_dir().join(format!("ruuter-constants-cmt-{}.ini", uuid()));
    std::fs::write(&tmp, "\n\n# lorem\n#ipsum\nOK=1\n\n#trailing\n").unwrap();

    let c = load_constants(tmp.to_str().unwrap()).expect("load");
    std::fs::remove_file(&tmp).ok();

    assert_eq!(c.len(), 1);
    assert_eq!(c.get("OK"), Some(&"1".to_string()));
}

#[test]
fn loader_errors_on_missing_file() {
    let missing = std::env::temp_dir().join(format!("ruuter-constants-nope-{}.ini", uuid()));
    let result = load_constants(missing.to_str().unwrap());
    assert!(result.is_err(), "missing file should surface an error");
}

#[test]
fn dsl_parser_substitutes_constants_at_parse_time() {
    let tmp_root = std::env::temp_dir().join(format!("ruuter-constants-dsl-{}", uuid()));
    let proj = tmp_root.join("svc").join("GET");
    std::fs::create_dir_all(&proj).unwrap();
    let dsl_body = "
respond:
  return: { url: \"[#DOMAIN_URL]/ping\" }
  status: 200
";
    std::fs::write(proj.join("hello.yml"), dsl_body).unwrap();

    let mut cfg = AppConfig::default();
    cfg.config_path = tmp_root.clone();
    let mut constants = HashMap::new();
    constants.insert("DOMAIN_URL".to_string(), "https://api.example.com".to_string());
    let loader = DslLoader::new(cfg, constants);
    let loaded = loader.load_everything().expect("load");

    // Round-trip: the substituted URL should appear in the parsed DSL.
    let dsl = loaded
        .http
        .get("svc")
        .and_then(|m| m.get("GET"))
        .and_then(|d| d.get("GET/hello"))
        .expect("dsl present");

    // The return step should contain the substituted URL literal.
    let json = serde_json::to_string(&dsl.steps).unwrap();
    assert!(
        json.contains("https://api.example.com/ping"),
        "substituted URL missing from parsed DSL. Got: {}",
        json
    );
    assert!(
        !json.contains("[#DOMAIN_URL]"),
        "unsubstituted marker still present"
    );
}

#[test]
fn ws_source_resolve_constants_errors_on_missing_key() {
    let cfg = WsSourceConfig {
        url: "[#missing_key]".into(),
        headers: HashMap::new(),
        on_connect: vec![],
        dispatch: DispatchConfig { channel: "$.T".into(), key: "$.S".into() },
        reconnect: ReconnectPolicy::default(),
    };
    let result = resolve_constants(&cfg, &HashMap::new());
    assert!(result.is_err(), "unknown constant must be an error, not silent passthrough");
    let msg = format!("{}", result.err().unwrap());
    assert!(msg.contains("missing_key"), "error should name the offending key: {}", msg);
}

#[test]
fn ws_source_resolve_constants_substitutes_headers_and_url() {
    let mut headers = HashMap::new();
    headers.insert("X-API-Key".into(), "[#api_key]".into());
    let cfg = WsSourceConfig {
        url: "[#ws_url]".into(),
        headers,
        on_connect: vec![],
        dispatch: DispatchConfig { channel: "$.T".into(), key: "$.S".into() },
        reconnect: ReconnectPolicy::default(),
    };
    let mut constants = HashMap::new();
    constants.insert("ws_url".to_string(), "wss://feed.example.com".to_string());
    constants.insert("api_key".to_string(), "s3cret".to_string());

    let out = resolve_constants(&cfg, &constants).expect("resolve");
    assert_eq!(out.url, "wss://feed.example.com");
    assert_eq!(out.headers.get("X-API-Key"), Some(&"s3cret".to_string()));
}
