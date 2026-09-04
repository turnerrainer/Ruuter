# Steps

A DSL file is an ordered map of named steps. The **first key** is the entry step.

Control flow:

- Implicit fall-through to the next step in source order.
- Explicit `next: <step-name>` to jump.
- Terminate with `next: end`.
- A `return` step terminates immediately (its `next:` is ignored except for `next: end` clarity).
- `next:` pointing to a step name that isn't declared in the DSL (typo, stale reference) raises a runtime error at the jump — the run stops and the caller sees a DslExecution error naming the source step and the missing target. This applies to top-level `next:`, `switch` branch `next:`, and any other named jump target.

Every step type is documented on its own page. Common fields:

| Field  | On which step | Purpose |
|--------|---------------|---------|
| `next` | all           | override implicit fall-through |
| `result` | `http`, `template` | bind response into caller context |
| `status`, `headers` | `return` | override HTTP status + response headers |

## One action per step

Every step contains exactly **one** action key from this set:

`call:` · `template:` · `assign:` · `return:` · `switch:` · `log:` · `state:` · `iterate:` · `ws_send:` · `ws_tag:` · `single_flight:`

A step listing more than one action key (e.g. `call:` alongside `assign:`, or `log:` alongside `switch:`) is **rejected at DSL load time** — Ruuter refuses to start (or fails the hot-reload, keeping the previous tree live) with an error naming the offending step and every offending action key. Split the actions into separate steps and chain them with `next:`.

This rule exists because a step maps 1:1 to a single action variant internally. Prior versions silently deserialised only one of the keys (winner picked by parser priority) and dropped the rest — which meant the DSL on disk didn't describe what actually ran. Refusing the ambiguous input at load time keeps the on-disk DSL truthful.

Other unknown top-level keys inside a step (typos, obsolete fields) are still ignored today, but that leniency may tighten in a future version — don't rely on it.
