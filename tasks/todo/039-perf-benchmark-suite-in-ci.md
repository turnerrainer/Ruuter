# 039 — Continuous perf benchmark suite

## Filed

2026-07-15 — motivated by needing to substantiate a "Ruuter is
extremely low performance + huge AWS costs" claim from a stakeholder.
The 0.4.0 numbers cited (60 k rps framework, 1-3 k rps DSL, 11 MiB
idle, 404 ms cold start, 34 MiB under load) came from an ad-hoc `wrk`
session and are not tracked between commits. Regressions are invisible.

## Problem

Every perf-adjacent change (tasks 036, 037, 038; also touched
by any change to router middleware, engine dispatch, Boa version bump,
tokio version bump) needs a delta measurement, not a vibe check.

## Fix

Ship a `bench/` directory + a GitHub Actions workflow:

### `bench/` scenarios

Each scenario is a `.bench.yml` file describing:

- Endpoint under test (path, method, headers, body).
- Target env (docker compose service name).
- wrk shape: `-t <N>` `-c <N>` `-d <duration>`.
- Baseline expectations: `min_rps`, `max_p50_ms`, `max_p99_ms`.

Recommended scenarios (must-have):

| Scenario | Purpose |
|---|---|
| `framework-baseline` (`/health`) | axum ceiling; regressions here mean
  the framework got slower, not the DSL engine |
| `thin-dsl` (`/samples/basic/hello`) | Boa setup cost per request |
| `js-heavy` (`/samples/variables/complex-object`) | assign + object literal |
| `path-params` (`/samples/things/abc/legs`) | route-resolution cost |
| `openapi-serve` (`/_/openapi.json`) | cached-response ceiling |
| `iterate-1k` | iterate step over 1 000 elements |
| `guard-stack` | 2-deep guard chain + main DSL |
| `ws-echo-throughput` | frames-per-second on `/samples/echo` |

### CI workflow

- New `.github/workflows/perf.yml`.
- Triggered on push to `dev` + `workflow_dispatch`.
- Runs on a fixed AWS runner size (or GH-hosted `ubuntu-latest`, with
  the caveat that noise is high there — record a moving 10-run median).
- Boots the compose stack, warms it, runs each scenario, compares
  against `bench/baseline.json`.
- Fails the workflow if any scenario is >15% below baseline.
- Uploads results as an artifact + posts a summary comment on the
  triggering commit / PR.

### Baseline refresh

Owner runs `bench/refresh-baseline.sh` after an intentional perf
improvement (e.g. task 036 lands) and commits the new `baseline.json`.

## Acceptance

- `bench/` directory with the 8 scenarios above.
- `bench/baseline.json` from 0.4.0 committed as the starting point.
- `.github/workflows/perf.yml` green on the current tree.
- README's "Performance" section (new — doesn't exist) points at the
  live-updated results.

## Non-goal

Micro-benchmarks of individual step executors. `cargo bench` is fine
for those; this suite is about end-to-end HTTP request throughput.
