# syntax=docker/dockerfile:1.7

# ---------- Builder ----------
FROM rust:1.85-slim-bookworm AS builder
WORKDIR /app

RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config \
    libssl-dev \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# Cache the dependency layer separately from the source so source-only changes
# don't force a full rebuild of the crate graph.
COPY Cargo.toml Cargo.lock* ./
RUN mkdir -p src && echo "fn main() {}" > src/main.rs && \
    cargo build --release && \
    rm -rf src target/release/deps/ekoru_image_processor* target/release/ekoru-image-processor*

COPY src ./src
RUN cargo build --release

# ---------- Runtime ----------
FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends \
    libssl3 \
    ca-certificates \
    wget \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --create-home --uid 1001 app

COPY --from=builder /app/target/release/ekoru-image-processor /usr/local/bin/ekoru-image-processor

USER app
EXPOSE 8090
CMD ["ekoru-image-processor"]
