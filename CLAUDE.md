# CLAUDE.md

Entry-point brief for coding agents (Claude Code, Cursor, etc.) working
on this repository. Human contributors: start with `README.md`, then
skim this file for the release gate and the v0.9.11-rc breaking-change
surface.

## What this repo is

Rust re-implementation of [buerokratt/Ruuter](https://github.com/buerokratt/Ruuter)
— a declarative REST/WebSocket router driven by YAML DSLs on disk. A
DSL at `DSL/<project>/<METHOD>/<path>.yml` becomes the route
`<METHOD> /<project>/<path>`. `<stem>.guard.yml` next to a directory
protects every DSL under it.

Public. Apache-2.0. `main` is a stub pointing at active development on
`dev`. All PRs target `dev`.

## Release gate (run before every PR)

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo clippy --all-targets --no-default-features --features scripting-quickjs -- -D warnings
cargo test --no-fail-fast
cargo audit --deny warnings
./target/debug/dsl-lint --dsl DSL/samples --constants constants.ini
./target/debug/dsl-test --dsl DSL --tests DSL-tests --constants constants.ini
( cd book && mdbook build )
```

Expected on a clean `dev` (verified 2026-09-07 on `ecbfe1b`):

| Check | Baseline |
|---|---|
| `cargo fmt --check` | clean |
| clippy (default features) | clean under `-D warnings` |
| clippy (`--features scripting-quickjs` only) | clean under `-D warnings` |
| `cargo test --no-fail-fast` | 488 passed / 0 failed / 3 ignored across 64 test binaries |
| `cargo audit --deny warnings` | 0 vulnerabilities, 0 warnings (advisory DB from RustSec) |
| `dsl-lint DSL/samples` | 63 files, 0 errors, 3 warnings (unresolved `[#…]` for webhook keys intentionally omitted from `constants.ini`) |
| `dsl-test DSL/DSL-tests` | 100 scenarios, 100 passed |
| `mdbook build` | html backend, no warnings |

`scripting-boa` and `scripting-quickjs` are mutually exclusive features;
`--all-features` will not compile. Check each set separately.

`.github/workflows/security.yml` runs `cargo audit --deny warnings` on
push, PR, and a daily 06:00 UTC cron. Exceptions live in
`.cargo/audit.toml` — currently RUSTSEC-2024-0384 (`instant`) and
RUSTSEC-2024-0436 (`paste`), both transitive-only, review date
2026-10-01.

## Breaking-change surface as of v0.9.11-rc (h2ck.me audit fixes)

Five hardening changes landed in commit `ecbfe1b` (PR #72). Any DSL
or operator config from ≤ v0.9.10-rc must be reviewed against these
four contract changes before upgrading. Full detail lives in
[CHANGELOG.md § 0.9.11-rc](CHANGELOG.md#0911-rc---2026-09-04).

### 1. `template:` calls now run the target DSL's guards (H1)

A public DSL that says `template: admin/things` used to bypass
`POST/admin/.guard.yml`. It no longer does. The template step runs
every applicable guard against the CHILD context; a guard returning
`>= 400` short-circuits and the caller's `${result}` binds the guard's
response.

**Grep for callers to review:**

```bash
grep -rE "^\s*template:\s*" DSL/
```

Any hit whose target path is under a guarded directory needs one of:

- Forward the required auth explicitly:
  ```yaml
  - template:
      dsl: admin/things
      headers:
        authorization: "${incoming.headers.authorization}"
  ```
- Or restructure so the shared logic lives in a non-guarded
  `templates/shared/…` DSL that both the guarded and public callers
  reach.

### 2. WebSocket upgrades now run guards (H2)

Any WS DSL under a project with a `.guard.yml` now runs that guard on
connect. Pre-fix, `handle_ws_upgrade` skipped guards entirely.

**Grep for WS DSLs whose project has a guard:**

```bash
for ws in $(find DSL -path '*/WS/*.yml'); do
    proj=$(echo "$ws" | cut -d/ -f2)
    ls DSL/$proj/*.guard.yml DSL/$proj/**/*.guard.yml 2>/dev/null | \
        head -1 | xargs -I{} echo "$ws  ->  {}"
done
```

The guard runs against a synthesized context whose `incoming.headers`
and `incoming.params` carry the WS handshake. There is no body. Guards
that dereferenced `incoming.body` will now fail; rewrite them against
headers/params.

### 3. `/_/openapi.json` is admin-gated (M1)

Previously mounted on the public router. Now mounted on
`admin_router()` alongside `/_/sources` and `/_/unguarded`. Any client
scraping `curl http://…/_/openapi.json` unauthenticated will now get a
404 unless `RUUTER_ADMIN_ENABLED=true` is set (and paired with
reverse-proxy auth in front of `/_/*`).

**Best-practice env for a public deployment:**

```bash
# admin endpoints OFF on the public listener
# (RUUTER_ADMIN_ENABLED unset or 'false')
```

**Best-practice env for an admin-only listener (behind proxy auth):**

```bash
RUUTER_ADMIN_ENABLED=true
```

Never expose the admin listener directly; put mTLS or OIDC in front.

### 4. `RUUTER_HTTP_REWRITE` warns loudly in release builds (M2)

The env var is documented test-only but the code path was compiled
into every release binary. Setting it silently disabled `check_ssrf`
allowlists and `block_private_networks` for the rewritten origin. It
now logs a `WARN` at boot in release builds, right after the "Loaded
config from …" line, so a misconfiguration is visible before the first
outbound request.

**Grep your Docker Compose / K8s manifests / systemd units:**

```bash
grep -rE "RUUTER_HTTP_REWRITE" .
```

**Best practice:** never set `RUUTER_HTTP_REWRITE` in prod. It exists
purely for `dsl-test` and integration harnesses. If you see it in a
release manifest, remove it.

### 5. WS outbound writer channels bounded, default 256 (M3)

Every WS registration used to allocate `mpsc::unbounded_channel`. Slow
or dead readers combined with `broadcast_where` fan-out could grow the
process memory without limit. Senders are now bounded
`mpsc::Sender<Outbound>` at `DEFAULT_OUTBOUND_QUEUE_CAPACITY = 256`.

**Behavioural change:** `WsRegistry::send` returns `Err` when a peer's
queue is full. `broadcast` / `broadcast_where` skip full peers so a
single slow subscriber can't stall fan-out to fast ones.

**DSL authors:** if a step assumed `ws_send` always succeeded, wire an
`error:` branch or accept "the framework logs and continues" as the
fallback.

## Config surface (best-practice defaults)

The full config is in `src/config/mod.rs` and every knob has a safe
default. Only override what you need.

| Setting | Default | When to change |
|---|---|---|
| `RUUTER_ADMIN_ENABLED` | unset (off) | Set to `true` ONLY on a listener you have your own auth in front of |
| `RUUTER_HTTP_REWRITE` | unset | Never set in prod — see M2 above |
| `internal_requests.disabled` | `false` | Set `true` in a locked-down deployment that never needs to reach internal services |
| `block_private_networks` (per outbound origin) | `true` | Only `false` for a test harness binding on `127.0.0.1`; production always `true` |
| `proxy.trusted` | `[]` | Populate with your load balancer's IPs; XFF / X-Real-IP are only promoted to `incoming.origin` when the direct TCP peer is in this list |
| WS outbound queue capacity | `256` | Only raise if you have a legitimate slow-writer burst pattern; document in a comment when you do |

## Config file resolution

At boot Ruuter looks for a YAML config file in this priority:

1. `--config <path>` CLI flag
2. `RUUTER_CONFIG=<path>` env
3. `./ruuter.yaml` or `./ruuter.yml` in the working directory
4. Built-in defaults if none of the above exists

Worked example with every top-level knob:
`DSL/samples/ruuter.yaml.example`. Copy to `./ruuter.yaml` and edit.

## Repo conventions worth knowing

- **`cargo fmt` and `cargo clippy -D warnings` are hard gates.** Land
  fmt fixes and lint fixes in the same commit as the code that
  breaks them; do not ship a PR that fails the release gate.
- **`Cargo.lock` is tracked.** Binary crate — needed for reproducible
  Docker builds and locked CI runs.
- **DSL YAML samples in `book/`** must be pure block style. No
  flow-style `{ … }` maps, no inline arrays. A reader must be able to
  copy any snippet straight into a DSL file.
- **`state.*` is a per-process cache**, not a KV store. No TTL, no
  eviction, no persistence. Container restart wipes it. Anything
  durable belongs upstream (typically Resql).
- **RC tags use bare `-rc`, not `-rc.N`**. Every version-carrying
  file must match on release day: `Cargo.toml`, `Cargo.lock`, `README.md`
  badge + docker pull recipes, `book/src/introduction.md` badge,
  `CHANGELOG.md` entry heading.
- **Container is hardened by default** in `docker-compose.yml`:
  `read_only`, `no-new-privileges`, `cap_drop: ALL`, memory and CPU
  limits. Do not remove any of these when adding a service.

## Where to look for more detail

| Topic | File |
|---|---|
| Full CHANGELOG | `CHANGELOG.md` (top-of-file `[Unreleased]` + `[0.9.11-rc]`) |
| SSRF match rules + blocklist | `book/src/framework/ssrf.md` |
| `state` step semantics + samples | `book/src/dsl/steps/state.md` |
| Idempotency (DSL-side) | `book/src/dsl/idempotency-pattern.md` |
| CI security gate config | `.github/workflows/security.yml`, `.cargo/audit.toml` |
| Test map (h2ck.me v0.9.10-rc regressions) | `tests/security_h2ck_v0_9_10_rc.rs`, `tests/security_hardening.rs`, `tests/security.rs` |
| DSL reference | `book/src/SUMMARY.md` (rendered) or `docs/DSL_REFERENCE.md` (single page) |
