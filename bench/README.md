# Perf benchmark suite

Continuous perf tracking for Ruuter-on-Rust. Answers the question
"did this change slow anything down?" with numbers instead of vibes.

## Layout

```
bench/
├── scenarios/*.bench.yml      # one file per scenario (see schema below)
├── baseline.json              # captured expected numbers (regenerated on intentional perf work)
├── run.sh                     # build → boot → wrk each scenario → results.json
├── refresh-baseline.sh        # capture median of N runs → baseline.json
├── compare.py                 # diff results.json vs baseline.json, fail on regression
└── README.md                  # you are here
```

## Scenario schema

```yaml
name: thin-dsl               # unique, matches key in baseline.json
route: /samples/basic/hello  # path on the running server
method: GET                  # GET | POST | PUT | DELETE | PATCH
headers:                     # optional
  Authorization: "Bearer foo"
body: null                   # optional string; if present, sent as request body
wrk:
  threads: 4
  connections: 64
  duration: 10s
```

## Local usage

```bash
# One-shot measurement, writes bench/results.json
bench/run.sh

# Compare against the committed baseline
bench/compare.py --baseline bench/baseline.json --current bench/results.json

# After an intentional perf change (e.g. task 037 landing):
bench/refresh-baseline.sh --runs 5     # median of 5 runs, less jitter
git add bench/baseline.json
git commit -m "bench: refresh baseline after task NNN"
```

Prereqs: `cargo`, `wrk`, `jq`, `python3` (with the standard `yaml`
module).

## CI

`.github/workflows/perf.yml` runs the suite on manual dispatch
(`workflow_dispatch`), taking the median of N runs.

**Why not `push`-triggered?** GitHub-hosted `ubuntu-latest` runners
have high variance on tail percentiles (shared hardware, noisy
neighbors). Auto-gating on every push produces false-regression
noise. When a dedicated bare-metal runner is available, switch to
`on: push` against `dev` and tighten `rps_tolerance` from 0.25 to
0.15.

## Interpreting numbers

- **rps** — headline metric; regression gate is throughput.
- **p50_ms** — median latency; warn-only in CI (tail-percentile noise
  on shared runners).
- **p75/p90/p99_ms** — collected for diagnostic, not gated.

The `run.sh` output normalizes wrk's mixed-unit latency reporting
(us/ms/s) into milliseconds so JSON comparisons work uniformly.

## Variance gotcha

A **single** `run.sh` output typically drifts ±20-30% from a
**3-run median** on a shared laptop or `ubuntu-latest` runner, even
with the workload unchanged. Comparing a single-shot vs the committed
median baseline will spuriously trip the gate on ~1 in 4 runs.

Rule: for any meaningful comparison, both sides must be
median-of-N. The CI workflow does this automatically. Locally, if
you want to check "did my change regress anything," run
`refresh-baseline.sh --runs 3` before AND after your change and
diff the two files.

## Adding a scenario

1. Create `bench/scenarios/<name>.bench.yml` matching the schema
   above.
2. Run `bench/refresh-baseline.sh` to add its numbers to the baseline.
3. Commit both files together.

## Removing / renaming

Rename or remove both the scenario YAML and its entry in
`baseline.json`. `compare.py` treats a scenario missing from the
current run as a failure — that's intentional so a silent removal
doesn't hide a regression.

## Scenarios shipped

| Scenario | Purpose |
|---|---|
| `framework-baseline` | axum ceiling (`/health`) — regressions here mean the framework itself slowed down |
| `thin-dsl` | one-step DSL returning a literal string — tracks task 037's literal fast-path |
| `js-heavy` | assign + object literal + `Date.now()` — tracks Boa slow-path baseline |
| `path-params` | path extraction + switch — router-resolution cost |
| `openapi-serve` | `/_/openapi.json` — cached-response ceiling |
| `guarded` | guard chain + main DSL — guard-resolution + guard-DSL cost |

Not (yet) shipped: `iterate-1k` (needs a dedicated DSL), `ws-echo`
(wrk doesn't do WS; needs a Rust-side bench binary). File follow-up
tasks if these become priorities.
