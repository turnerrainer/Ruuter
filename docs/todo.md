# Ruuter-RS Development TODO

## Version: 0.1.0-rust-foundation

### Project Goals
Complete Rust rewrite of Java-based Ruuter maintaining 100% DSL compatibility while improving performance and resource efficiency.

## Current Version Features
- [x] Project structure initialized
- [x] Git repository setup with dev branch
- [x] Initial dependencies configured
- [ ] Core implementation pending

## Roadmap

### Phase 1: Foundation (v0.1.x)
- [ ] Core DSL parser with serde_yaml
- [ ] Basic HTTP server with Axum
- [ ] File-system-based routing
- [ ] Simple return step implementation
- [ ] Basic configuration loading

### Phase 2: Steps Implementation (v0.2.x)
- [ ] Assign step (variable assignment)
- [ ] HTTP steps (GET, POST, PUT, DELETE)
- [ ] Switch step (conditionals)
- [ ] Return step (full implementation)
- [ ] Log step

### Phase 3: JavaScript Engine (v0.3.x)
- [ ] Boa engine integration
- [ ] Variable substitution ${...}
- [ ] JavaScript expression evaluation
- [ ] Context bindings (incoming, step results)
- [ ] Optional chaining support

### Phase 4: Advanced Features (v0.4.x)
- [ ] Template step (recursive DSL calls)
- [ ] Guards system
- [ ] Declaration step + OpenAPI generation
- [ ] Constants.ini support
- [ ] Multi-project namespaces

### Phase 5: Production Features (v0.5.x)
- [ ] Hot reload with file watching
- [ ] OpenTelemetry integration
- [ ] OpenSearch logging
- [ ] Error handling (local + global DSLs)
- [ ] CORS configuration
- [ ] Security features (allowlists, filtering)

### Phase 6: Performance & Optimization (v0.6.x)
- [ ] Connection pooling
- [ ] DSL caching
- [ ] Async parallel execution
- [ ] Memory optimization
- [ ] Benchmark suite

### Phase 7: Testing & Validation (v0.7.x)
- [ ] Unit tests for all steps
- [ ] Integration tests
- [ ] DSL compatibility tests
- [ ] Performance benchmarks vs Java
- [ ] Load testing

### Phase 8: Production Ready (v0.8.x)
- [ ] Docker support
- [ ] Documentation complete
- [ ] Migration guide
- [ ] Example DSLs
- [ ] Production deployment guide

## Proposed Improvements Over Java Version

### Performance
- Faster startup time (target: <2s vs Java's 5-10s)
- Lower memory footprint (target: <50MB vs Java's 200MB+)
- Higher throughput (target: >10k req/sec)
- Better resource utilization

### Developer Experience
- Better error messages with context
- Type-safe configuration
- Improved debugging support
- Hot reload without JVM overhead

### Features to Consider
- [ ] GraphQL endpoint generation from DSLs
- [ ] WebSocket support via DSL
- [ ] Built-in rate limiting
- [ ] Request/response transformation pipelines
- [ ] Plugin system for custom steps
- [ ] DSL validation CLI tool
- [ ] Interactive DSL debugger
- [ ] Metrics dashboard
- [ ] Health check endpoints with detailed status
- [ ] A/B testing support via DSL

### Architecture Improvements
- [ ] Trait-based step system for extensibility
- [ ] Zero-copy optimizations where possible
- [ ] Lazy evaluation of JavaScript expressions
- [ ] Compile-time DSL validation option
- [ ] Step execution tracing for debugging

## Known Limitations from Java Version to Address
- Sequential execution only (add parallel support)
- No caching for HTTP calls (add with TTL)
- Limited error context (improve with Rust's error handling)
- No request batching (consider adding)

## Breaking Changes to Consider
None planned - maintaining full DSL compatibility is priority.

## Notes
- Follow existing DSL structure strictly
- All improvements should be additive, not breaking
- Performance gains should not sacrifice correctness
- Document all deviations from Java implementation
