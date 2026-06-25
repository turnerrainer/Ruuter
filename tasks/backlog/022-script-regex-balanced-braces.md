# 022 — `${...}` script delimiter regex breaks on inner `{...}`

**Filed:** 2026-06-25 by desk team during replay-stack DSL development.

## Problem

`src/scripting/mod.rs` uses

```rust
static SCRIPT_PATTERN: Lazy<Regex> = Lazy::new(|| Regex::new(r"\$\{([^}]+)\}").unwrap());
```

The negated character class `[^}]+` stops at the FIRST `}`. JS
expressions containing object literals are silently truncated:

```yaml
value: "${JSON.stringify({sym: sym, side: side})}"
```

The captured script becomes `JSON.stringify({sym: sym, side: side` —
the trailing `})}` falls outside the capture. Boa then errors:

```
Script evaluation error: SyntaxError: abrupt end
```

For desk's replay FSM this fires on every per-tick state update,
saturating CloudWatch with ERROR logs and producing no FSM state
writes downstream.

## Implementation (this patch)

Replace the regex match with a balanced-brace walker:

1. Scan the input for `${`.
2. From the position after `${`, advance a depth counter incrementing
   on `{` and decrementing on `}`. Quit when depth hits zero.
3. The substring between `${` and the matching `}` is the script
   source.
4. Leave string-literal handling as-is for now — embedded `}` inside
   JS string literals is an open edge case (low risk for our DSL
   style; revisit if it surfaces).

Same logic applies to the `LINE_PATTERN` `$=...=$` form.

## Cross-references

- Surfaces in: `desk/DSL/Ruuter/POST/replay/tick.yml` FSM state-update
  expressions using `JSON.stringify(Object.assign({}, fsm, {...}))`.
- Sibling: `021-ws-source-upgrade-headers` (also blocking desk replay).
