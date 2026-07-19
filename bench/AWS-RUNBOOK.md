# Running the bench on an isolated host (AWS or bare-metal)

Localhost benching on a developer laptop is dominated by noise: browser tabs, IDE indexing, Docker containers, thermal throttling, other agents. A ±20% run-to-run swing is normal and drowns out feature-level deltas smaller than that.

For headline-grade numbers — anything you plan to share with stakeholders or use as a shipping-perf claim — run the bench on a single-tenant host with nothing else on it.

## Host requirements

- Linux x86_64 (kernel ≥ 5.15 recommended for the modern epoll paths)
- ≥ 4 cores (bench uses `-t4 -c64` by default)
- Nothing else on the box during the run: no background containers, no cron jobs, no OS updates in flight
- Kernel tunings (see below)

## AWS instance-type suggestions

- **c7i.xlarge** (4 vCPU, 8 GB, Intel) — cheapest sensible option, ~$0.17/hr on-demand in us-east-1. Fine for run-to-run stability at the tolerances the harness uses (15–30%).
- **c7g.xlarge** (4 vCPU Graviton) — cheaper (~$0.14/hr), similar stability. Note: numbers will not compare 1:1 to x86 hosts.
- **c7i.4xlarge** (16 vCPU) — headroom for `wrk -t8 -c256` style stress tests. Use when you want to push the framework baseline toward its actual ceiling.

Avoid:
- **t3/t4g** (burstable) — CPU credits distort throughput after the first ~60 s. Never bench on these.
- **m/r** classes for CPU-bound work — memory-optimised, not compute; irrelevant here.

## Kernel + net tunings (one-shot at boot)

```bash
# Raise ephemeral port range (wrk holds many sockets)
sudo sysctl -w net.ipv4.ip_local_port_range="10000 65535"

# TIME_WAIT reuse for the fast reconnect loop wrk drives
sudo sysctl -w net.ipv4.tcp_tw_reuse=1

# Raise the FD ceiling — 64 wrk connections × many keep-alive is small,
# but the second-instance harness (043 A/B) opens more
ulimit -n 65535

# UDS tests: make sure /tmp is a real filesystem, not tmpfs-with-noexec
mount | grep '/tmp '   # if noexec: pass an alternate socket dir
```

## Bench workflow on a fresh host

```bash
# 1. Prereqs
sudo apt-get update
sudo apt-get install -y wrk jq python3 python3-yaml build-essential curl pkg-config libssl-dev

# 2. Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
source ~/.cargo/env

# 3. Repo
git clone https://github.com/turnerrainer/Ruuter.git ruuter
cd ruuter
git checkout dev
git log --oneline -5    # confirm the SHA you expect

# 4. Build
cargo build --release --bin ruuter-on-rust --bin dsl-lint --bin dsl-test

# 5. Baseline (median of 5 — 5 runs of 6 scenarios ≈ 5 min)
bench/refresh-baseline.sh --runs 5 --port 8081 --skip-build

# 6. A/B comparisons — 3 features × 2 configs × 3 runs ≈ 15 min
RUNS=5 bench/run-ab-comparison.sh | tee /tmp/ab-report.log
```

## Interpreting the numbers

- The harness prints ALL runs; the report is your responsibility. Take the **median** of N (never the mean — one thermal spike dominates a mean).
- Cross-host comparison is only valid when the hosts are the same class AND the same kernel. Never quote "AWS beat laptop by X%".
- The A/B rule: only trust a delta bigger than the run-to-run swing on the *same* scenario. If the same-config runs vary 40% and you see a +30% "improvement", that's noise.
- For 043 in particular: TCP loopback benefits from reqwest's keep-alive pool, UDS v1 doesn't. A UDS win on this benchmark requires the keep-alive pool follow-up.

## Cost math

- c7i.xlarge, 30 min of benching: ~$0.09
- c7i.4xlarge, 30 min: ~$0.35
- Never leave the instance running after the bench. Terminate, don't just stop.

## Comparing local vs isolated numbers

Bench both on the same day if possible, then diff the ratio, not the absolute rps. Example: if `thin-dsl` shows 70k rps on laptop and 90k rps on c7i.xlarge, the ratio (90/70 = 1.29×) is the "isolation multiplier" — apply it to laptop numbers to estimate what the same code would do on the isolated host. Not perfectly accurate, but useful for guiding release decisions between full bench runs.
