# Matchers

Two matcher modes are used by the runner:

- **Deep equal** — recursive equality, no wildcards. Used by `expect.body:` and `verify_mocks[].body_matches:` when the expected value is a scalar or exactly-shaped structure.
- **Subset match** — every key/index in `expected` must be present in `actual` with a subset-matching value. Extras in `actual` are ignored. Used by `expect.body_matches:`, `verify_state[].value:`, `verify_mocks[].body_matches:`, and every `ws.expect_frames[]` entry.

## Wildcards (subset match only)

The wildcard tokens below match anything on the actual side when they appear on the expected side:

| Token | Meaning |
|---|---|
| `"***"` | Matches any value at this position |
| `"$type:string"` | Actual must be a JSON string |
| `"$type:number"` | Actual must be a JSON number (integer or float) |
| `"$type:bool"` | Actual must be a JSON boolean |
| `"$type:object"` | Actual must be a JSON object |
| `"$type:array"` | Actual must be a JSON array |
| `"$type:null"` | Actual must be JSON `null` |
| `"$type:any"` | Always matches (same as `***`) |
| `"$regex:<pattern>"` | Actual must be a string matching the Rust `regex` pattern |

## Numeric tolerance

`400` (JSON integer) and `400.0` (JSON float) subset-match as equal. This is deliberate — most DSLs don't declare number type explicitly, and both round-trips through the JS engine and the state store can convert integer→float. If you need strict integer-vs-float distinction, use `expect.body:` (deep equal) instead of `body_matches:`.

## Object subset semantics

```yaml
expected: { a: 1 }
actual:   { a: 1, b: 2 }
```

`subset_matches(expected, actual)` = **true**. Extras in `actual` are ignored. This lets you assert on a stable subset of fields without listing every timestamp / correlation id.

## Array semantics

Arrays are **length-checked and position-matched**. Both arrays must have the same length; each element pair subset-matches.

If you want "actual array contains at least N", split it into `verify_state` / `verify_mocks` assertions instead — the subset matcher deliberately doesn't do containment on arrays.

Use `"***"` per element to opt an individual position out of matching:

```yaml
expected: [ { id: 1 }, "***", { id: 3 } ]
actual:   [ { id: 1, extra: true }, { anything: "here" }, { id: 3 } ]
# matches: length equal, elements 0 and 2 subset-match, element 1 is a wildcard
```

## verify_state semantics

- `value: null` → the key must be **missing** from the state store.
- Any other value → subset-match against the actual value (or deep-equal if the actual is a scalar).

## Worked examples

### Number type

```yaml
body_matches:
  age: "$type:number"          # matches 42, 42.0, -1.5
```

### Regex on a string field

```yaml
body_matches:
  ref: "$regex:^txn-\\d+$"     # matches "txn-12345", not "TXN-1" or "txn-abc"
```

### Extras ignored, timestamp checked by type

```yaml
body_matches:
  message: "Received"
  timestamp: "$type:number"    # exact value would flake; type check is stable
  # other fields in actual are allowed
```

### Full array match

```yaml
body:
  items: [1, 2, 3]             # deep-equal: exact length, exact elements
```

### Partial array match

```yaml
body_matches:
  items: [ "$type:number", "$type:number", "$type:number" ]
```
