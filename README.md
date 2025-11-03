# Ruuter-RS

**Rust implementation of Ruuter - Declarative REST Router**

Version: 0.1.0-rust-foundation
Author: Rainer Türner
Status: Early Development

## About

Ruuter-RS is a complete Rust rewrite of the Java-based Ruuter declarative REST routing engine. It maintains 100% compatibility with existing Ruuter DSL files while providing significant performance improvements and lower resource usage.

### Key Features (Planned)
- File-system-based REST routing
- YAML DSL for endpoint definition
- JavaScript expression evaluation
- HTTP client with full REST support
- Guards for authentication/authorization
- Template system for DSL composition
- Hot reload support
- OpenTelemetry tracing
- OpenSearch logging integration

## Project Status

This is the initial foundation release. Core features are under active development.

See [docs/todo.md](docs/todo.md) for detailed roadmap.

## Building

Requires Rust 1.75+

```bash
cargo build
cargo run
cargo test
```

## Documentation

- [Development TODO](docs/todo.md) - Development roadmap and proposed improvements
- [CHANGELOG.md](CHANGELOG.md) - Version history

## Original Project

This is a Rust implementation of Ruuter, originally developed at the Information System Authority of Estonia (RIA).

Original Java repository: https://github.com/buerokratt/Ruuter
Reference implementation: /home/rainer/Desktop/Buerostack/Ruuter

## License

MIT License - See LICENSE file
