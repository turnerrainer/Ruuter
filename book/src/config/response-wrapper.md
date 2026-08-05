# Response wrapper

Controls whether the body of a DSL response is wrapped in Java's
`RuuterResponse` envelope — `{"response": <value>}` — or returned raw.

## What it is

Java Ruuter always wraps a DSL's return value:

```json
{"response": {"greeting": "hello"}}
```

Ruuter-on-Rust defaults to the same shape as of 2026-08-05 (parity flip
from `false` to `true`). A `return:` step's explicit `wrapper: true |
false` still wins over the config default.

## The config

```yaml
response:
  default_wrapper: true
```

## The default and why

`true` — matches Java. Every operator porting a `application.yml` gets
identical response shapes without an override.

## What breaks if you set it wrong

- `default_wrapper: false` when a client expects the envelope → the
  client's parser looks for a `response` key that isn't there and
  throws a schema-validation error.
- `default_wrapper: true` when a proxy or gateway already unwraps →
  double-wrapping. The client sees `{"response": {"response": ...}}`.

## Per-step override

A single `return:` step can opt out (or in) regardless of the config
default:

```yaml
raw:
  return:
    body:
      count: 42
    wrapper: false
```

The `wrapper:` field on `return:` is the last word — the config only
sets the default when the step doesn't specify.

## Migration from Java

No change needed. Both defaults match.

## Cross-links

- [return step](../dsl/steps/return.md)
- [Configuration overview](../ops/configuration.md)
