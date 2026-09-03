# JavaScript gotchas

## Object literal at expression start

`{...}` at the start of a JS expression is parsed as a block statement, not an object literal.

```yaml
right: "${({ id: order.id, ok: true })}"     # wrap in parens
wrong: "${{ id: order.id, ok: true }}"       # SyntaxError: expected ';'
```

## Nested string quoting

YAML sees `"…"` first, then JS sees the content. Prefer single quotes inside `${...}`:

```yaml
ok:    "${incoming.headers['x-user']}"
wrong: "${incoming.headers[\"x-user\"]}"     # works, ugly
```

## Truthy of missing values

`incoming.body.missing` is `undefined`, which is falsy — safe in a `switch` condition. Use `??` for a default:

```yaml
name: "${incoming.body.name ?? 'anon'}"
```

## Undeclared identifiers (issue #57)

Bare references to an identifier that was never bound resolve to `undefined` (not `ReferenceError`), so template composition can pass optional bindings without every caller having to `assign:` a placeholder first:

```yaml
tag:      "${platform?.id}"        # `platform` never bound → null
audit:    "${caller_id ?? 'anon'}" # `caller_id` never bound → 'anon'
```

The lenient behaviour applies only to *undeclared* identifiers. Dereferencing a *declared-but-null* value still throws (JS-spec `TypeError`), because that is a real DSL bug — the guard against it is exactly what `?.` is for:

```yaml
right: "${platform?.id}"           # safe under both undeclared and null-platform
wrong: "${platform.id}"            # if platform === null → TypeError
```

## Nullish serialisation (issue #57)

Where a `${…}` result surfaces on the wire, `null` and `undefined` are treated per JS spec and per HTTP realities — never as the literal string `"null"`:

| Sink | `null` / `undefined` behaviour |
|---|---|
| **Response header** value | Header is dropped. HTTP has no null-valued header. |
| **Outbound request header** or query param | Same — header / query param is dropped. |
| **Response body** — object property | `undefined` property is dropped; `null` property is preserved (`{"k": null}`). |
| **Response body** — array slot | Both become JSON `null` (spec parity with `JSON.stringify([1, undefined])`). |
| **Mixed string** `"hi ${x}"` when `x` is null/undefined | Segment interpolates as empty (`"hi "`, not `"hi null"`). |
| **Whole-value** `${x}` when `x` is null/undefined | Returns JSON `null` (preserves native type per the [type-preservation rule](./expressions.md#type-preservation)). |

## Numbers vs strings

Query and header values are always strings. Cast if you need arithmetic:

```yaml
age: "${parseFloat(incoming.params.age)}"
n:   "${Number(incoming.headers['x-count'])}"
```

## Runtime limits

Boa is CPU-bounded, not wall-clock-bounded. A `while(true){}` aborts at `max_loop_iterations` (default 1 000 000). Deep recursion aborts at `max_stack_size` (default 400). Both surface as `Script evaluation error`.

## No async

`fetch`, `Promise`, `await` — none of these work inside `${...}`. Use the [`http` step](./steps/http.md).

## Type preservation

Single-expression values keep their JS type. Mixed strings stringify:

```yaml
number: ${1 + 1}                # → 2 (JSON number)
string: "value ${1 + 1}"        # → "value 2" (JSON string)
```
