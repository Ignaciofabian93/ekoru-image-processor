# `Dockerfile` — multi-stage build

## What this file is

Recipe Docker uses to produce a runnable image of the service. Multi-stage so the **builder** (heavy: full Rust toolchain, dev libs) and the **runtime** (slim: just the OS libs we need and our binary) are separate. The final image is a few tens of MB instead of ~1.5 GB.

## The full file

```dockerfile
# syntax=docker/dockerfile:1.7

# ---------- Builder ----------
FROM rust:1.82-slim-bookworm AS builder
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
```

## Walkthrough

### The directive line

```dockerfile
# syntax=docker/dockerfile:1.7
```

Opts into Docker's "BuildKit" frontend version 1.7. Required for some newer instructions and for safer parsing. Always go at the very top of the file.

### Stage 1: builder

```dockerfile
FROM rust:1.82-slim-bookworm AS builder
WORKDIR /app
```

- `rust:1.82-slim-bookworm` — the official Rust toolchain image, "slim" variant (fewer packages baked in), based on Debian Bookworm. Pinning the major+minor version (`1.82`) makes builds reproducible — when Rust 1.83 ships, our build doesn't silently change.
- `AS builder` names this stage so the runtime stage can copy from it.
- `WORKDIR /app` — like `cd /app`. Also creates the directory.

```dockerfile
RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config \
    libssl-dev \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*
```

OS-level build dependencies:

- `pkg-config` — used by some Rust crates' build scripts (e.g. `openssl-sys`) to find C libraries.
- `libssl-dev` — OpenSSL headers, in case any dependency wants to link OpenSSL at build time.
- `ca-certificates` — for any build-time HTTPS calls (e.g. downloading crates from crates.io).

`--no-install-recommends` skips optional "recommended" deps to keep things minimal. The `rm -rf /var/lib/apt/lists/*` at the end removes the apt index cache — these files are big and we don't need them at build time after install. All combined into one `RUN` so the cleanup ends up in the same Docker layer (otherwise the cleanup wouldn't actually shrink the image).

### Dependency-layer caching

```dockerfile
COPY Cargo.toml Cargo.lock* ./
RUN mkdir -p src && echo "fn main() {}" > src/main.rs && \
    cargo build --release && \
    rm -rf src target/release/deps/ekoru_image_processor* target/release/ekoru-image-processor*
```

This is the trick that makes incremental builds fast.

The naive approach would be:

```dockerfile
COPY . .
RUN cargo build --release
```

That works, but **any** change to **any** source file invalidates the Docker layer cache for this `RUN`, so we re-fetch and re-compile all 200+ deps every single build (slow — minutes).

What this version does instead:

1. Copy only `Cargo.toml` and `Cargo.lock` (the `*` after `Cargo.lock` makes it optional — first-time builds with no lock file still work).
2. Create a **dummy** `src/main.rs` that's just `fn main() {}`.
3. Build the project — Cargo fetches and compiles all dependencies, but only the trivial empty `main` for our own crate.
4. Delete the dummy source and the leftover artifacts that depend on it. The compiled dependencies stay cached in `target/release/deps/`.

Now this layer is cached as long as `Cargo.toml` / `Cargo.lock` don't change. When you change a `.rs` file, Docker skips back to here from cache and only the next step runs.

```dockerfile
COPY src ./src
RUN cargo build --release
```

5. Copy the actual source.
6. Build for real. Cargo notices all deps are already compiled and only compiles _our_ crate. Subsequent rebuilds (no dep changes) take seconds rather than minutes.

> **Will this break if I add a dep?** Adding/removing/version-bumping anything in `Cargo.toml` invalidates the cache and triggers a clean dep rebuild. That's correct — the new dep tree needs new artifacts.

### Stage 2: runtime

```dockerfile
FROM debian:bookworm-slim
```

Fresh base image — `debian:bookworm-slim` is ~80 MB. We don't carry over the Rust toolchain, dev libs, or anything else from the builder.

```dockerfile
RUN apt-get update && apt-get install -y --no-install-recommends \
    libssl3 \
    ca-certificates \
    wget \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --create-home --uid 1001 app
```

Runtime OS dependencies:

- `libssl3` — the OpenSSL **runtime** library. (We had `libssl-dev` in the builder for headers; here we just need the shared library at runtime.) Required because our binary is dynamically linked against OpenSSL (via the AWS SDK and its HTTP stack).
- `ca-certificates` — trust roots for TLS. Without this, every HTTPS call to R2 would fail with a cert verification error.
- `wget` — used by the Docker `healthcheck` in the compose files. ~200 KB and we'd rather have a healthcheck than save the space.

`useradd --create-home --uid 1001 app` creates a non-root user. We'll run as them below.

```dockerfile
COPY --from=builder /app/target/release/ekoru-image-processor /usr/local/bin/ekoru-image-processor
```

Copies **only** the compiled binary from the builder stage into the runtime image. The Rust toolchain, build artifacts, source code, and everything else are left behind.

```dockerfile
USER app
EXPOSE 8090
CMD ["ekoru-image-processor"]
```

- `USER app` — drop from root to the unprivileged user we created. Anything CVE-related that escapes the binary can't trivially root the container. Required posture for production-grade containers.
- `EXPOSE 8090` — purely documentation. Doesn't actually open a port; it just signals "this container listens on 8090." Compose / Kubernetes / `docker run -P` read it to know what to publish.
- `CMD ["ekoru-image-processor"]` — the command run when the container starts. The "exec form" (JSON array) is preferred over the "shell form" (`CMD ekoru-image-processor`) because it runs the binary directly (PID 1) rather than via `/bin/sh -c`, which means signals (SIGTERM on `docker stop`) reach the process correctly.

## Why this shape

| Decision                                                            | Why                                                                                          |
| ------------------------------------------------------------------- | -------------------------------------------------------------------------------------------- |
| Multi-stage build                                                    | Final image ~80 MB instead of ~1.5 GB. Smaller pulls, faster deploys, smaller attack surface. |
| Pin Rust version (`1.82` not `latest`)                                | Reproducible builds; CI behavior doesn't change on Tuesday for free.                          |
| Dummy-main dependency caching trick                                   | 10x speed-up on the typical "I edited one file" rebuild.                                       |
| `libssl-dev` in builder, `libssl3` in runtime                         | Builder needs headers; runtime only needs the shared lib.                                     |
| Drop privileges to a non-root user                                    | Hardening — defense in depth against container escapes.                                       |
| Include `wget` for healthcheck                                         | Small price; lets the compose `healthcheck:` actually work without spawning a shell.          |
| `CMD` in exec form (JSON array)                                       | Signals reach the binary directly, so `docker stop` is graceful.                              |

## Common changes

| You want to…                                                        | What to do                                                                                  |
| ------------------------------------------------------------------- | ------------------------------------------------------------------------------------------- |
| Use a newer Rust                                                     | Bump `rust:1.82-slim-bookworm` to e.g. `rust:1.85-slim-bookworm`.                            |
| Switch to musl for a static binary                                   | Replace the runtime base with `gcr.io/distroless/static-debian12` and build with `--target x86_64-unknown-linux-musl`. |
| Skip the `wget` install                                              | Replace the compose healthcheck with `["CMD", "/usr/local/bin/ekoru-image-processor", "--health"]` and implement a `--health` flag (more code, smaller image). |
| Use cargo-chef for caching                                           | Slightly fancier; install `cargo-chef`, prepare a recipe, cook deps, build. Often overkill for small projects. |
| Build for ARM (Apple Silicon, Graviton)                              | Run `docker buildx build --platform linux/arm64,linux/amd64 ...`. The base images are multi-arch. |
| Inspect the final image size                                         | `docker images ekoru-image-processor:dev`.                                                   |
