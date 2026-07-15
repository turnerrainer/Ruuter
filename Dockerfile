FROM rust:1.88-slim as builder
WORKDIR /build

RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release

FROM debian:bookworm-slim
WORKDIR /app

RUN apt-get update && apt-get install -y --no-install-recommends \
    libssl3 ca-certificates curl tini \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /build/target/release/ruuter-on-rust /app/ruuter-on-rust
COPY DSL ./DSL
COPY constants.ini ./constants.ini

EXPOSE 8080
RUN useradd -m -u 1000 ruuter && chown -R ruuter:ruuter /app
USER ruuter

ENTRYPOINT ["/usr/bin/tini", "--"]
CMD ["/app/ruuter-on-rust"]
