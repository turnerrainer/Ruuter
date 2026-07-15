# 037 — Fast-path bypass of ScriptEngine for values without `${...}` / `$=`

## Filed

2026-07-15 — surfaced by the 0.4.0 perf benchmark (see task 039).

## Problem

`ScriptEngine::evaluate()` is called on EVERY value in EVERY step —
including plain string literals with no expressions in them. Example
from `DSL/samples/GET/ping.yml`:

```yaml
response:
  status: 202
  headers:
    XPingStatusHeader: pong delivered   # plain literal, no ${...}
  return: pong                          # plain literal
```

Both `"pong delivered"` and `"pong"` are pushed through Boa (setup +
eval) despite having zero JS. That's ~2ms per header value on a
non-pooled context.

## Fix

Before calling `ScriptEngine::evaluate()`, scan the string for `${` or
`$=`. If neither is present, return the value verbatim without
touching Boa.

Also applies recursively — a `Value::Object`/`Value::Array` whose
every leaf is a literal returns unchanged without spinning up a
context at all.

## Numbers

Rough estimate: ~30-40% of DSL step values in `DSL/samples/*` are
literals (status codes, header values, error messages, boolean flags).
Skipping Boa for them should give a smaller-than-036 but still real
win — probably 20-30% throughput on the sample corpus, more on
DSLs that use plain values heavily.

## Acceptance

- `ScriptEngine::evaluate()` short-circuits for expression-free values.
- Regex check is O(n) once per value; cheaper than any Boa call.
- Unit tests: assert Boa is NOT constructed when input has no `${` /
  `$=`. Property test: for any literal value, `evaluate()` output ==
  input.
- Perf: measurable improvement on `/samples/basic/hello` and
  `/samples/ping`. If gains are <10%, don't merge — the added
  complexity isn't worth it.

## Non-goal

Parsing the JS to detect no-op expressions like `${1+1}`. String scan
for the delimiters is enough.
