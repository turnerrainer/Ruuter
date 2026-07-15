# 023 — Implement http.patch DSL step

**Filed:** 2026-06-30 by stocktrading-dev/desk team after 2026-06-29 live-paper outage.

## Problem

`http.patch` is not implemented as a DSL step. Every DSL cell that uses
it fails at runtime with:

```
WARN ruuter_on_rust::triggers: trigger DSL failed
  project="alpaca" channel="t" key="${sym}"
  error=Invalid DSL step: Unknown HTTP method: http.patch
```

Source code search (in the desk-ruuter binary v0.4.0): the HTTP step
dispatcher likely matches on `get|post|delete` and falls through on
`patch`.

## Impact (concrete production outage 2026-06-29)

The desk-trading bot uses `http.patch` to ratchet a resting broker stop
during the gap-fade strategy's ARMED state — the trail moves up (long)
or down (short) on every favorable tick, and the broker stop must be
PATCHed to `stop_price=trail_px` so the safety net follows.

On 2026-06-29 in production, every arm and ratchet step fired this
warning and broker stops NEVER moved off cat_SL. Two positions
(GPC, RPRX) reached MFE >100 bp but their broker stops stayed at
~−25 bp from entry — fully unprotected upside. A manual side-channel
PATCH via direct Alpaca API was needed to lock in profit.

## Workaround in DSL (already deployed, ugly)

`triggers/t/_default.yml` was refactored to DELETE-then-POST instead
of PATCH:

```yaml
do_arm_patch_broker_stop:
  call: http.delete
  args:
    url: "...orders/id?q=${fsm.broker_stop_id}"
  next: do_arm_place_new_stop

do_arm_place_new_stop:
  call: http.post
  args:
    url: "...orders"
    body:
      symbol: ...
      qty: ...
      side: ...
      type: stop
      stop_price: "${new_trail}"
      client_order_id: "...-arm-${new Date().getTime()}"
  result: arm_post_result
  next: ...
```

The brief gap between cancel-ack and post-response (typically <100 ms)
leaves the position un-stopped. The new stop is favorable (tighter)
so cur is on the safe side of the OLD stop too; only a fast adverse
move during the gap is a loss path. Acceptable but suboptimal.

## Proper fix (this task)

Add `http.patch` to the DSL step dispatcher. Body and headers handled
identically to `http.post` (Alpaca's PATCH /v2/orders/{id} expects a
JSON body with the fields to update; response has the new order
record).

Generic-component scope check: PATCH is a standard HTTP verb and the
implementation is identical to POST modulo method string. No
service-specific code.

## Acceptance

- `call: http.patch` works in HTTP DSL files with body + headers
- Echoed request method is PATCH
- Status + body extractable as `result.response.status` /
  `result.response.body` (same shape as other HTTP step results)
- Add `tests/dsl/http_methods/test_http_patch.yml` covering: 200,
  4xx, 5xx, network timeout

## Related

- task 021 (WS source upgrade headers) — same pattern: a missing
  primitive forced a workaround in DSL. We are filing this one
  proactively so the next caller doesn't repeat the workaround.
- (Separate issue not in this task: `ws_send` step in HTTP DSL
  context appears to be a no-op — only `send_json` inside
  `sources/{name}.yml::on_connect` actually emits WS frames. Will
  file separately after confirming.)
