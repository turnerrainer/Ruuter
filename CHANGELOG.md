# Changelog

All notable changes to Ruuter-RS will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.0-functional-core] - 2025-11-03

### Added
- Complete DSL parser with YAML support
- File-based routing system
- JavaScript engine integration (Boa)
- All core step types:
  - Assign (variable assignment)
  - Return (response generation)
  - HTTP (GET/POST/PUT/DELETE)
  - Switch (conditional logic)
  - Log (logging)
  - Template (placeholder)
- HTTP client with timeout support
- Execution context with variable storage
- Constants.ini support
- Configuration system
- Error handling framework
- Guards system (basic structure)
- Axum web server with health check
- Tracing/logging infrastructure

### Notes
- Functional core implementation complete
- Template step needs recursive DSL execution
- Guards not fully implemented
- Ready for basic DSL execution

## [0.1.0-rust-foundation] - 2025-11-03

### Added
- Initial project structure
- Dependency configuration
- Documentation
- Git workflow
