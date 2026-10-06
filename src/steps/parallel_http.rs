//! Issues #135 + #136 — bounded concurrent fan-out to N peer gates
//! with structured aggregation.
//!
//! Replaces the Klite-multiplexer pattern retired by K4
//! (kemit-ee/efti-gate-ee#252). The primitive is deliberately generic
//! — it fans out HTTP calls to a dynamic list of peers under a
//! configurable concurrency cap, aggregates per three modes
//! (`collect_ok` / `collect_all` / `first_n`), and binds a structured
//! array of `{peer, response}` records to the DSL. The eFTI Gate's
//! own replacement for Klite's multiplexer composes this step inside
//! a `detach` step (#137) that writes the result array to Postgres
//! via Resql — see `book/src/dsl/steps/parallel_http.md`.
//!
//! The executor:
//!   1. Evaluates `peers:` to an array.
//!   2. Pre-evaluates `args.url` / `args.body` / `args.headers` /
//!      `args.query` once per peer in the parent context, with
//!      `${peer}` temporarily bound (same model as `iterate.as`).
//!      `traceparent` is auto-forwarded from the parent context.
//!   3. Spawns one `tokio::task` per peer, bounded by a
//!      `tokio::sync::Semaphore` sized at `max_concurrency`.
//!   4. Each task calls `HttpClient::request_with_ct` directly —
//!      same SSRF / allowlist / DNS-pinning / `#89` transport-error
//!      contract as the `http.*` step, but the step-level
//!      `error:` handler and `default_dsl_in_case_of_exception`
//!      do not apply (a parallel fan-out has its own aggregate
//!      semantics; one bad peer must not abort the step).
//!   5. Collects results per `aggregate` mode. `first_n`'s
//!      `remaining_peers_after` policy decides what happens to
//!      still-in-flight tasks when the quota is met.

use crate::context::ExecutionContext;
use crate::scripting::ScriptEngine;
use crate::steps::engine::StepEngine;
use crate::steps::http::evaluate_map_arg;
use crate::steps::{
    AggregateMode, EarlyExitOn, ParallelHttpStep, RemainingPeersAfter, StepExecutor, StepLogExtras,
    StepResult,
};
use crate::{Result, RuuterError};
use reqwest::Method;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

pub struct ParallelHttpStepExecutor {
    step: ParallelHttpStep,
    engine: StepEngine,
    script_engine: ScriptEngine,
}

impl ParallelHttpStepExecutor {
    pub fn new(step: ParallelHttpStep, engine: StepEngine) -> Self {
        Self {
            step,
            engine,
            script_engine: ScriptEngine::new(),
        }
    }

    fn parse_method(call: &str) -> Result<Method> {
        match call {
            "http.get" => Ok(Method::GET),
            "http.post" => Ok(Method::POST),
            "http.put" => Ok(Method::PUT),
            "http.patch" => Ok(Method::PATCH),
            "http.delete" => Ok(Method::DELETE),
            other => Err(RuuterError::InvalidStep(format!(
                "parallel_http.call unknown method: {} (expected http.get / http.post / http.put / http.patch / http.delete)",
                other
            ))),
        }
    }
}

impl StepExecutor for ParallelHttpStepExecutor {
    async fn execute(&self, context: &ExecutionContext) -> Result<StepResult> {
        let body = &self.step.parallel_http;

        // (1) Peers → array.
        let peers_val = self
            .script_engine
            .evaluate(&Value::String(body.peers.clone()), context)?;
        let peers = match peers_val {
            Value::Array(arr) => arr,
            Value::Null => Vec::new(),
            other => {
                return Err(RuuterError::InvalidStep(format!(
                    "parallel_http.peers must evaluate to an array, got {}",
                    kind_of(&other)
                )));
            }
        };

        if peers.is_empty() {
            // Empty peers list: nothing to fan out. Bind an empty
            // array to `result` so downstream `${result}` consumers
            // don't `undefined`-trap, log the no-op, and advance.
            context.set_variable(body.result.clone(), Value::Array(Vec::new()));
            let extras = StepLogExtras::new()
                .push("peers", 0u64)
                .push("aggregate", agg_name(body.aggregate));
            return Ok(StepResult {
                next_step: self.step.next.clone(),
                log_extras: extras,
                ..StepResult::new()
            });
        }

        let method = Self::parse_method(&body.call)?;
        let timeout = body.timeout.map(Duration::from_millis);
        let content_type = body.args.content_type.clone();
        let traceparent = context.traceparent().map(String::from);
        let peer_count = peers.len();

        // (2) Pre-evaluate per-peer args in the parent context. Same
        // `${peer}` binding model as `iterate.as` — bind, evaluate,
        // move on. The last-peer binding leaks to subsequent steps
        // (parity with iterate's item_var).
        let mut resolved_args: Vec<ResolvedPeerArgs> = Vec::with_capacity(peer_count);
        for peer in &peers {
            context.set_variable("peer".to_string(), peer.clone());

            let url_val = self
                .script_engine
                .evaluate(&Value::String(body.args.url.clone()), context)?;
            let url = url_val.as_str().unwrap_or("").to_string();

            let req_body = if let Some(b) = &body.args.body {
                Some(self.script_engine.evaluate(b, context)?)
            } else {
                None
            };

            let query = evaluate_map_arg(
                body.args.query.as_ref(),
                &self.script_engine,
                context,
                "parallel_http",
                "query",
            )?;

            let mut headers = evaluate_map_arg(
                body.args.headers.as_ref(),
                &self.script_engine,
                context,
                "parallel_http",
                "headers",
            )?
            .unwrap_or_default();

            // Auto-forward traceparent unless the DSL already set one
            // per-peer. Mirrors the http step at src/steps/http.rs:83–90.
            if !headers
                .keys()
                .any(|k| k.eq_ignore_ascii_case("traceparent"))
            {
                if let Some(tp) = &traceparent {
                    headers.insert("traceparent".to_string(), Value::String(tp.clone()));
                }
            }

            resolved_args.push(ResolvedPeerArgs {
                peer: peer.clone(),
                url,
                body: req_body,
                query: query.unwrap_or_default(),
                headers,
            });
        }

        // (3) Spawn tasks, bounded by Semaphore.
        let max_concurrency = body
            .max_concurrency
            .map(|n| n.max(1) as usize)
            .unwrap_or(peer_count);
        let semaphore = Arc::new(Semaphore::new(max_concurrency));
        let http_client = self.engine.http_client().clone();

        let mut tasks: JoinSet<(usize, Value, std::result::Result<HttpResponseVal, String>)> =
            JoinSet::new();
        for (idx, args) in resolved_args.into_iter().enumerate() {
            let semaphore = semaphore.clone();
            let http_client = http_client.clone();
            let method = method.clone();
            let content_type = content_type.clone();
            tasks.spawn(async move {
                // Permit acquired INSIDE the task so the join order
                // reflects completion order, not spawn order.
                let _permit = match semaphore.acquire_owned().await {
                    Ok(p) => p,
                    Err(e) => {
                        return (idx, args.peer, Err(format!("semaphore closed: {}", e)));
                    }
                };
                let response = http_client
                    .request_with_ct(
                        method,
                        &args.url,
                        args.body.as_ref(),
                        if args.query.is_empty() {
                            None
                        } else {
                            Some(&args.query)
                        },
                        if args.headers.is_empty() {
                            None
                        } else {
                            Some(&args.headers)
                        },
                        timeout,
                        content_type.as_deref(),
                    )
                    .await;
                match response {
                    Ok(resp) => (idx, args.peer, Ok(HttpResponseVal::from(resp))),
                    Err(e) => (idx, args.peer, Err(e.to_string())),
                }
            });
        }

        // (4) Collect per aggregate mode.
        let collected = match body.aggregate {
            AggregateMode::CollectOk => collect_filtered(&mut tasks, true).await,
            AggregateMode::CollectAll => collect_filtered(&mut tasks, false).await,
            AggregateMode::FirstN => {
                collect_first_n(
                    &mut tasks,
                    body.first_n.unwrap_or(1).max(1) as usize,
                    body.early_exit_on.as_ref(),
                    body.remaining_peers_after
                        .unwrap_or(RemainingPeersAfter::Cancel),
                    &self.script_engine,
                    context,
                )
                .await
            }
        };

        // Preserve peer order from the input — tasks finish in
        // completion order; the DSL author expects results aligned
        // to the input `peers:` list.
        let mut ordered: Vec<Value> = collected;
        // ordered is already stored indexed; sort by the `_idx`
        // sentinel inside each record.
        ordered.sort_by_key(|v| v.get("_idx").and_then(|n| n.as_u64()).unwrap_or(u64::MAX));
        for v in &mut ordered {
            if let Value::Object(map) = v {
                map.remove("_idx");
            }
        }

        let yielded = ordered.len() as u64;
        context.set_variable(body.result.clone(), Value::Array(ordered));

        let extras = StepLogExtras::new()
            .push("peers", peer_count as u64)
            .push("aggregate", agg_name(body.aggregate))
            .push("yielded", yielded);
        Ok(StepResult {
            next_step: self.step.next.clone(),
            log_extras: extras,
            ..StepResult::new()
        })
    }
}

struct ResolvedPeerArgs {
    peer: Value,
    url: String,
    body: Option<Value>,
    query: HashMap<String, Value>,
    headers: HashMap<String, Value>,
}

/// Serialisable projection of `HttpResponse` used for the structured
/// result array. Mirrors the shape the DSL already sees today from
/// `http.*` step's `${result.response}` binding.
struct HttpResponseVal {
    status: u16,
    body: Option<Value>,
    headers: HashMap<String, String>,
    error: Option<String>,
}

impl From<crate::http_client::HttpResponse> for HttpResponseVal {
    fn from(r: crate::http_client::HttpResponse) -> Self {
        Self {
            status: r.status,
            body: r.body,
            headers: r.headers,
            error: r.error,
        }
    }
}

impl HttpResponseVal {
    fn to_json(&self) -> Value {
        let mut headers = serde_json::Map::new();
        for (k, v) in &self.headers {
            headers.insert(k.clone(), Value::String(v.clone()));
        }
        json!({
            "status": self.status,
            "body": self.body.clone().unwrap_or(Value::Null),
            "headers": Value::Object(headers),
            "error": self.error.clone().map(Value::String).unwrap_or(Value::Null),
        })
    }

    fn is_transport_error(&self) -> bool {
        self.status == 0 || self.error.is_some()
    }
}

async fn collect_filtered(
    tasks: &mut JoinSet<(usize, Value, std::result::Result<HttpResponseVal, String>)>,
    drop_errors: bool,
) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    while let Some(join_result) = tasks.join_next().await {
        match join_result {
            Ok((idx, peer, Ok(response))) => {
                if drop_errors && response.is_transport_error() {
                    continue;
                }
                out.push(build_peer_entry(idx, peer, response));
            }
            Ok((idx, peer, Err(e))) => {
                if drop_errors {
                    continue;
                }
                // Spawn failure (semaphore closed, etc.) — surface as
                // transport-error-shaped record for stability of the
                // array shape.
                out.push(build_peer_error_entry(idx, peer, &e));
            }
            Err(join_err) => {
                tracing::warn!(error = %join_err, "parallel_http: a peer task panicked");
            }
        }
    }
    out
}

async fn collect_first_n(
    tasks: &mut JoinSet<(usize, Value, std::result::Result<HttpResponseVal, String>)>,
    target: usize,
    predicate: Option<&EarlyExitOn>,
    remaining: RemainingPeersAfter,
    script: &ScriptEngine,
    context: &ExecutionContext,
) -> Vec<Value> {
    let mut matches: Vec<Value> = Vec::new();
    let mut non_matches: Vec<Value> = Vec::new();

    while matches.len() < target {
        let Some(join_result) = tasks.join_next().await else {
            // All tasks finished without hitting the quota. Return what
            // we have — DSL authors can detect a short result via
            // `${result.length < first_n}`.
            break;
        };
        match join_result {
            Ok((idx, peer, Ok(response))) => {
                let counted = predicate_matches(predicate, &response, script, context);
                let entry = build_peer_entry(idx, peer, response);
                if counted {
                    matches.push(entry);
                } else {
                    non_matches.push(entry);
                }
            }
            Ok((idx, peer, Err(e))) => {
                non_matches.push(build_peer_error_entry(idx, peer, &e));
            }
            Err(join_err) => {
                tracing::warn!(error = %join_err, "parallel_http: a peer task panicked");
            }
        }
    }

    // Dispose of still-in-flight tasks per policy.
    match remaining {
        RemainingPeersAfter::Cancel => {
            tasks.abort_all();
            // Drain join handles so JoinSet doesn't error on drop. We
            // don't inspect the results — they're being cancelled.
            while tasks.join_next().await.is_some() {}
        }
        RemainingPeersAfter::DrainBg => {
            // Move the leftover JoinSet into a detached tokio task
            // that drains it in the background. Each eventual peer
            // response is logged via tracing (for audit); the DSL
            // caller returns immediately with just the first_n
            // matches. `std::mem::take` swaps in an empty JoinSet so
            // the caller's `tasks` is cleanly drained before return.
            let mut drained: JoinSet<(usize, Value, std::result::Result<HttpResponseVal, String>)> =
                std::mem::take(tasks);
            tokio::spawn(async move {
                while let Some(join_result) = drained.join_next().await {
                    match join_result {
                        Ok((_idx, peer, Ok(response))) => {
                            tracing::info!(
                                peer = %peer,
                                status = response.status,
                                "parallel_http drain_bg: peer completed"
                            );
                        }
                        Ok((_idx, peer, Err(e))) => {
                            tracing::warn!(
                                peer = %peer,
                                error = %e,
                                "parallel_http drain_bg: peer failed"
                            );
                        }
                        Err(e) => {
                            tracing::warn!(
                                error = %e,
                                "parallel_http drain_bg: task panicked"
                            );
                        }
                    }
                }
            });
        }
    }

    matches
}

fn predicate_matches(
    predicate: Option<&EarlyExitOn>,
    response: &HttpResponseVal,
    script: &ScriptEngine,
    context: &ExecutionContext,
) -> bool {
    // Transport errors never count toward first_n.
    if response.is_transport_error() {
        return false;
    }
    // Status range: default [200, 299] when not set.
    let (lo, hi) = predicate
        .and_then(|p| p.status_range)
        .map(|[l, h]| (l, h))
        .unwrap_or((200, 299));
    if response.status < lo || response.status > hi {
        return false;
    }
    // Body predicate (optional): evaluate against context with
    // `${response}` bound. Non-truthy / error → not a match.
    if let Some(expr) = predicate.and_then(|p| p.body_predicate.as_deref()) {
        // Bind `response` for the predicate evaluation. The leak-after
        // is the same pattern iterate uses for its item_var — a
        // subsequent step reading `${response}` sees the last peer's
        // response; document in the step page.
        context.set_variable("response".to_string(), response.to_json());
        let result = script.evaluate(&Value::String(expr.to_string()), context);
        match result {
            Ok(Value::Bool(b)) => return b,
            Ok(Value::Null) => return false,
            Ok(Value::Number(n)) => return n.as_f64().map(|f| f != 0.0).unwrap_or(false),
            Ok(Value::String(s)) => return !s.is_empty(),
            Ok(_) => return true,
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "parallel_http: first_n body_predicate evaluation failed; peer not counted"
                );
                return false;
            }
        }
    }
    true
}

fn build_peer_entry(idx: usize, peer: Value, response: HttpResponseVal) -> Value {
    json!({
        "_idx": idx,
        "peer": peer,
        "response": response.to_json(),
    })
}

fn build_peer_error_entry(idx: usize, peer: Value, err: &str) -> Value {
    json!({
        "_idx": idx,
        "peer": peer,
        "response": {
            "status": 0,
            "body": Value::Null,
            "headers": Value::Object(serde_json::Map::new()),
            "error": err,
        },
    })
}

fn agg_name(mode: AggregateMode) -> &'static str {
    match mode {
        AggregateMode::CollectOk => "collect_ok",
        AggregateMode::CollectAll => "collect_all",
        AggregateMode::FirstN => "first_n",
    }
}

fn kind_of(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}
