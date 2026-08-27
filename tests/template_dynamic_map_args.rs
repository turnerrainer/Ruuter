//! Regression contract: `template` step `body:`, `query:`, and
//! `headers:` accept both a YAML mapping AND a single `${expr}`
//! string that evaluates to an object at runtime. The parse-time
//! type is `Option<Value>`; the shape is validated when the step
//! runs.

use ruuter_on_rust::config::AppConfig;
use ruuter_on_rust::dsl::loader::DslLoader;
use ruuter_on_rust::dsl::parser::DslParser;
use ruuter_on_rust::http_client::HttpClient;
use ruuter_on_rust::router::DslRouter;
use ruuter_on_rust::state::StateStore;
use ruuter_on_rust::steps::engine::StepEngine;
use ruuter_on_rust::ws::WsRegistry;
use std::collections::HashMap;
use std::sync::Arc;

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

fn build(files: &[(&str, &str)]) -> DslRouter {
    let tmp = std::env::temp_dir().join(format!("ruuter-tpl-dyn-{}", uuid()));
    for (rel, body) in files {
        let p = tmp.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, *body).unwrap();
    }
    let mut cfg = AppConfig::default();
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

// ============================================================================
// 1. Parse-time: dynamic map expressions must not fail schema validation.
// ============================================================================

#[test]
fn template_body_as_expr_parses_at_load() {
    let parser = DslParser::new(HashMap::new());
    let dsl = parser
        .parse_content(
            r#"
call:
  template: templates/x
  request_type: POST
  body: "${followup_json.response.body}"
  result: r
  next: reply
reply:
  return: "ok"
  status: 200
"#,
        )
        .expect("parse must succeed for body: '${expr}'");
    let step = dsl.get_step("call").expect("call step");
    let json = serde_json::to_value(step).unwrap();
    assert_eq!(
        json["body"],
        serde_json::json!("${followup_json.response.body}")
    );
}

#[test]
fn template_query_as_expr_parses_at_load() {
    let parser = DslParser::new(HashMap::new());
    let dsl = parser
        .parse_content(
            r#"
call:
  template: templates/x
  request_type: GET
  query: "${merged_query}"
  result: r
  next: reply
reply:
  return: "ok"
  status: 200
"#,
        )
        .expect("parse must succeed for query: '${expr}'");
    let step = dsl.get_step("call").expect("call step");
    let json = serde_json::to_value(step).unwrap();
    assert_eq!(json["query"], serde_json::json!("${merged_query}"));
}

#[test]
fn template_headers_as_expr_parses_at_load() {
    let parser = DslParser::new(HashMap::new());
    let dsl = parser
        .parse_content(
            r#"
call:
  template: templates/x
  request_type: GET
  headers: "${incoming_hdrs}"
  result: r
  next: reply
reply:
  return: "ok"
  status: 200
"#,
        )
        .expect("parse must succeed for headers: '${expr}'");
    let step = dsl.get_step("call").expect("call step");
    let json = serde_json::to_value(step).unwrap();
    assert_eq!(json["headers"], serde_json::json!("${incoming_hdrs}"));
}

#[test]
fn template_mapping_shape_still_parses() {
    // Backwards compat with the pre-fix mapping form.
    let parser = DslParser::new(HashMap::new());
    let dsl = parser
        .parse_content(
            r#"
call:
  template: templates/x
  request_type: POST
  body:
    name: "alice"
    role: "${dyn_role}"
  headers:
    X-Foo: "bar"
  result: r
  next: reply
reply:
  return: "ok"
  status: 200
"#,
        )
        .expect("parse");
    let step = dsl.get_step("call").expect("call step");
    let json = serde_json::to_value(step).unwrap();
    assert_eq!(json["body"]["name"], "alice");
    assert_eq!(json["body"]["role"], "${dyn_role}");
    assert_eq!(json["headers"]["X-Foo"], "bar");
}

// ============================================================================
// 2. Runtime: `${expr}` shapes are evaluated and forwarded to the callee.
// ============================================================================

#[tokio::test]
async fn template_body_from_expr_forwards_evaluated_object() {
    let router = build(&[
        (
            "svc/POST/templates/create.yml",
            r#"
respond:
  return: { title: "${incoming.body.title}", tag: "${incoming.body.tag}" }
  status: 201
  next: end
"#,
        ),
        (
            "svc/POST/caller.yml",
            r#"
prep:
  assign:
    payload:
      title: "hello"
      tag: "world"
  next: fwd
fwd:
  template: templates/create
  request_type: POST
  body: "${payload}"
  result: out
  next: shape
shape:
  return: { echoed: "${out}" }
  status: 200
  next: end
"#,
        ),
    ]);

    let r = router
        .execute_dsl(
            "svc",
            "POST",
            "caller",
            HashMap::new(),
            HashMap::new(),
            HashMap::new(),
            "t".into(),
        )
        .await
        .unwrap();
    assert_eq!(r.status, 200);
    let body = r.value.unwrap();
    assert_eq!(body["echoed"]["title"], "hello");
    assert_eq!(body["echoed"]["tag"], "world");
}

#[tokio::test]
async fn template_query_from_expr_forwards_evaluated_object() {
    let router = build(&[
        (
            "svc/GET/templates/qview.yml",
            r#"
respond:
  return: { seen: "${incoming.params.q}" }
  status: 200
  next: end
"#,
        ),
        (
            "svc/GET/qcaller.yml",
            r#"
prep:
  assign:
    q_bag:
      q: "abc"
  next: fwd
fwd:
  template: templates/qview
  request_type: GET
  query: "${q_bag}"
  result: out
  next: shape
shape:
  return: { got: "${out.seen}" }
  status: 200
  next: end
"#,
        ),
    ]);
    let r = router
        .execute_dsl(
            "svc",
            "GET",
            "qcaller",
            HashMap::new(),
            HashMap::new(),
            HashMap::new(),
            "t".into(),
        )
        .await
        .unwrap();
    assert_eq!(r.status, 200);
    assert_eq!(r.value.unwrap()["got"], "abc");
}

#[tokio::test]
async fn template_headers_from_expr_forwards_evaluated_object_as_strings() {
    // Headers get coerced to Strings for the child context (matches
    // Java's `convertMapObjectValuesToString`).
    let router = build(&[
        (
            "svc/GET/templates/hview.yml",
            r#"
respond:
  return: { auth: "${incoming.headers['x-auth']}" }
  status: 200
  next: end
"#,
        ),
        (
            "svc/GET/hcaller.yml",
            r#"
prep:
  assign:
    hdrs:
      x-auth: "token-42"
  next: fwd
fwd:
  template: templates/hview
  request_type: GET
  headers: "${hdrs}"
  result: out
  next: shape
shape:
  return: { got: "${out.auth}" }
  status: 200
  next: end
"#,
        ),
    ]);
    let r = router
        .execute_dsl(
            "svc",
            "GET",
            "hcaller",
            HashMap::new(),
            HashMap::new(),
            HashMap::new(),
            "t".into(),
        )
        .await
        .unwrap();
    assert_eq!(r.status, 200);
    assert_eq!(r.value.unwrap()["got"], "token-42");
}

// ============================================================================
// 3. Runtime: mapping shape still works end-to-end (regression fence).
// ============================================================================

#[tokio::test]
async fn template_body_mapping_still_works() {
    let router = build(&[
        (
            "svc/POST/templates/create.yml",
            r#"
respond:
  return: { title: "${incoming.body.title}" }
  status: 201
  next: end
"#,
        ),
        (
            "svc/POST/caller.yml",
            r#"
fwd:
  template: templates/create
  request_type: POST
  body:
    title: "static-title"
  result: out
  next: shape
shape:
  return: { echoed: "${out.title}" }
  status: 200
  next: end
"#,
        ),
    ]);
    let r = router
        .execute_dsl(
            "svc",
            "POST",
            "caller",
            HashMap::new(),
            HashMap::new(),
            HashMap::new(),
            "t".into(),
        )
        .await
        .unwrap();
    assert_eq!(r.status, 200);
    assert_eq!(r.value.unwrap()["echoed"], "static-title");
}

// ============================================================================
// 4. Diagnostics: non-object expression must be a clear runtime error, not a
//    parse failure or an opaque crash.
// ============================================================================

#[tokio::test]
async fn template_body_non_object_expr_errors_with_diagnostic() {
    let router = build(&[
        (
            "svc/POST/templates/x.yml",
            r#"
respond:
  return: "ok"
  status: 200
  next: end
"#,
        ),
        (
            "svc/POST/caller.yml",
            r#"
prep:
  assign:
    payload: "not-a-map"
  next: fwd
fwd:
  template: templates/x
  request_type: POST
  body: "${payload}"
  result: out
  next: end
"#,
        ),
    ]);
    let err = router
        .execute_dsl(
            "svc",
            "POST",
            "caller",
            HashMap::new(),
            HashMap::new(),
            HashMap::new(),
            "t".into(),
        )
        .await
        .expect_err("non-object body expression must surface as an error");
    // The outer message is the step-context wrapper ("step 'X'
    // (template) in project 'svc' failed"). The specific
    // must-be-an-object diagnostic lives on the underlying cause;
    // walk the source chain to find it.
    let mut cause: Option<&dyn std::error::Error> = Some(&err);
    let mut collected = String::new();
    while let Some(e) = cause {
        collected.push_str(&e.to_string());
        collected.push('\n');
        cause = e.source();
    }
    assert!(
        collected.contains("template")
            && collected.contains("body")
            && collected.contains("object"),
        "expected cause-chain naming template.body must-be-object, got:\n{collected}"
    );
}

// ============================================================================
// 5. Composition: `${expr}` shape + object with `undefined` property must
//    compose cleanly. Undefined properties drop per JS spec — no panic, no
//    "expected object" diagnostic, no phantom key on the callee side.
// ============================================================================

#[tokio::test]
async fn template_body_from_object_assign_with_undefined_drops_missing_field() {
    let router = build(&[
        (
            "svc/POST/templates/create.yml",
            r#"
respond:
  return: { title: "${incoming.body.title}", tag: "${incoming.body.tag}" }
  status: 200
  next: end
"#,
        ),
        (
            "svc/POST/caller.yml",
            r#"
fwd:
  template: templates/create
  request_type: POST
  body: "${Object.assign({title: 'hello'}, {tag: undefined})}"
  result: out
  next: shape
shape:
  return: { echoed_title: "${out.title}", echoed_tag: "${out.tag}" }
  status: 200
  next: end
"#,
        ),
    ]);
    let r = router
        .execute_dsl(
            "svc",
            "POST",
            "caller",
            HashMap::new(),
            HashMap::new(),
            HashMap::new(),
            "t".into(),
        )
        .await
        .unwrap();
    assert_eq!(r.status, 200);
    let body = r.value.unwrap();
    assert_eq!(body["echoed_title"], "hello");
    // `tag` was undefined → dropped from the merged object → callee saw
    // no `incoming.body.tag` → its return echoes null.
    assert_eq!(body["echoed_tag"], serde_json::Value::Null);
}

#[tokio::test]
async fn template_query_from_object_assign_with_undefined_drops_missing_param() {
    let router = build(&[
        (
            "svc/GET/templates/qview.yml",
            r#"
respond:
  return: { q: "${incoming.params.q}", opt: "${incoming.params.opt}" }
  status: 200
  next: end
"#,
        ),
        (
            "svc/GET/qcaller.yml",
            r#"
fwd:
  template: templates/qview
  request_type: GET
  query: "${Object.assign({q: 'abc'}, {opt: undefined})}"
  result: out
  next: shape
shape:
  return: { got_q: "${out.q}", got_opt: "${out.opt}" }
  status: 200
  next: end
"#,
        ),
    ]);
    let r = router
        .execute_dsl(
            "svc",
            "GET",
            "qcaller",
            HashMap::new(),
            HashMap::new(),
            HashMap::new(),
            "t".into(),
        )
        .await
        .unwrap();
    assert_eq!(r.status, 200);
    let body = r.value.unwrap();
    assert_eq!(body["got_q"], "abc");
    assert_eq!(body["got_opt"], serde_json::Value::Null);
}

#[tokio::test]
async fn template_headers_from_object_assign_with_undefined_drops_missing_header() {
    // Full end-to-end proof for the issue #34 shape carried through the
    // template step: `Object.assign(base, { 'x-dyn': undefined })` for
    // `headers:` must not surface the phantom key to the callee.
    let router = build(&[
        (
            "svc/GET/templates/hview.yml",
            r#"
respond:
  return: { static: "${incoming.headers['x-static']}", dyn: "${incoming.headers['x-dyn']}" }
  status: 200
  next: end
"#,
        ),
        (
            "svc/GET/hcaller.yml",
            r#"
fwd:
  template: templates/hview
  request_type: GET
  headers: "${Object.assign({'x-static': 'yes'}, {'x-dyn': undefined})}"
  result: out
  next: shape
shape:
  return: { got_static: "${out.static}", got_dyn: "${out.dyn}" }
  status: 200
  next: end
"#,
        ),
    ]);
    let r = router
        .execute_dsl(
            "svc",
            "GET",
            "hcaller",
            HashMap::new(),
            HashMap::new(),
            HashMap::new(),
            "t".into(),
        )
        .await
        .unwrap();
    assert_eq!(r.status, 200);
    let body = r.value.unwrap();
    assert_eq!(body["got_static"], "yes");
    assert_eq!(body["got_dyn"], serde_json::Value::Null);
}
