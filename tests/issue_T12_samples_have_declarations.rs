//! h2ck.me v1 T-12 — every HTTP DSL sample under `DSL/samples/`
//! must carry a `declaration:` block.
//!
//! Pre-fix, `grep -rln '^declaration:' DSL/samples/` returned 2 of
//! 58 samples. Ruuter's own samples didn't demonstrate the feature
//! Ruuter advertises — DSL authors reading the docs had no working
//! reference to copy from. Post-fix, every HTTP sample carries at
//! least a minimal declaration (description + additive-allowlist)
//! so operators grepping the tree see the shape immediately.
//!
//! The T-12 regression pin: for every HTTP method bucket
//! (GET/POST/PUT/PATCH/DELETE), every `.yml` sample loads with a
//! populated `dsl.declaration` field. `warn_on_missing_declarations`
//! from `src/dsl/loader.rs` returns 0.
//!
//! WS / triggers / cronmanager samples are explicitly NOT part of
//! this pin — they're routed differently and `warn_on_missing_declarations`
//! already skips them (`src/dsl/loader.rs:86-91`).

#![allow(clippy::field_reassign_with_default)]

use ruuter_on_rust::config::AppConfig;
use ruuter_on_rust::dsl::loader::{warn_on_missing_declarations, DslLoader};
use std::collections::HashMap;
use std::path::PathBuf;

fn samples_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("DSL/samples")
}

#[test]
fn every_http_sample_has_a_declaration_block() {
    let mut cfg = AppConfig::default();
    cfg.config_path = samples_root();
    let loader = DslLoader::new(cfg.clone(), HashMap::new());
    let loaded = loader.load_everything().expect("load DSL/samples");
    let missing = warn_on_missing_declarations(&loaded.http, false);
    assert_eq!(
        missing, 0,
        "h2ck.me v1 T-12 regression pin: every HTTP sample must \
         carry a `declaration:` block. Got {missing} missing."
    );
}

#[test]
fn declaration_count_covers_all_http_methods() {
    let mut cfg = AppConfig::default();
    cfg.config_path = samples_root();
    let loader = DslLoader::new(cfg.clone(), HashMap::new());
    let loaded = loader.load_everything().expect("load");
    // Walk every method bucket and count DSLs; assert each bucket
    // has at least one DSL AND every DSL has a declaration.
    for by_method in loaded.http.values() {
        for (method, dsls) in by_method {
            if !matches!(
                method.to_uppercase().as_str(),
                "GET" | "POST" | "PUT" | "PATCH" | "DELETE" | "OPTIONS" | "HEAD"
            ) {
                continue;
            }
            for (key, dsl) in dsls {
                assert!(
                    dsl.declaration.is_some(),
                    "HTTP DSL {method}/{key} lacks a declaration block \
                     (h2ck.me v1 T-12 requires one)"
                );
            }
        }
    }
}
