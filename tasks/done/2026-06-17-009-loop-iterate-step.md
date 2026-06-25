# 009 — Generic `loop` / `iterate` step

**Status**: BACKLOG.
**Severity**: LOW (workaround: chain `switch` + `next`).
**Effort**: 0.5 day.
**Filed**: 2026-06-17.

## What's wrong

`execute_steps` (`src/router/mod.rs:73-104`) caps iterations at 100
and offers no first-class way to iterate over a list. Today a DSL
that wants to apply a sub-pipeline to each element of an array has
to fake it with `switch` jumps — readable for nobody, and the 100-
iteration cap is a footgun.

## Fix

Add a step type:

```yaml
- iterate:
    over: "${state.get('open_positions')}"
    as: "pos"
    do:
      - log: "Reviewing ${pos.symbol}"
      - http:
          call: GET
          args: { url: "${constants.broker_url}/positions/${pos.symbol}" }
          result: "pos_status"
    # Optional aggregate result.
    collect: "${pos_status.body}"
    into: "statuses"
```

Implementation: a new `DslStep::Iterate` variant whose executor runs
a child step list per element, sharing the same `ExecutionContext`
under a nested scope (so `pos` doesn't leak out).

Remove the hardcoded `max_iterations = 100` from `execute_steps`;
replace with per-DSL config (`max_steps`, default 10_000) plus an
explicit per-`iterate` step `max_items`.

## Verification

- DSL test iterating over a 1000-element list; confirm correct
  aggregate and bounded execution time.

## Why this is generic

`iterate` is a generic control-flow primitive available in every
modern DSL.
