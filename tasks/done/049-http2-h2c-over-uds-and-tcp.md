# 049 — HTTP/2 (h2c) over UDS and TCP loopback

## Filed

2026-07-19 — surfaced by the v0.6.0 A/B bench (see
`bench/run-ab-comparison.sh`). UDS via HTTP/1.1 did not beat TCP
loopback because reqwest's pooled keep-alive amortises TCP handshake
cost across many requests, while HTTP/1.1 head-of-line blocking caps
per-connection throughput symmetrically on both transports. HTTP/2's
stream multiplexing is the missing lever.

## Problem

HTTP/1.1 has a fundamental limit: one connection = one in-flight
request. To scale concurrency, clients open many connections; each
one costs a fresh handshake (TLS on external, still TCP framing on
loopback). Servers correspondingly hold many concurrent sockets and
allocate per-connection state.

HTTP/2 multiplexes many streams onto ONE connection. A single warm
connection can carry hundreds of concurrent requests interleaved.
For the sidecar-hop pattern (Ruuter → Resql/TIM/AS4), this is a
5-10× throughput win under load and a meaningful latency reduction
even on single-request paths (no per-request handshake).

## Fix

Support h2c (HTTP/2 cleartext, RFC 7540 §3.4) on:

### Outbound (`src/http_client/mod.rs`)

- Reqwest already speaks HTTP/2 over HTTPS by default (via ALPN).
  For h2c (plaintext) it needs an explicit builder option, or
  the caller adds an `HTTP2-Settings` upgrade header.
- Or: use `hyper-util::client::legacy::Client` with `http2_only(true)`
  and swap it in for TCP+UDS paths behind a feature flag.
- Detection: `http_client.http_version: "http1" | "http2" | "auto"`
  config option. Default `auto`; picks h2c for hosts in
  `unix_socket_map` and localhost self-calls, http1.1 otherwise.

### Inbound (`src/main.rs`)

- Axum 0.7 uses hyper 1.5 which supports HTTP/2 via
  `hyper::server::conn::http2::Builder`. Multi-listener code needs
  to spawn h2c-capable accept loops when the listener declares
  `http2: true`.
- Same Router serves both HTTP/1.1 and h2c listeners; the transport
  is invisible to the DSL.

### UDS + HTTP/2 pairing

The combination is where the win compounds:

- UDS eliminates TCP framing + softirq for local hops
- HTTP/2 eliminates HOL blocking for high-concurrency workloads
- Together: closest thing to shared-memory IPC over an HTTP
  interface. Should recover the "~150 µs p50 UDS win" the 043 task
  originally predicted, plus deliver 3-5× throughput improvement
  under concurrent load.

## Acceptance

- Cargo config toggle: `[dependencies] hyper = { features = [..., "http2"] }`
  and matching hyper-util features.
- `HttpClient` new field: `http_version: HttpVersion` enum (`Http1`,
  `Http2`, `Auto`) plumbed from AppConfig.
- Outbound: h2c connection to a host that speaks h2c produces the
  same `HttpResponse` shape as http1.1.
- Inbound: listener with `http2: true` accepts h2c connections;
  same routes work.
- Integration tests: outbound h2c against an in-process h2c mock;
  inbound h2c server accepts h2c requests from a hyper h2 client.
- Bench: extend `bench/run-043-ab.sh` with h2 variants
  (`043-uds-h2`, `043-tcp-h2`) and confirm h2 wins by ≥3× on
  concurrent duplicate-target load.
- Book chapter: extend `book/src/framework/inter-service-transport.md`
  with the http1 vs h2 tradeoff table and when to pick each.

## Interaction with tasks 043 + 044

- 043 (UDS transport): h2c over UDS is where the compound win lives.
  043's v1 UDS uses HTTP/1.1 per-request-handshake — h2c would
  eliminate per-request handshake AND connect pooling both.
- 044 (self-call short-circuit): unchanged. The short-circuit
  bypasses BOTH transport and the HTTP layer entirely, so h2 is
  irrelevant for self-calls (they never touch a socket).
- 050 (UDS keep-alive pool): the h1.1 fix for 043's pooling gap.
  Composes with h2 but 050 is smaller-scope; do 050 first, then
  h2c on top.

## Non-goals

- HTTP/3 (QUIC over UDP). Nice-to-have for cross-node WAN hops but
  the marginal win over h2 on loopback/UDS is small. File a separate
  task if a compelling workload emerges.
- Downgrade negotiation. h2c and http1.1 use different upgrade
  paths; supporting both on the same listener is possible but
  ugly. Simpler: separate listeners per protocol.
- h2 over TLS on the inbound side. External-facing TLS termination
  is deployment-layer's problem (nginx, envoy, ALB); Ruuter's
  inbound stays cleartext.

## Risk

- Debuggability: h2c binary framing is harder to eyeball than
  http1.1 text framing. Curl needs `--http2-prior-knowledge`.
  Wireshark decodes both. Docs must call this out.
- Bench harness: wrk speaks http1.1 only. h2 benching needs `h2load`
  (part of nghttp2) or `bombardier --http2`. Update
  `bench/AWS-RUNBOOK.md` with the extra dependency.
- Interaction with `Idempotency-Key` middleware: header handling is
  identical, but tests should cover the h2c path explicitly.
