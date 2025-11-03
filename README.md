# Ruuter-RS

**Rust implementation of Ruuter - Declarative REST Router**

Version: 0.3.0-docker-support
Author: Rainer Türner
Status: Functional Core Complete

## Features

- ✅ File-system-based REST routing
- ✅ YAML DSL parser
- ✅ JavaScript expression evaluation (Boa engine)
- ✅ HTTP client (GET/POST/PUT/DELETE)
- ✅ All core step types (assign, return, http, switch, log)
- ✅ Constants.ini support
- ✅ Configuration system
- ✅ Error handling
- ✅ Docker support
- ⚠️ Template step (basic)
- ⚠️ Guards system (placeholder)

## Quick Start

### Docker (Recommended)

```bash
docker-compose up -d
```

Server starts on `http://localhost:8080`

### Local Build

```bash
cargo build --release
cargo run --release
```

## Example DSL

```yaml
# DSL/samples/GET/ping.yml
response:
  status: 202
  return: pong
```

Access: `GET http://localhost:8080/samples/ping`

Health check: `GET http://localhost:8080/health`

## Docker Configuration

The Docker image uses multi-stage builds for minimal size:
- Build stage: Rust 1.75
- Runtime stage: Debian slim
- Non-root user for security
- Volume mounts for DSL files and constants

### Volumes

- `./DSL:/app/DSL:ro` - DSL files (read-only)
- `./constants.ini:/app/constants.ini:ro` - Constants (read-only)

### Environment

- `RUST_LOG=info` - Logging level (debug, info, warn, error)

## Documentation

- [Development TODO](docs/todo.md)
- [CHANGELOG.md](CHANGELOG.md)

## Original Project

Rust rewrite of: https://github.com/buerokratt/Ruuter

## License

MIT License
