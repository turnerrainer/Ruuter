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

RUN apt-get update && apt-get install -y \
    libssl3 ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /build/target/release/ruuter-rs /app/ruuter-rs
COPY DSL ./DSL
COPY constants.ini ./constants.ini

EXPOSE 8080
RUN useradd -m -u 1000 ruuter && chown -R ruuter:ruuter /app
USER ruuter

CMD ["/app/ruuter-rs"]
