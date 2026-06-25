# 002 — Reuse a single Boa context per DSL execution

**Status**: BACKLOG.
**Severity**: HIGH (latency on any DSL with >1 `${...}` expression).
**Effort**: 0.5 day.
**Filed**: 2026-06-17.

## What's wrong

`src/scripting/mod.rs:68-81` (`execute_js`) creates a new
`BoaContext::default()` for **every** `${...}` expression, and
`setup_bindings` then re-injects the entire `incoming` object plus all
context variables as JSON-stringified `var` declarations.

For a request with N expressions the cost is O(N × full context init).
A DSL with an `assign` step containing five `${...}` keys pays the
context-init cost five times.

## Fix

Two-stage:

1. Lift context construction up to the DSL-execution scope:
   - `ExecutionContext` owns a `RefCell<Option<BoaContext>>` (or a
     dedicated `ScriptScope` struct) initialised lazily on first JS
     evaluation.
   - `ScriptEngine::evaluate` borrows this scope instead of creating
     a fresh `BoaContext`.
   - When `ExecutionContext::set_variable` mutates state, the scope is
     either invalidated or the new binding is pushed into the existing
     Boa context (cheaper).

2. Optional follow-up (separate task): pre-compile each expression's
   AST/bytecode once at DSL load time and cache it keyed by source
   string.

## Verification

- Unit test: build an `ExecutionContext` with 10 variables and evaluate
  100 `${...}` expressions; expect single-digit ms total on dev box.
- Existing DSL samples in `DSL/samples/` continue to return identical
  responses.

## Why this is generic

Pure scripting-engine optimisation. No service-specific knowledge of
any callsite required.
