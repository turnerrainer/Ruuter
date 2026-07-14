# Docker

The shipped `docker-compose.yml` is production-hardened out of the box.

## Compose

```yaml
services:
  ruuter-rs:
    build:
      context: .
      dockerfile: Dockerfile
    image: ruuter-on-rust:0.4.0
    container_name: ruuter-rs
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
docker compose up -d --build
docker compose logs -f ruuter-rs
```

## Reload after DSL change

DSLs are read at boot only. After editing files under `DSL/`:

```bash
docker compose restart ruuter-rs
```

Sub-second reload on the sample corpus (45 DSLs).

## Image

- Multi-stage: `rust:1.88-slim` → `debian:bookworm-slim`
- Runtime deps: `libssl3`, `ca-certificates`, `curl` (for the healthcheck), `tini` (PID 1)
- Non-root user (uid 1000)
- Final image: ~135 MB
