# Multi-stage build for minimal image size
FROM rust:1.75-slim as builder

WORKDIR /build

# Install build dependencies
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

# Copy manifests
COPY Cargo.toml Cargo.lock ./

# Create dummy main to cache dependencies
RUN mkdir src && \
    echo "fn main() {}" > src/main.rs && \
    echo "pub fn dummy() {}" > src/lib.rs

# Build dependencies (cached layer)
RUN cargo build --release && \
    rm -rf src

# Copy source code
COPY src ./src

# Build application
RUN cargo build --release

# Runtime stage
FROM debian:bookworm-slim

WORKDIR /app

# Install runtime dependencies
RUN apt-get update && apt-get install -y \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# Copy binary from builder
COPY --from=builder /build/target/release/ruuter-rs /app/ruuter-rs

# Copy DSL files and constants
COPY DSL ./DSL
COPY constants.ini ./constants.ini

# Expose port
EXPOSE 8080

# Run as non-root user
RUN useradd -m -u 1000 ruuter && \
    chown -R ruuter:ruuter /app

USER ruuter

CMD ["/app/ruuter-rs"]
