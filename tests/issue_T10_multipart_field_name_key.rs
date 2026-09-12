//! h2ck.me v1 T-10 — multipart `Content-Disposition` filename must
//! not win over the field `name` as the `incoming.body` map key.
//!
//! Pre-fix, `filename.or(name)` in `parse_multipart_body` meant an
//! attacker-controlled filename (path-traversal shape,
//! `../etc/passwd`, Unicode homoglyph, extremely long, empty
//! string) became the key that downstream DSLs read via
//! `${incoming.body.<key>}`. In the framework the key is just a
//! JSON-map key — no fs code touches it — but a DSL author who
//! forwards that key to a trusted system (path building, log line,
//! cache key) inherits whatever nastiness the filename carried.
//!
//! Post-fix: field name wins. Filename is a fallback only when the
//! field has no `name=`. `part` is the final fallback for
//! anonymous fields.
//!
//! Tests written to try to BREAK the fix:
//! - Normal `name="file"; filename="note.txt"` → key is `file`.
//! - Traversal filename with a field name → key is field name, NOT
//!   the traversal string.
//! - Unicode homoglyph filename with a field name → key is field name.
//! - Empty filename `filename=""` with a field name → key is field name.
//! - No filename at all → key is field name.
//! - No field name, filename present → key IS the filename (fallback).
//! - Anonymous field (no name, no filename) → key is "part".
//! - Multiple fields with the same field name — later wins (existing
//!   HashMap semantic; documented, not a T-10 change but a
//!   belts-and-braces pin against a serde change).

#![allow(clippy::field_reassign_with_default)]

use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use ruuter_on_rust::config::AppConfig;
use ruuter_on_rust::dsl::loader::DslLoader;
use ruuter_on_rust::http_client::HttpClient;
use ruuter_on_rust::router::DslRouter;
use ruuter_on_rust::state::StateStore;
use ruuter_on_rust::steps::engine::StepEngine;
use ruuter_on_rust::ws::WsRegistry;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tower::ServiceExt;

fn uuid() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("{}", nanos)
}

/// Build a router with one DSL that echoes the entire `incoming.body`
/// map. Tests inspect the echoed keys directly.
fn build_echo_router() -> DslRouter {
    let mut cfg = AppConfig::default();
    let tmp = std::env::temp_dir().join(format!("ruuter-T10-{}", uuid()));
    let dsl_path = tmp.join("svc/POST/echo.yml");
    std::fs::create_dir_all(dsl_path.parent().unwrap()).unwrap();
    std::fs::write(
        &dsl_path,
        r#"
reply:
  return: "${incoming.body}"
  status: 200
"#,
    )
    .unwrap();
    cfg.config_path = tmp;
    let loader = DslLoader::new(cfg.clone(), HashMap::new());
    let loaded = loader.load_everything().unwrap();
    let ws = WsRegistry::new();
    let shared = Arc::new(loaded.http);
    let engine = StepEngine::new(HttpClient::new(&cfg))
        .with_ws_registry(ws.clone())
        .with_dsls(shared.clone());
    DslRouter::from_arc(shared, loaded.guards, cfg, StateStore::new(), ws, engine)
}

const BOUNDARY: &str = "----RuuterT10Boundary";

/// POST a multipart body of the given raw shape and return the
/// echoed JSON object under `response`.
async fn post_multipart(body: String) -> serde_json::Value {
    let router = build_echo_router();
    let app = router.build_axum_router();
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/svc/echo")
                .header(
                    "content-type",
                    format!("multipart/form-data; boundary={}", BOUNDARY),
                )
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .expect("send");
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = to_bytes(resp.into_body(), 16 * 1024).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    // Response wrapped by default (audit finding 12).
    json["response"].clone()
}

#[tokio::test]
async fn normal_name_and_filename_uses_field_name() {
    let body = format!(
        "--{b}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"note.txt\"\r\n\r\nhello upload\r\n--{b}--\r\n",
        b = BOUNDARY
    );
    let echo = post_multipart(body).await;
    // Field name "file" is the key; filename "note.txt" is NOT.
    assert_eq!(echo["file"], "hello upload");
    assert!(
        echo.get("note.txt").is_none(),
        "filename must not become the map key; got {echo}"
    );
}

#[tokio::test]
async fn traversal_filename_with_field_name_uses_field_name() {
    // Attacker sends `filename="../etc/passwd"`. Pre-fix, this
    // string was the map key — a DSL forwarding it into a path or
    // log line inherits the traversal shape. Post-fix, the stable
    // field name wins.
    let body = format!(
        "--{b}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"../etc/passwd\"\r\n\r\nsecret bytes\r\n--{b}--\r\n",
        b = BOUNDARY
    );
    let echo = post_multipart(body).await;
    assert_eq!(echo["file"], "secret bytes");
    assert!(
        echo.get("../etc/passwd").is_none(),
        "path-traversal filename must not become the map key; got {echo}"
    );
}

#[tokio::test]
async fn unicode_homoglyph_filename_with_field_name_uses_field_name() {
    // fullwidth colon (U+FF1A) + fullwidth slash — attackers use
    // homoglyphs to slip past naive path validators. Post-fix, the
    // filename never becomes the key at all.
    let body = format!(
        "--{b}\r\nContent-Disposition: form-data; name=\"avatar\"; filename=\"a\\uFF1Ab\\uFF0Fc.png\"\r\n\r\navatar bytes\r\n--{b}--\r\n",
        b = BOUNDARY
    );
    let echo = post_multipart(body).await;
    assert_eq!(echo["avatar"], "avatar bytes");
    // Loose check — the homoglyph filename must not appear as a key.
    for k in echo.as_object().unwrap().keys() {
        assert!(
            !k.contains('\u{FF1A}') && !k.contains('\u{FF0F}'),
            "homoglyph filename must not become a map key; found key {k}"
        );
    }
}

#[tokio::test]
async fn empty_filename_with_field_name_uses_field_name() {
    let body = format!(
        "--{b}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"\"\r\n\r\nempty-filename content\r\n--{b}--\r\n",
        b = BOUNDARY
    );
    let echo = post_multipart(body).await;
    // multer may either report filename as Some("") or None — the
    // post-fix semantic is "name wins if present," so `file` is
    // the key either way.
    assert_eq!(echo["file"], "empty-filename content");
}

#[tokio::test]
async fn no_filename_uses_field_name() {
    let body = format!(
        "--{b}\r\nContent-Disposition: form-data; name=\"comment\"\r\n\r\njust a text field\r\n--{b}--\r\n",
        b = BOUNDARY
    );
    let echo = post_multipart(body).await;
    assert_eq!(echo["comment"], "just a text field");
}

#[tokio::test]
async fn no_field_name_falls_back_to_filename() {
    // Pathological but legal: no name= attribute. The fallback
    // reaches for filename. This is the ONLY path where the
    // filename becomes a key — documented, and DSL authors who
    // want to lock this out should require `name=` in their
    // upstream API contract.
    let body = format!(
        "--{b}\r\nContent-Disposition: form-data; filename=\"only-filename.txt\"\r\n\r\nfallback\r\n--{b}--\r\n",
        b = BOUNDARY
    );
    let echo = post_multipart(body).await;
    // Fallback: key is the filename.
    assert_eq!(echo["only-filename.txt"], "fallback");
}

#[tokio::test]
async fn no_name_no_filename_falls_back_to_part() {
    let body = format!(
        "--{b}\r\nContent-Disposition: form-data\r\n\r\ntruly anonymous\r\n--{b}--\r\n",
        b = BOUNDARY
    );
    let echo = post_multipart(body).await;
    assert_eq!(echo["part"], "truly anonymous");
}

#[tokio::test]
async fn multiple_fields_with_same_name_last_wins() {
    // HashMap semantic — the last field with a given name
    // overwrites earlier ones. Not a T-10 fix; belts-and-braces
    // pin against a future refactor that changed collection type.
    let body = format!(
        "--{b}\r\nContent-Disposition: form-data; name=\"tag\"\r\n\r\nfirst\r\n--{b}\r\nContent-Disposition: form-data; name=\"tag\"\r\n\r\nsecond\r\n--{b}--\r\n",
        b = BOUNDARY
    );
    let echo = post_multipart(body).await;
    assert_eq!(echo["tag"], "second");
}
