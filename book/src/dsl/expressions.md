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

A `${…}` segment inside a mixed string that evaluates to `null` or `undefined` interpolates as empty (never the literal string `"null"`); the same value in a header / query slot drops that header or param entirely. Full nullish-value table + rationale: [JavaScript gotchas → Nullish serialisation](./js-gotchas.md#nullish-serialisation-issue-57).

## Scripting backend

Two backends ship in the binary; exactly one is active per build (feature-gated, mutually exclusive):

- **Boa** (`scripting-boa`, default) — pure-Rust ECMAScript engine.
- **QuickJS** (`scripting-quickjs`) — C engine via `rquickjs`.

Both are exercised on every release-gate cycle (see [Scripting engines](../framework/scripting-engines.md)). The supported-construct list below is validated on both by [`tests/issue_90_js_subset.rs`](https://github.com/turnerrainer/Ruuter/blob/dev/tests/issue_90_js_subset.rs); anything not on the list is either "unverified — file an issue" or "deliberately unsupported."

## Supported constructs

Every row below is verified on Boa AND QuickJS via the same test file, so a regression on either backend fails CI. All examples are wrapped in `${…}` in real DSL YAML (`"${1 + 2}"` etc.); the raw expression is shown for brevity.

### Primitives

| Category | Expressions |
|---|---|
| Arithmetic | `+`, `-`, `*`, `/`, `%`, `**` |
| Comparison | `<`, `>`, `<=`, `>=`, `==`, `===`, `!=`, `!==` |
| Logical | `&&`, `\|\|`, `!` |
| Nullish | `??` (nullish coalescing), `?.` (optional chaining, including on function calls `f?.()`) |
| Ternary | `cond ? a : b` — **quote the YAML scalar** or the `: ` inside terminates the plain scalar. See [YAML gotchas](./yaml-gotchas.md). |
| `typeof` | Returns `"string"`, `"number"`, `"boolean"`, `"object"` (for null / arrays / objects), `"undefined"` |

### String methods

| Method | Example |
|---|---|
| `.length` | `'hello'.length` → 5 |
| `.toLowerCase()` / `.toUpperCase()` | `'HI'.toLowerCase()` → `"hi"` |
| `.startsWith(x)` / `.endsWith(x)` | `'hello'.startsWith('he')` → true |
| `.includes(x)` | `'hello'.includes('ell')` → true |
| `.indexOf(x)` / `.lastIndexOf(x)` | `'hello'.indexOf('l')` → 2 |
| `.substring(a, b)` / `.slice(a, b)` | `'hello'.slice(-2)` → `"lo"` |
| `.split(sep)` | `'a,b,c'.split(',')` → `["a","b","c"]` |
| `.charAt(i)` / `.charCodeAt(i)` | `'hello'.charAt(1)` → `"e"` |
| `.trim()` / `.trimStart()` / `.trimEnd()` | `'  hi  '.trim()` → `"hi"` |
| `.replace(a, b)` / `.replaceAll(a, b)` | `'hello'.replaceAll('l','L')` → `"heLLo"` |

### Array methods

| Method | Example |
|---|---|
| `Array.isArray(x)` | `Array.isArray([1,2])` → true |
| `.length` | `[1,2,3].length` → 3 |
| `.map(fn)` | `[1,2,3].map(x => x * 2)` → `[2,4,6]` |
| `.filter(fn)` | `[1,2,3].filter(x => x > 1)` → `[2,3]` |
| `.some(fn)` / `.every(fn)` | `[1,2,3].every(x => x > 0)` → true |
| `.find(fn)` / `.findIndex(fn)` | `[1,2,3].find(x => x > 1)` → 2 |
| `.reduce(fn, init)` | `[1,2,3].reduce((a,b) => a + b, 0)` → 6 |
| `.includes(x)` | `[1,2,3].includes(2)` → true |
| `.indexOf(x)` | `[1,2,3].indexOf(2)` → 1 |
| `.join(sep)` | `[1,2,3].join('-')` → `"1-2-3"` |
| `.concat(other)` | `[1,2].concat([3,4])` → `[1,2,3,4]` |
| Spread | `[...[1,2], 3]` → `[1,2,3]` |

### Object methods

| Method | Example |
|---|---|
| `Object.keys(o)` | `Object.keys({a:1, b:2})` → `["a","b"]` |
| `Object.values(o)` | `Object.values({a:1, b:2})` → `[1,2]` |
| `Object.entries(o)` | `Object.entries({a:1})` → `[["a", 1]]` |
| `Object.assign(t, s)` | `Object.assign({}, {a:1}, {b:2})` → `{a:1, b:2}` |
| Spread | `{...({a:1}), b:2}` → `{a:1, b:2}` |
| Bracket access | `obj['a-b']` — for keys with hyphens or special characters |

### Type conversion

| Method | Example |
|---|---|
| `String(x)` | `String(42)` → `"42"` |
| `Number(x)` | `Number('42.5')` → `42.5` |
| `Boolean(x)` | `Boolean('')` → false; `Boolean('x')` → true |
| `.toString()` | `(42).toString()` → `"42"` |
| `parseInt(x, 10)` | `parseInt('42', 10)` → 42 |
| `parseFloat(x)` | `parseFloat('42.5')` → 42.5 |

### JSON

| Method | Example |
|---|---|
| `JSON.parse(s)` | `JSON.parse('{"a":1}')` → `{a:1}` |
| `JSON.stringify(o)` | `JSON.stringify({a:1})` → `'{"a":1}'` |

### Math

`Math.floor`, `Math.ceil`, `Math.round`, `Math.abs`, `Math.min`, `Math.max`, `Math.random`, `Math.pow` (or `**`), `Math.sqrt`, `Math.log`, `Math.log10`, and the other standard `Math.*` are all supported. Verified rows in the test cover `floor`, `ceil`, `round`, `abs`, `min`, `max`, `random`.

### Regex

Both backends support ECMAScript regex literals and the `RegExp` constructor:

```yaml
matches:  ${/^\d+$/.test('12345')}                    # → true
found:    ${'hello world'.match(/(\w+)/)[0]}          # → "hello"
compiled: ${new RegExp('^he').test('hello')}          # → true
```

Flags (`i`, `g`, `m`, `s`, `u`) work as in standard JS. A non-matching `.match(...)` returns `null` (not an empty array).

### Functions

Arrow functions, `function` expressions, and IIFEs all work:

```yaml
double:  ${((x) => x * 2)(21)}                             # → 42
same:    ${(function(x) { return x * 2; })(21)}            # → 42
sum:     ${[[1,2],[3,4]].map(a => a.reduce((x,y)=>x+y,0))} # → [3, 7]
```

Functions are per-evaluation only; there is no way to "define once, reuse" across steps (each `${…}` gets a fresh eval context).

## Deliberately unsupported

The following are NOT and will NOT be available inside `${…}`:

- **`console.log`, `console.*`** — use the [`log` step](./steps/log.md) for structured logging with redaction.
- **`fetch`, `XMLHttpRequest`, `WebSocket`** — use the [`http` step](./steps/http.md) or the [`ws_send` step](./steps/ws_send.md). Network I/O in an expression would bypass Ruuter's SSRF allow-list, timeouts, and response-size cap.
- **`require`, `import`, `import()`** — no module system inside expressions. Composition is via [`template` steps](./steps/template.md).
- **`eval`, `new Function(...)`** — deliberately disabled. An expression that constructs another expression from user input is a hostile-input eval vector.
- **`async` / `await` / `Promise.*`** — `evaluate` is synchronous. Await points would need to unwind through the sync engine boundary; supporting this would rewrite the whole scripting seam and hasn't been asked for.
- **`setTimeout`, `setInterval`, `queueMicrotask`** — no event loop is exposed to expressions. A scheduled action belongs in CronManager or in an event-driven WS source.
- **Filesystem, process, environment** (`fs`, `process`, `os.*`) — none of Node's globals are available. The runtime is a pure ECMAScript engine, not Node.

## Runtime limits

Per-evaluation budget (see [Script runtime limits](../framework/script-limits.md)):

- `max_loop_iterations` (default 1 000 000) — caught by the engine, aborts with `Script evaluation error`.
- `max_stack_size` (default 400) — expression depth limit.

Exceeding either aborts the evaluation and produces `Script evaluation error` on the step. The step's `error:` handler (if wired) picks up the error binding.

## Adding a construct to the supported list

Anything a DSL author has "observed to work" but isn't on the table above is unverified. Two paths:

1. **Add a row to `tests/issue_90_js_subset.rs`** and open a PR — if it passes on both backends the row moves to "supported" and the doc updates in the same change.
2. **File an issue** with the specific construct + a minimal DSL that uses it. Issue #90 tracks the general "which JS is supported" question.

The maintainer's stance: the supported list expands empirically. Verified on both engines → in the table. Working on one engine but not the other → issue tracker (unlikely, given the 2026-era Boa / QuickJS coverage).
