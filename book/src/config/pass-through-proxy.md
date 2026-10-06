# Pass-through proxy config (issue #134)

Process-wide tuning for routes declared with
[`declaration.proxy:`](../dsl/proxy.md). The dedicated reqwest client
built from this block is independent of the one `http.*` steps use,
so a saturated proxy workload cannot starve the DSL step budget.

## What it is

A top-level `pass_through_proxy:` block in `ruuter.yaml`:

```yaml
pass_through_proxy:
  pool_max_idle_per_host: 32
  pool_idle_timeout_ms: 90000
  connect_timeout_ms: 10000
```

Absent-block defaults match the shown values.

## Fields

| Field | Default | Meaning |
|---|---|---|
| `pool_max_idle_per_host` | `32` | Idle connections kept per upstream host in the proxy client's pool. Separate from the `http.*` step pool. |
| `pool_idle_timeout_ms` | `90000` | How long an idle pooled connection is kept before it is closed. |
| `connect_timeout_ms` | `10000` | TCP connect timeout to the upstream. Shorter than the per-route `request_timeout_ms` so a dead upstream fails fast and surfaces as a 502 rather than holding a Semaphore slot for the full request budget. |

## Per-route proxy settings

The per-route surface — `upstream`, `max_body_bytes`, `max_in_flight`,
`inbound_progress_timeout_ms`, `allowed_encodings`,
`request_timeout_ms`, `preserve_headers` — is declared on each route
via `declaration.proxy:`. See
[Pass-through proxy routes](../dsl/proxy.md).

## Why two pools

The `http.*` step and pass-through proxy routes have different
traffic shapes:

- **`http.*` step** — many small short-lived JSON calls, typically to
  Resql / TIM / sibling services at < 100 ms per call.
- **Proxy routes** — fewer large long-lived calls (AS4 / eDelivery
  with binary attachments), often minutes per call.

Mixing them in one pool means a saturated proxy workload (dozens of
long-held upstream connections) starves the `http.*` step's budget
and tail latency on normal API routes spikes. Separating them is a
cheap containment surface — each pool has its own
`pool_max_idle_per_host` / `pool_idle_timeout_ms` ceiling.

## When to raise `connect_timeout_ms`

The default 10 s is tuned for upstreams reachable on the same
network segment or across a well-provisioned WAN. If your upstream
sits behind a VPN or a stateful appliance that takes multiple
seconds to establish a session, raise this. A value longer than
`request_timeout_ms` is nonsensical; `dsl-lint` currently does not
check this but a future version may.

## Cross-links

- [Pass-through proxy routes](../dsl/proxy.md) — per-route settings
  and the request/response contract.
- [Internal-requests (SSRF allow-list)](internal-requests.md) — SSRF
  gate applies to proxy upstreams too; a private-network upstream
  needs an explicit allowlist opt-in.
- [`declaration` step](../dsl/steps/declaration.md) — where
  `declaration.proxy:` fits in the declaration surface.
