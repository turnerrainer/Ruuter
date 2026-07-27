# Docker

Ruuter ships as a multi-arch (`linux/amd64` + `linux/arm64`) container
image, published on **release tag** push (`v<major>.<minor>.<patch>`)
by the [publish workflow](https://github.com/turnerrainer/Ruuter/blob/dev/.github/workflows/publish.yml).

## Where to pull from

| Registry | Repository | Recommended for |
|---|---|---|
| **Docker Hub** | `turnerrainer/ruuter-on-rust` | Discoverability, casual pulls |
| **GHCR** | `ghcr.io/turnerrainer/ruuter` | High-volume / anonymous pulls (no rate limit) |

Both registries carry the same digests. Pick either. Tag conventions:

| Tag         | Meaning                                              |
|-------------|------------------------------------------------------|
| `0.7.0`     | Immutable version. Pin this in production.           |
| `0.7`       | Latest patch on the 0.7 line. Auto-updates on 0.7.x. |
| `latest`    | Whatever the most recent release tag pointed at. Fine for dev; footgun in prod. |

## Verify the image (cosign)

Every published digest is signed keyless by the GitHub Actions
workflow that produced it. Sigstore holds the transparency log entry;
`cosign` verifies the manifest against the exact workflow identity.

```bash
cosign verify turnerrainer/ruuter-on-rust:0.7.0 \
    --certificate-identity-regexp \
      "^https://github.com/turnerrainer/Ruuter/\.github/workflows/publish\.yml@refs/tags/v.*$" \
    --certificate-oidc-issuer "https://token.actions.githubusercontent.com"
```

A successful verify prints the annotations block and exits `0`.
Failure means the image was NOT produced by the shipped publish
workflow — do not run it.

## Provenance and SBOM

`docker/build-push-action` attaches an in-toto provenance attestation
(build steps, source ref, materials) and an SPDX SBOM to each
multi-arch manifest. Inspect with:

```bash
docker buildx imagetools inspect --format '{{ json .Provenance }}' \
    turnerrainer/ruuter-on-rust:0.7.0 | jq .
docker buildx imagetools inspect --format '{{ json .SBOM }}' \
    turnerrainer/ruuter-on-rust:0.7.0 | jq '.SPDX.packages | length'
```

## Run directly

The fastest way — no clone, no build:

```bash
docker run -d --name ruuter -p 8080:8080 \
    turnerrainer/ruuter-on-rust:0.7.0
```

The image bakes in `DSL/samples/` so `/samples/*` endpoints work out
of the box. Mount your own tree to replace them:

```bash
docker run -d --name ruuter -p 8080:8080 \
    -v $(pwd)/DSL:/app/DSL:ro \
    -v $(pwd)/constants.ini:/app/constants.ini:ro \
    turnerrainer/ruuter-on-rust:0.7.0
```

## Compose (production-hardened)

The shipped `docker-compose.yml` uses the local build context, but the
same securityposture works with the published image — swap the
`build:` block for `image:`.

```yaml
services:
  ruuter-on-rust:
    image: turnerrainer/ruuter-on-rust:0.7.0
    container_name: ruuter-on-rust
    ports:
      - "8080:8080"
    volumes:
      - ./DSL:/app/DSL:ro
      - ./constants.ini:/app/constants.ini:ro
      # - ./ruuter.yaml:/app/ruuter.yaml:ro          # optional
    environment:
      - RUST_LOG=info
    restart: unless-stopped
    read_only: true
    tmpfs:
      - /tmp:size=64M
    security_opt:
      - no-new-privileges:true
    cap_drop:
      - ALL
    deploy:
      resources:
        limits:
          cpus: '2.0'
          memory: 512M
        reservations:
          memory: 128M
    healthcheck:
      test: ["CMD", "curl", "-fsS", "http://localhost:8080/health"]
      interval: 30s
      timeout: 10s
      retries: 3
      start_period: 40s
```

## Bring up

```bash
docker compose up -d
docker compose logs -f ruuter-on-rust
```

## Reload after DSL change

DSLs are read at boot only. After editing files under `DSL/`:

```bash
docker compose restart ruuter-on-rust
```

Sub-second reload on the sample corpus (~60 DSLs). For a zero-restart
workflow during development, see [Hot reload](./hot-reload.md).

## Image contents

- Multi-stage: `rust:1.88-slim` → `debian:bookworm-slim`
- Runtime deps: `libssl3`, `ca-certificates`, `curl` (for the
  healthcheck), `tini` (PID 1)
- Non-root user (uid 1000)
- Final image: ~135 MB per platform
