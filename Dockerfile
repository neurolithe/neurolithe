# Multi-stage build for the NeuroLithe memory service (the JARVIS "brain").
# Runs on the always-on Mac mini next to the Kafka broker (Docker Compose).
# Stage 1 compiles a release binary (vendored librdkafka, bundled SQLite, and
# the sqlite-vec C extension, all built from source); stage 2 is a slim runtime
# with just the binary. Mirrors Chronos/Ledger.

# ── build ────────────────────────────────────────────────────────────────────
FROM rust:1.94-slim-bookworm AS builder
WORKDIR /app
# Build deps: cmake/g++/make build the vendored librdkafka (rdkafka-sys), the
# bundled SQLite amalgamation + sqlite-vec (rusqlite/sqlite-vec), and the rustls
# crypto backend (aws-lc-rs) used by reqwest. zlib is librdkafka's only default
# dependency; pkg-config wires it up.
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        cmake g++ make pkg-config zlib1g-dev \
    && rm -rf /var/lib/apt/lists/*
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release --bin neurolithe && strip target/release/neurolithe

# ── runtime ──────────────────────────────────────────────────────────────────
FROM debian:bookworm-slim AS runtime
# zlib1g: librdkafka runtime. ca-certificates: reqwest hits Pithos over plain
# HTTP on the LAN, but a *cloud* LLM/embedding provider (configurable) is HTTPS,
# so the trust store must be present. SQLite + sqlite-vec are statically linked.
RUN apt-get update \
    && apt-get install -y --no-install-recommends zlib1g ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd -r -u 10001 -s /usr/sbin/nologin neurolithe \
    && mkdir -p /data && chown neurolithe:neurolithe /data
COPY --from=builder /app/target/release/neurolithe /usr/local/bin/neurolithe
USER neurolithe
# Both SQLite stores live on a persistent named volume mounted at /data (see
# docker-compose.yml). All other config (Kafka, Pithos, LLM) comes from the env.
ENV NEUROLITHE__STM__PATH=/data/neurolithe-stm.sqlite \
    NEUROLITHE__LTM__PATH=/data/neurolithe-ltm.sqlite
# Long-running daemon: MCP (stdio) + feeder + command consumer + schedulers.
ENTRYPOINT ["/usr/local/bin/neurolithe"]
