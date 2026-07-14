# Expression language

Two forms substitute JavaScript expressions into DSL values:

```yaml
inline: "${expr}"        # anywhere inside a string or as the whole value
whole:  "$= expr =$"     # whole-line variant, equivalent to ${expr}
```

## Type preservation

A value that is **exactly** `${expr}` returns the JS value's native JSON type:

```yaml
count:   ${1 + 1}                # → 2      (number)
active:  ${incoming.body.on}     # → true   (bool)
items:   ${[1,2,3]}              # → [1,2,3] (array)
```

A value that MIXES literal text with `${...}` is stringified:

```yaml
greeting: "hi ${name}"           # → "hi Ada"
url:      "https://[#API_HOST]/v1/user/${id}"
```

## Available JS

Boa engine (ECMAScript 2015+ subset). Standard built-ins that work:

- `JSON.stringify`, `JSON.parse`
- `Date.now()`, `new Date().toISOString()`
- `Math.*`
- `Array.prototype.*` including `map`, `filter`, `reduce`
- `String.prototype.*` including `split`, `replace`, `padStart`
- Optional chaining `?.` and nullish coalescing `??`

Not available:

- `console.log` — use the [`log` step](./steps/log.md).
- `fetch` / `XMLHttpRequest` — use the [`http` step](./steps/http.md).
- `require` / `import`.
- Async / Promises inside expressions.

## Runtime limits

Per-evaluation budget (see [Script runtime limits](../framework/script-limits.md)):

- `max_loop_iterations` (default 1 000 000)
- `max_stack_size` (default 400)

Exceeding either aborts the evaluation with `Script evaluation error`.
