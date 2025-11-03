# Changelog

All notable changes to Ruuter-RS will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.3.0-docker-support] - 2025-11-03

### Added
- Docker support with multi-stage builds
- docker-compose.yml for easy deployment
- .dockerignore for optimized builds
- Health check endpoint in Docker
- Volume mounts for DSL and constants
- Non-root user in Docker image
- Docker documentation in README

### Changed
- Updated README with Docker instructions
- Improved quick start guide

## [0.2.0-functional-core] - 2025-11-03

### Added
- Complete DSL parser with YAML support
- File-based routing system
- JavaScript engine integration (Boa)
- All core step types (assign, return, http, switch, log, template)
- HTTP client with timeout support
- Execution context with variable storage
- Constants.ini support
- Configuration system
- Error handling framework
- Guards system (basic structure)
- Axum web server with health check
- Tracing/logging infrastructure

## [0.1.0-rust-foundation] - 2025-11-03

### Added
- Initial project structure
- Dependency configuration
- Documentation
- Git workflow
