# CLAUDE.md

Entry-point brief for coding agents (Claude Code, Cursor, etc.) working
on this repository. Human contributors: start with `README.md`, then
skim this file for the release gate and the v0.9.11-rc breaking-change
surface.

Agents shipping a breaking change: read the [Handling a breaking
change](#handling-a-breaking-change-mandatory-for-coding-agents)
section BEFORE opening a PR. Version bumps and releases are
NOT yours to approve — a user confirmation of what version WOULD
be correct is not the same as "cut it."

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

Expected on a clean `dev` (verified 2026-09-10 on `11dae74`):

| Check | Baseline |
|---|---|
| `cargo fmt --check` | clean |
| clippy (default features) | clean under `-D warnings` |
| clippy (`--features scripting-quickjs` only) | clean under `-D warnings` |
| `cargo test --no-fail-fast` | 606 passed / 0 failed / 3 ignored across 71 test binaries |
| `cargo audit --deny warnings` | 0 vulnerabilities, 0 warnings (advisory DB from RustSec) |
| `dsl-lint DSL/samples` | 64 files, 0 errors, 3 warnings (unresolved `[#…]` for webhook keys intentionally omitted from `constants.ini`) |
| `dsl-test DSL/DSL-tests` | 100 scenarios, 100 passed |
| `mdbook build` | html backend, no warnings |

`scripting-boa` and `scripting-quickjs` are mutually exclusive features;
`--all-features` will not compile. Check each set separately.

`.github/workflows/security.yml` runs `cargo audit --deny warnings` on
push, PR, and a daily 06:00 UTC cron. Exceptions live in
`.cargo/audit.toml` — currently RUSTSEC-2024-0384 (`instant`) and
RUSTSEC-2024-0436 (`paste`), both transitive-only, review date
2026-10-01.

## Handling a breaking change (mandatory for coding agents)

Something is a **breaking change** if a downstream caller must
adapt to it. On this repo that means one or more of:

- **Rust public-API surface.** Signature change (added positional
  arg, changed return type), removed / renamed pub item, moved
  pub item between modules, changed trait bounds. See T-4
  (`StepEngine::new` gained positional args) and T-5
  (`StateStore::set` → `Result<()>`) in `[CHANGELOG.md]` for the
  reference shape.
- **DSL runtime semantics.** Default value flip that alters
  behaviour, error surface widened (was OK, now Err), value shape
  change, YAML-key rename, response body / header shape. See T-10
  (multipart map-key: filename → field name) and issue #98
  (Content-Type-driven body decode).
- **HTTP wire behaviour.** Status code, response body shape,
  response header semantics. Adding a header is non-breaking;
  removing / renaming / changing meaning is breaking. See T-15
  (wrong-method-on-known-path: 404 → 405 + `Allow:`).
- **Config surface.** Renamed field, removed field, or changed
  default that flips runtime behaviour. See T-1
  (`http_response_size_limit` absent-YAML default flip).
- **Env-var behaviour.** A variable that was no-op is now
  behaviourally meaningful, or vice versa. See T-6
  (`RUUTER_HTTP_REWRITE` compiled out of release without
  `dev-http-rewrite` feature).

Non-breaking (still document, but no version-bump urgency): new
optional config field with a safe default, new boot WARN, new
pub fn/struct, bug fix restoring behaviour that was documented
as intended.

### What to do (in order)

1. **Classify against semver.**
   - **Patch RC** (`0.9.16-rc` → `0.9.17-rc`) — bug fix that
     restores documented-as-intended behaviour with no API /
     config / wire change.
   - **Minor RC** (`0.9.16-rc` → `0.10.0-rc`) — any Rust API
     signature change, any documented-behaviour change, any new
     feature. This is the common case for anything caller-visible.
   - **Major** — reserved for after v1.0.0. Do not propose.
   State the classification in the PR body.

2. **Write the CHANGELOG entry** under the top-of-file
   `[Unreleased]` heading. Use the appropriate `###` section:
   - `### Fixed` — bug fix restoring documented behaviour.
   - `### Changed (breaking)` — Rust public-API breaking change.
   - `### Changed (behavior)` — client-facing behaviour change
     (DSL, HTTP wire, env-var).
   - `### Added` — new feature (usually the trigger for a
     minor bump).

   Body MUST include: pre-fix behaviour, post-fix behaviour, a
   migration snippet with a before/after diff block, and a
   pointer to the regression-test file. The v0.9.15-rc /
   v0.9.16-rc entries are the tone reference — verbose,
   name every seam, cite issue and PR numbers.

3. **Add a regression-test file.** One dedicated
   `tests/issue_XXX_short_name.rs` (or `tests/issue_TN_...`)
   per fix. Cover the happy path AND every input that would have
   caught the pre-fix bug ("write tests that try to BREAK the
   fix"). See the `[Unreleased]` batch (T-1..T-16) for shapes:
   subscriber-captured WARNs, subprocess-spawned binaries, in-
   process axum + mockito, hand-rolled UDS listeners.

4. **Update every internal caller in the SAME PR.** For a Rust
   API change, that means every internal test fixture too. T-4
   updated ~50 test files in one PR — don't leave "will fix in
   follow-up." Post-merge cleanup PRs are a smell.

5. **PR title + body**
   - `fix(#N): ...` for bug fixes, `feat(#N): ...` for features,
     `chore(...)` for internal / infrastructure.
   - Add ` (BREAKING)` suffix on titles when the change is
     Rust-API-breaking.
   - PR body includes: the migration snippet, the semver
     classification, and the release-gate checklist.

6. **Merge conflicts on sequential PRs.** `CHANGELOG.md` and
   often `src/config/mod.rs`, `src/router/mod.rs`,
   `src/http_client/mod.rs`, `src/main.rs` will conflict when
   several PRs stack on `dev`. Force-push is blocked, so use
   `git merge origin/dev` on the feature branch (not rebase):
   ```bash
   git checkout fix/N && git merge origin/dev --no-edit
   # CHANGELOG.md: keep both entries side-by-side, origin/dev
   # first (older merges), HEAD second (newest). If both add
   # unrelated blocks inside the same function, keep both.
   # A `resolve_changelog.py` helper lives in prior release
   # branches — one-liner regex replacement of the conflict
   # markers.
   cargo build   # smoke check
   git commit --no-edit && git push
   ```

### Version bumps and releases: NEVER approve these yourself

The user reserves release authority. A confirmation of what
version WOULD be correct ("So v0.10.0-rc would be correct?") is
validation of the semver reasoning — it is NOT authorization to
execute the bump. Wait for an explicit imperative: "bump it",
"cut the release", "tag v0.X.Y", "publish now".

**Do not touch, until the user says "cut it":**

- `Cargo.toml` version field.
- `Cargo.lock` package pin for `ruuter-on-rust`.
- `README.md` version badge + `> Upgrading from vX-rc?` callout
  + docker pull recipes (there are 4 recipes in the README).
- `book/src/introduction.md` version badge.
- `CHANGELOG.md` release header (do NOT rename `[Unreleased]`
  → `[X.Y.Z-rc] - YYYY-MM-DD`).
- `CLAUDE.md` "verified YYYY-MM-DD on `<sha>`" line and the
  test-count baseline row — those are release-carrying doc.
- `CLAUDE.md` "Behaviour-change surface as of vX-rc" section —
  add a new one only at release cut.
- Git tags.
- `.github/workflows/publish.yml` (dispatch via `gh workflow
  run publish.yml` is release authority).

**When the user says "cut it":** bump every file above in
lockstep. Miss one and the container tag / docs / crate
metadata will lie about the version. RC tags on this repo use
bare `-rc` (not `-rc.N`); see
`memory/project_rc_version_convention.md` if that memory is
loaded.

### Verifying the fix before opening the PR

Run the full release gate at the top of this file. Every check
must pass on your branch:

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo clippy --all-targets --no-default-features --features scripting-quickjs -- -D warnings
cargo test --no-fail-fast
cargo audit --deny warnings -n     # -n skips the flaky remote fetch
$CARGO_TARGET_DIR/debug/dsl-lint --dsl DSL/samples --constants constants.ini
$CARGO_TARGET_DIR/debug/dsl-test --dsl DSL --tests DSL-tests --constants constants.ini
( cd book && mdbook build )
```

If `dsl-test` fails on `http_rewrite:` scenarios in release
mode, you're missing `--features dev-http-rewrite` — see T-6.

If a Rust API change breaks other feature branches on merge,
the fix belongs in the LAST PR before that batch merges, not
in a follow-up. See `chore/post-merge-t4-signature-updates` for
the recovery shape if it slips.

### Worked examples in the release history

| Change | Kind | Where to read |
|---|---|---|
| `StepEngine::new` gained `guards` + `guards_mode` args | Rust API | `[Unreleased]` T-4; PR #104 |
| `StateStore::set`/`update` → `Result` | Rust API | `[Unreleased]` T-5; PR #105 |
| Multipart map-key: filename → field name | DSL semantic | `[Unreleased]` T-10; PR #110 |
| Wrong method on known path: 404 → 405 + `Allow:` | HTTP wire | `[Unreleased]` T-15; PR #115 |
| `RUUTER_HTTP_REWRITE` behind `dev-http-rewrite` feature | Env-var + build | `[Unreleased]` T-6; PR #106 |
| `http_response_size_limit` absent-YAML default | Config surface | `[Unreleased]` T-1; PR #101 |
| `stop_in_case_of_exception` absent-YAML default | Config surface | § 0.9.15-rc; PR #95 |
| `http.*` transport failures now DSL-catchable | DSL semantic | § 0.9.15-rc; PR #94 |
| `template:` step runs target-DSL guards | HTTP + DSL wire | § 0.9.11-rc; commit `ecbfe1b` |
| `#98` Content-Type-driven body decode | DSL semantic | § 0.9.16-rc; PR #99 |

For a new breaking change, copy the shape of the closest analog
above — same CHANGELOG structure, same test-file naming, same
PR-body template.

## Behaviour-change surface as of v0.9.15-rc (issues #89 / #90 / #91 / #92)

One catchability contract change, one config-default flip, and a
docs/tooling round. Landed as three separate PRs on top of v0.9.14-rc.
Full detail in [CHANGELOG.md § 0.9.15-rc](CHANGELOG.md#0915-rc---2026-09-10).

- **`http.*` transport failures are catchable by the DSL (#89, PR #94).**
  Pre-fix, connection-refused / DNS failure / TLS-handshake error /
  read-or-write timeout on an `http.get` / `http.post` step
  propagated as `RuuterError::Http`, aborting the run and producing
  the framework's generic 500. Post-fix, the transport error is
  surfaced in-band as a stub `HttpResponse { status: 0,
  error: Some(kind), body: {error, message}, headers: {} }` bound
  to the DSL's `result:`. Author options:
    - Branch in a subsequent `check_*` switch on
      `${result.response.status == 0}` (the reporter's pattern for
      gateway/adapter DSLs emitting semantic 502s).
    - Inspect the specific kind via `${result.response.error}` —
      stable short strings: `timeout`, `connect`, `request`, `body`,
      `decode`, `unknown`. Mapping helper is public:
      `http_client::classify_transport_error`.
    - Wire an `error:` handler on the step.
    - Fall through to `next:` if neither `error:` nor an inspection
      is set.
  Policy-level pre-flight rejections (SSRF blocked, host-allowlist
  denial, malformed URL, response-size cap) still raise — those are
  ops decisions, not availability events. The allow-list-miss path
  (upstream returns a real status outside `http_codes_allow_list`)
  is unchanged: still raises when no `error:` handler is set. New
  field on `HttpResponse`: `error: Option<String>`. Pre-existing
  tests that asserted `res.is_err()` on transport failure paths
  were updated to assert on the stub shape instead.
- **`stop_in_case_of_exception` default is now `true` (#92, PR #95).**
  Rust's `bool` Default is `false`, so `#[serde(default)]` on the
  field deserialised an absent value as `false`, tripping
  `warn_on_stale_config_fields` on every boot for operators who
  never set the field. Fix: `#[serde(default =
  "default_stop_in_case_of_exception")]` returning `true`, matching
  the engine's actual behaviour (always halts on step error).
  Explicit `false` in `ruuter.yaml` still WARNs — that's the
  intended surface for "you set a value we can't honour."
- **Supported JS subset documented and empirically verified (#90,
  PR #96).** `book/src/dsl/expressions.md` rewritten with a 26-row
  support matrix pinned by `tests/issue_90_js_subset.rs` — every row
  runs against BOTH Boa (default) and QuickJS
  (`--no-default-features --features scripting-quickjs`) on every
  release-gate cycle. New "Deliberately unsupported" section names
  `console.*`, `fetch`, `require`, `eval`, `new Function`, async /
  Promise, `setTimeout`, filesystem / process — each with a
  DSL-shaped alternative. Not runtime-visible; docs + test-pin only.
- **`dsl-lint` scalar-quoting warnings across five classes (#91,
  PR #96).** New `book/src/dsl/yaml-gotchas.md`. `dsl-lint` now
  emits WARNs (never errors) for: `: ` inside an unquoted `${…}`,
  ` #` inside one, `,` inside one in flow context, values starting
  with reserved YAML metasyntax (`!`, `&`, `*`, `%`, `@`, backtick),
  and Unicode homoglyphs in structural YAML (fullwidth colon, en
  dash, em dash, hyphen). Homoglyph check is scoped to
  pre-`#`-comment structural region and skips quoted regions, so
  em dashes in doc comments / quoted prose don't false-positive.
  `dsl-lint DSL/samples` baseline unchanged (64 files, 0 errors,
  3 warnings — all pre-existing unresolved-constant refs).

## Behaviour-change surface as of v0.9.14-rc (issues #82 / #83 / #85)

Three small fixes on top of v0.9.13-rc. Landed in commit `6c699b7`
(PR #86). Full detail in
[CHANGELOG.md § 0.9.14-rc](CHANGELOG.md#0914-rc---2026-09-09).

- **Tools ship in the image (#83).** `dsl-lint` and `dsl-test` are
  under `/usr/local/bin/` in the published image; downstream CI can
  `docker run … turnerrainer/ruuter:<tag> dsl-lint --dsl DSL` with
  no full path and no separate build. `publish.yml` smoke test
  invokes `--help` on both arches before cosign signs.
- **Template header nulls dropped (#85).** A `template:` step whose
  `headers:` value evaluates to `undefined` no longer forwards
  `header: "null"` (the four-byte string) to the child DSL. Matches
  the pre-existing null-drop at `http_client` (#57) and
  `return_step`. DSLs that intentionally sent the string `"null"`
  as a header value must send it explicitly.
- **Step-key list unified (#82).** `src/steps/mod.rs::{STEP_KEYS,
  ACTION_STEP_KEYS}` are the single source of truth consumed by
  both `src/dsl/parser.rs` and `src/bin/dsl_lint.rs`. Pinned by a
  unit test + a `tests/issue_82_dsl_lint_step_recognition.rs`
  integration test that would have caught PR #80 the day it landed.
  New `DSL/samples/WS/inbound/roles.yml` closes the coverage gap
  where `dsl-lint DSL/samples` had no shipped `ws_tag:` DSL.

## Behaviour-change surface as of v0.9.13-rc (issue #79 + PR #80)

Issue #79 (sviljus): a `template:` step inside a guard whose target
is under the same guard used to stack-overflow the tokio worker
(fatal-abort regression from v0.9.11-rc H1). Landed in commit `688a4a2`
(PR #81). PR #80 (also sviljus): `dsl-lint` now accepts `ws_tag:`
steps. Full detail in
[CHANGELOG.md § 0.9.13-rc](CHANGELOG.md#0913-rc---2026-09-09).

- **Guard `template:` recursion is broken by a per-request stack.**
  `ExecutionContext::guard_stack` holds keys of guards currently
  mid-execution. All three guard-loop call sites (HTTP entry, WS
  upgrade, template step) push before running and pop via RAII
  drop-guard. Template step filters `applicable_guards_for(target)`
  against the stack, so same-key cycles are skipped. Not a breaking
  change for any correctly-shaped DSL — the pre-fix crash made
  shipping the bad shape impossible.
- **`MAX_GUARD_DEPTH = 32`** — belt-and-braces for exotic
  mutual-recursion (three-guard cycle) patterns. Breach surfaces as
  `RuuterError::DslExecution { step: "guard", … }` with a diagnostic
  listing every key on the stack.
- **`dsl-lint` now accepts `ws_tag:`.** Two-year drift between the
  parser's `ACTION_KEYS` and the linter's `KNOWN_STEP_KEYS` closed.
  Follow-up work (single source of truth + regression test + sample
  DSL) tracked in issue #82.

## Behaviour-change surface as of v0.9.12-rc (issue #75)

Issue #75 landed in commit `223f2a1` (PR #76) — `declaration.allowlist`
contract fixes. None of these are breaking for a correctly-shaped
DSL, but they change observable behaviour on the buggy paths the
reporter identified. Full detail in
[CHANGELOG.md § 0.9.12-rc](CHANGELOG.md#0912-rc---2026-09-08).

- **Guards run BEFORE `allowlist:` stripping.** A route's
  `allowlist.headers` no longer strips headers the parent guard
  reads. If a route had unexplained 401/400 from its parent guard
  after adding `allowlist.headers`, retest — the workaround (drop
  the allowlist) is no longer needed.
- **`required: false` is honoured.** Structured `allowlist.body:`
  entries no longer force presence unless `required: true` is
  explicit. Legacy flat `allowed_body: [...]` unchanged.
- **Missing required → `400`, not `500`.** Client-side tests that
  asserted `status == 500` on this path must flip to `400`. Body
  shape unchanged.
- **Body `type:` mismatch → `400`.** Declared `type: string` /
  `integer` / etc. is now enforced at the wire. If any DSL declared
  a type for OpenAPI purposes while accepting the wrong type at the
  wire, callers now see `400`.
- **New posture: `additive: true`.** Third choice alongside `strict:`
  — allowlist is documentation-only, undeclared fields pass through.
  Mutually exclusive with `strict:` (parse-time error if both set).
- **Guards can declare their own contract.** `required:`,
  `required_one_of`, and body `type:` are enforced against the raw
  request before a guard's steps run. Guards with only
  `override_ancestors: true` are unaffected.
- **New: `allowlist.required_one_of`.** Per-section OR-of-alternatives
  groups. Motivating case: "X-Api-Key OR X-Internal-Service-Token".

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
