# 056 — Add `cargo audit` gate to CI (h2ck.me S8c) — **REQUIRED**

## Filed

- 2026-07-20 — surfaced by h2ck.me pre-publication audit (finding
  S8c, `REVIEW.md`).
- **2026-07-20 (later)** — the first fix round shipped tasks
  054/055 (deps clean) but skipped 056. Advisory-clean state now
  depends on staying pinned; any `cargo update` can silently
  regress. See `POST-FIX-REVIEW-2.md`. This task is upgraded from
  "recommended" to **required** for release-readiness, with a
  drop-in playbook below.

## Severity

**Medium** — process control gap. Without a CI gate, tasks
054/055's clean floor is only true today. The RustSec DB gets new
entries roughly weekly; a silent regression via `cargo update` or
even against an UNCHANGED `Cargo.lock` (because someone filed a
new advisory against a currently-pinned crate) goes unnoticed
until the next manual audit.

## Current state (2026-07-20)

- `.github/workflows/` contains `docs.yml`, `perf.yml`, `tests.yml`
  — no `security.yml`, no `audit` step in any existing job.
- No `.cargo/audit.toml` — no place for justified exceptions.
- `cargo audit` output right now: **0 errors, 2 warnings**
  (`instant` unmaintained, `paste` unmaintained; both transitive
  through non-Ruuter crates, no fix path).

The tree is clean enough that a CI gate lands green on the first
run. Ship this now while there's no debt.

## Drop-in playbook

### Step 1 — the workflow

Create `.github/workflows/security.yml` verbatim. It mirrors the
shape of the existing `tests.yml` (concurrency, checkout, cargo
registry cache) so future maintainers see one convention across
workflows.

```yaml
name: security

on:
  push:
    branches: [main, dev]
  pull_request:
    branches: [main, dev]
  # Weekly refresh: catches advisories filed against an unchanged
  # Cargo.lock (the case a push/PR trigger would MISS entirely).
  # Monday 06:00 UTC — off-peak; before the working week starts.
  schedule:
    - cron: "0 6 * * 1"
  # Manual re-run knob for ops.
  workflow_dispatch:

concurrency:
  group: security-${{ github.ref }}
  cancel-in-progress: true

env:
  CARGO_TERM_COLOR: always

jobs:
  audit:
    name: cargo audit
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      # Prefer the prebuilt binary over `cargo install` — takes ~2s
      # vs ~60s and does not thrash the cargo cache. `taiki-e`'s
      # install-action is the community standard for Actions-tool
      # binary installs.
      - name: Install cargo-audit
        uses: taiki-e/install-action@v2
        with:
          tool: cargo-audit

      # Refresh the advisory-db from RustSec BEFORE the audit. The
      # cargo-audit binary would do this itself on first run, but
      # calling it explicitly makes the log line and any fetch
      # failures unambiguous.
      - name: Fetch RustSec advisory database
        run: cargo audit fetch

      # Hard-fail on vulnerabilities. `cargo audit` (default mode)
      # exits non-zero on `error:` (RUSTSEC vulnerability advisory)
      # and prints — but does not fail — on `warning:`
      # (unmaintained, notice). That is the correct posture for
      # this project today (instant + paste are transitive-only
      # unmaintained crates we cannot fix ourselves).
      - name: cargo audit (vulnerabilities)
        run: cargo audit

      # Informational: surface unmaintained crates as an annotation
      # so PR reviewers see the current state, without blocking
      # the merge. If the project decides to tighten later, change
      # `|| true` to a hard fail — but do that in a separate PR
      # after clearing `instant`/`paste` transitives.
      - name: cargo audit (unmaintained — informational)
        run: cargo audit --deny warnings || true
```

### Step 2 — the exceptions file

Create `.cargo/audit.toml`. Empty `ignore` today; ready for
justified additions later.

```toml
# .cargo/audit.toml
#
# Documented exceptions to `cargo audit`. Every entry MUST:
#   - reference a RustSec ID
#   - link to a tracking issue in this repo or upstream
#   - state a review date (`YYYY-MM-DD`) after which we re-evaluate
#
# Prefer FIXING over ignoring. An entry here is technical debt.

[advisories]
ignore = [
    # Example (uncomment when needed):
    # "RUSTSEC-YYYY-NNNN",   # <crate>: <why unavoidable>; issue #NNN; review YYYY-MM-DD
]

# Optionally add an informational-warnings block later if the
# project decides which unmaintained transitives are noise vs
# real risk. For now leave defaults.
```

### Step 3 — smoke-check locally

```bash
cargo install --locked cargo-audit
cargo audit          # expect: 0 vulnerabilities, ≤2 warnings
```

If this exits with an `error:`, tasks 054 + 055 have regressed —
fix the dep before adding CI.

### Step 4 — verify the workflow

Push the branch and watch the `security / cargo audit` check run.
Expected outcome: green, with 2 unmaintained warnings printed in
the second step's log.

To manually re-run (e.g. after publishing a fix): the
`workflow_dispatch` trigger in the file adds a "Run workflow"
button on the Actions tab.

## Acceptance (definition of done)

**All of these must hold** before closing the task:

- [ ] `.github/workflows/security.yml` exists with the drop-in
      YAML above (or equivalent — the trigger/cron/tool-install
      shape MUST match).
- [ ] `.cargo/audit.toml` exists with an empty `ignore` list and
      the exception-policy comment above.
- [ ] A CI run on the merged branch shows the `security / cargo
      audit` check green.
- [ ] The vulnerability step (`cargo audit`) exits 0.
- [ ] The unmaintained-informational step runs and its log lists
      exactly `instant` (RUSTSEC-2024-0384) and `paste`
      (RUSTSEC-2024-0436). Any additional warning means a dep
      moved and warrants a look.
- [ ] The weekly cron entry (`schedule: - cron: "0 6 * * 1"`) is
      present.
- [ ] CHANGELOG under `### Security`:
      "CI now runs `cargo audit` on every push, PR, and weekly.
      Fails the build on any vulnerability advisory."

## Testing the gate actually catches things

To convince yourself the gate is wired end-to-end (do NOT commit
this to main):

```bash
# On a scratch branch — temporarily add a known-vulnerable dep.
cargo add tokio@0.1  # RUSTSEC-2020-0072 unsound Sink impls
git commit -am "TEMP: prove cargo-audit gate fails on vulnerable dep"
git push
# Observe the security workflow fail on this branch. Then revert.
git reset --hard HEAD~1
git push --force-with-lease   # scratch branch only!
```

## Non-goals

- SBOM generation (`cargo cyclonedx`) — separate task if wanted.
- License scanning — separate task.
- `cargo-vet` supply-chain review — separate, larger task.
- Automated PR-opens for advisory-fix bumps (`dependabot` /
  `renovate`) — a good next step but decoupled from this gate.

## Cross-reference

- `projects/Ruuter-on-Rust/REVIEW.md § S8`
- `projects/Ruuter-on-Rust/REMEDIATION.md § S8c`
- `projects/Ruuter-on-Rust/POST-FIX-REVIEW-2.md` — flags 056 as
  still-open.
- Depends on: tasks 054, 055 (both landed).

Effort estimate: 20 min (workflow file + exceptions file + one
scratch-branch test run to confirm the failure path fires).
