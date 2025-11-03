# Ruuter-RS

**Rust implementation of Ruuter - Declarative REST Router**

Version: 0.2.0-functional-core
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
- ⚠️ Template step (basic)
- ⚠️ Guards system (placeholder)

## Quick Start

```bash
cargo build
cargo run
```

Server starts on `http://localhost:8080`

## Example DSL

```yaml
# DSL/samples/GET/ping.yml
response:
  status: 202
  return: pong
```

Access: `GET http://localhost:8080/samples/ping`

## Documentation

- [Development TODO](docs/todo.md)
- [CHANGELOG.md](CHANGELOG.md)

## Original Project

Rust rewrite of: https://github.com/buerokratt/Ruuter

## License

MIT License
