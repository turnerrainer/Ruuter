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
