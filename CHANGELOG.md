# Changelog

All notable changes to Ruuter-RS will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.3.1] - 2025-11-03

### Added
- Comprehensive DSL sample library covering all features:
  - Basic samples (hello, status codes, headers)
  - Variable assignment samples
  - HTTP step samples (GET, POST, chaining)
  - Conditional/switch samples
  - JavaScript evaluation samples (math, strings, dates, arrays)
  - Advanced patterns (step chaining, multi-step processing, pagination)
  - Logging demonstrations
- DSL/samples/README.md with complete documentation
- 20+ working example DSL files
- Quick reference guide for DSL syntax
- Usage examples with curl commands

### Documentation
- Added comprehensive samples guide
- Included syntax quick reference
- Added tips and best practices

## [0.3.0-docker-support] - 2025-11-03

### Added
- Docker support with multi-stage builds
- docker-compose.yml for easy deployment
- .dockerignore for optimized builds
- Health check endpoint in Docker
- Volume mounts for DSL and constants
- Non-root user in Docker image

## [0.2.0-functional-core] - 2025-11-03

### Added
- Complete DSL parser with YAML support
- File-based routing system
- JavaScript engine integration (Boa)
- All core step types
- HTTP client
- Execution context
- Configuration system

## [0.1.0-rust-foundation] - 2025-11-03

### Added
- Initial project structure
- Dependency configuration
- Documentation
- Git workflow
