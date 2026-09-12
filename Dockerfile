FROM rust:1.88-slim AS builder
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

# `apt-get upgrade` pulls in bookworm-security fixes for base-image
# packages (libpcre2-8-0, glibc, etc.) that Trivy will otherwise
# flag on the published image. Without this the runtime image ships
# whatever the base-image bookworm-slim tag was baked with, which
# lags Debian security updates by weeks. Trivy in publish.yml is
# `severity: HIGH,CRITICAL, ignore-unfixed: true, exit-code: 1` —
# any HIGH/CRITICAL with a fix available in the archive fails
# publish. Refresh here so `libssl3 ca-certificates …` land at
# their patched versions.
RUN apt-get update \
    && DEBIAN_FRONTEND=noninteractive apt-get -y upgrade \
    && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
       libssl3 ca-certificates curl tini \
    && apt-get -y autoremove \
    && apt-get -y clean \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /build/target/release/ruuter-on-rust /app/ruuter-on-rust
# Issue #83 — ship the CI tools alongside the runtime binary. Same
# `cargo build --release` produces all three, so this adds ~15 MB and
# zero build time. Symlinks under /usr/local/bin/ so downstream CI
# can just `docker run … turnerrainer/ruuter:<tag> dsl-lint --dsl …`
# without a full path. Version-alignment guarantee: tools built from
# exactly the engine version they'll be linting against.
COPY --from=builder /build/target/release/dsl-lint /app/dsl-lint
COPY --from=builder /build/target/release/dsl-test /app/dsl-test
RUN ln -s /app/dsl-lint /usr/local/bin/dsl-lint \
 && ln -s /app/dsl-test /usr/local/bin/dsl-test
COPY DSL ./DSL
COPY constants.ini ./constants.ini

EXPOSE 8080
RUN useradd -m -u 1000 ruuter && chown -R ruuter:ruuter /app
USER ruuter

ENTRYPOINT ["/usr/bin/tini", "--"]
CMD ["/app/ruuter-on-rust"]
