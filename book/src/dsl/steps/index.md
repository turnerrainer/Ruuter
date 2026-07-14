# Steps

A DSL file is an ordered map of named steps. The **first key** is the entry step.

Control flow:

- Implicit fall-through to the next step in source order.
- Explicit `next: <step-name>` to jump.
- Terminate with `next: end`.
- A `return` step terminates immediately (its `next:` is ignored except for `next: end` clarity).

Every step type is documented on its own page. Common fields:

| Field  | On which step | Purpose |
|--------|---------------|---------|
| `next` | all           | override implicit fall-through |
| `result` | `http`, `template` | bind response into caller context |
| `status`, `headers` | `return` | override HTTP status + response headers |

Unknown top-level keys inside a step are ignored (forward-compat) but may become a hard error in a future version — don't rely on it.
