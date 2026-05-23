# `Cargo.toml` — package manifest

## What this file is

The Rust equivalent of `package.json`. Cargo (Rust's `npm`) reads it to know:

- What the package is called and which version of Rust to compile against
- Which external crates ("crates" = Rust libraries) to fetch and link in
- Build profiles (`dev` vs `release`) and their settings

There's also a `Cargo.lock` (analogue of `package-lock.json`) which Cargo generates automatically and which pins the exact version of every transitively-resolved dep so builds are reproducible. You **commit** `Cargo.lock` for binaries (this service), and don't commit it for libraries.

## The full file

```toml
[package]
name = "ekoru-image-processor"
version = "0.1.0"
edition = "2021"

[dependencies]
axum = { version = "0.8", features = ["multipart", "macros"] }
tokio = { version = "1", features = ["full"] }
tower = "0.5"
tower-http = { version = "0.6", features = ["trace"] }
image = { version = "0.25", features = ["webp"] }
aws-sdk-s3 = "1"
aws-config = "1"
aws-credential-types = "1"
uuid = { version = "1", features = ["v4"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
dotenvy = "0.15"
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
bytes = "1"
anyhow = "1"
thiserror = "1"

[profile.release]
opt-level = 3
lto = "thin"
codegen-units = 1
strip = "symbols"
```

## `[package]` section

- **`name`** — the crate's name (and the produced binary's name). Hyphens are converted to underscores when used as a module path (`ekoru_image_processor`).
- **`version`** — semver. Bump it for releases.
- **`edition`** — the language edition (`2015`, `2018`, `2021`, `2024`). Editions are sets of language defaults that occasionally evolve; **all editions interoperate**, so picking a newer one doesn't break anything. We use `2021` for broad toolchain compatibility — `2024` would require Rust ≥ 1.85.

## `[dependencies]` — what each crate does

Each entry is `name = "version"` or `name = { version = "...", features = [...] }`. **Features** are opt-in pieces of a crate that you enable to keep compile time and binary size down.

### Web framework

#### `axum` — the HTTP framework

```toml
axum = { version = "0.8", features = ["multipart", "macros"] }
```

[`axum`](https://docs.rs/axum) is the equivalent of Express / NestJS. It gives you:

- `Router` — route registration
- `Handler` — async functions that respond to requests
- Extractors (`State`, `Path`, `Multipart`, `Json`, …) — parse parts of the request into typed values

Features:
- `multipart` — enables `axum::extract::Multipart` so we can parse `multipart/form-data` uploads.
- `macros` — gives `#[axum::debug_handler]`, which produces clearer error messages when a handler signature is wrong. Not strictly required.

#### `tokio` — the async runtime

```toml
tokio = { version = "1", features = ["full"] }
```

[`tokio`](https://docs.rs/tokio) is the executor that actually runs `async` code. axum requires one. The `full` feature enables all of tokio (multi-thread runtime, fs, net, sync primitives, time, macros). It's heavier than picking individual features but simpler to read and only matters for compile time; the linker will dead-code-eliminate what isn't used.

#### `tower` and `tower-http`

```toml
tower = "0.5"
tower-http = { version = "0.6", features = ["trace"] }
```

[`tower`](https://docs.rs/tower) is the **middleware abstraction** axum is built on. Anything you `.layer(...)` onto a router is a tower service.

[`tower-http`](https://docs.rs/tower-http) ships ready-made middleware. We only enable `trace`, which gives `TraceLayer` for per-request structured logs (method, path, status, latency).

### Image processing

#### `image`

```toml
image = { version = "0.25", features = ["webp"] }
```

Pure-Rust image decoder/encoder. Reads JPEG, PNG, GIF, BMP, TIFF, and WebP out of the box. The `webp` feature enables WebP encoding (decoding is on by default).

### Cloudflare R2 (via the AWS SDK)

R2 implements the **S3 protocol**, so we use Amazon's own Rust SDK and just point it at R2's endpoint instead of S3's.

```toml
aws-sdk-s3 = "1"
aws-config = "1"
aws-credential-types = "1"
```

- [`aws-sdk-s3`](https://docs.rs/aws-sdk-s3) — the actual S3 client (`put_object`, `delete_object`, …).
- [`aws-config`](https://docs.rs/aws-config) — gives `BehaviorVersion` and configuration helpers.
- [`aws-credential-types`](https://docs.rs/aws-credential-types) — provides the `Credentials` type for static access-key/secret pairs.

### Utilities

```toml
uuid = { version = "1", features = ["v4"] }
```

[`uuid`](https://docs.rs/uuid) — generates UUIDs. `v4` enables version-4 (random) UUIDs, used to give every uploaded object a unique key suffix.

```toml
serde = { version = "1", features = ["derive"] }
serde_json = "1"
```

[`serde`](https://docs.rs/serde) — the canonical serialization framework. `derive` lets us write `#[derive(Serialize, Deserialize)]` on a struct and have JSON-mapping code generated for us. [`serde_json`](https://docs.rs/serde_json) is the JSON-specific adapter.

```toml
dotenvy = "0.15"
```

[`dotenvy`](https://docs.rs/dotenvy) — loads `.env` files into `std::env` at startup. The actively-maintained successor to the unmaintained `dotenv` crate.

```toml
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
```

[`tracing`](https://docs.rs/tracing) — structured logging API (think Pino/Winston with spans). [`tracing-subscriber`](https://docs.rs/tracing-subscriber) configures where logs go and how they're filtered. The `env-filter` feature lets us control verbosity via `RUST_LOG=info,aws=warn`.

```toml
bytes = "1"
```

[`bytes`](https://docs.rs/bytes) — efficient byte-buffer types (`Bytes`, `BytesMut`). Multipart fields return `Bytes`. We don't reach for it directly much but it's a transitive necessity for several APIs.

### Error handling

```toml
anyhow = "1"
thiserror = "1"
```

Two complementary crates:

- [`anyhow`](https://docs.rs/anyhow) — `anyhow::Error` is "any error, doesn't matter what kind." Great for the top of the call stack (e.g. `main()`) where you just want to bubble things up and not enumerate every variant.
- [`thiserror`](https://docs.rs/thiserror) — derive-macro for building your **own** error enums with proper `Display` / `Error` impls. We use it for `AppError`, which then gets mapped to HTTP statuses.

The convention: `thiserror` for **library / domain** errors (where callers need to switch on the variant), `anyhow` for **application** errors (where you just want to fail loudly).

## `[profile.release]` — production build settings

```toml
[profile.release]
opt-level = 3
lto = "thin"
codegen-units = 1
strip = "symbols"
```

| Setting             | Meaning                                                                              |
| ------------------- | ------------------------------------------------------------------------------------ |
| `opt-level = 3`     | Maximum optimization (default for `release`).                                        |
| `lto = "thin"`      | Link-Time Optimization across crate boundaries — smaller, faster binary, slower build. |
| `codegen-units = 1` | Compile the whole crate as one unit so the optimizer sees everything. Slower build, faster binary. |
| `strip = "symbols"` | Remove debug symbols from the final binary. Big size reduction (~50%).               |

These settings only affect `cargo build --release` (and what the Dockerfile produces). Local `cargo run` uses the `dev` profile which is unoptimized and fast to compile.

## Common changes

| You want to…                                  | What to do                                                                 |
| --------------------------------------------- | -------------------------------------------------------------------------- |
| Add a new crate                               | Run `cargo add <name>` (or edit `Cargo.toml` then `cargo build`).         |
| Enable a feature on an existing crate         | Edit the `features = [...]` list, then `cargo build` to rebuild.           |
| Pin a specific version                        | Replace `"1"` with `"=1.4.2"`. (Cargo treats `"1"` as `>=1.0, <2.0`.)      |
| Update all deps to latest matching versions   | `cargo update`. To bump majors, edit the version requirements first.       |
| Remove a crate                                | `cargo remove <name>` (or delete its line and `cargo build`).              |
