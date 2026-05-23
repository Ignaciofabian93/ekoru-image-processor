# `src/main.rs` — the entry point

## What this file is

The starting point of the program. Every Rust binary has a `main` function and Rust looks for it in `src/main.rs` by convention. This file:

1. Loads `.env`
2. Sets up logging
3. Loads config from env vars
4. Builds the shared state (S3 client + config)
5. Constructs the HTTP router with middleware
6. Binds a TCP listener and serves until killed

It is deliberately thin — all the real work is delegated to other modules. That's a common pattern in Rust apps: `main.rs` is the "wiring" file.

## The full file

```rust
use anyhow::Result;
use axum::{
    extract::DefaultBodyLimit,
    middleware,
    routing::{delete, get, post},
    Router,
};
use std::net::SocketAddr;
use tower_http::trace::TraceLayer;
use tracing_subscriber::EnvFilter;

mod auth;
mod config;
mod error;
mod handlers;
mod processor;
mod state;
mod storage;

use state::AppState;

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info,aws=warn,hyper=warn")),
        )
        .init();

    let cfg = config::Config::from_env()?;
    let body_limit = cfg.max_upload_bytes + 1024 * 1024;
    let state = AppState::new(cfg).await?;

    let protected = Router::new()
        .route("/process", post(handlers::upload::handle_upload))
        .route("/objects/{*key}", delete(handlers::delete::handle_delete))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            auth::require_internal_token,
        ));

    let app = Router::new()
        .route("/health", get(|| async { "ok" }))
        .merge(protected)
        .layer(DefaultBodyLimit::max(body_limit))
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    let addr = SocketAddr::from(([0, 0, 0, 0], 8090));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!("ekoru-image-processor listening on {}", addr);

    axum::serve(listener, app).await?;
    Ok(())
}
```

## Walkthrough

### `use` statements

```rust
use anyhow::Result;
use axum::{
    extract::DefaultBodyLimit,
    middleware,
    routing::{delete, get, post},
    Router,
};
use std::net::SocketAddr;
use tower_http::trace::TraceLayer;
use tracing_subscriber::EnvFilter;
```

`use` is the analogue of TypeScript's `import`. It brings names into scope so we can write `Router` instead of `axum::Router`. The `{ ... }` syntax imports multiple things from the same crate path in one statement.

Note `use anyhow::Result` — this _shadows_ the standard `Result<T, E>` so `Result<()>` in our `main` signature means `anyhow::Result<()>`, which is `Result<(), anyhow::Error>`.

### `mod` declarations

```rust
mod auth;
mod config;
mod error;
mod handlers;
mod processor;
mod state;
mod storage;
```

`mod foo;` tells the compiler to look for either `src/foo.rs` or `src/foo/mod.rs` and include its contents as a module named `foo`. **Every module the program uses must be declared somewhere reachable from `main.rs`**, even if you only `use` it indirectly. That's why you'll see `mod handlers;` here even though `main` only calls `handlers::upload::handle_upload` — without that `mod`, the compiler wouldn't know `src/handlers/` exists.

`pub mod handlers::upload;` inside `handlers/mod.rs` is what makes `handlers::upload::handle_upload` visible from here.

### `#[tokio::main]`

```rust
#[tokio::main]
async fn main() -> Result<()> {
```

This is a **proc-macro attribute** from the `tokio` crate. Plain Rust `main` is synchronous and can't be `async`. The macro rewrites our async `main` into something like:

```rust
fn main() -> Result<()> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(async { /* our body */ })
}
```

So we get to write `async fn main` and have a multi-threaded tokio runtime spun up for us. Returning `Result<()>` means we can use `?` for early-return on errors anywhere inside.

### Loading `.env`

```rust
dotenvy::dotenv().ok();
```

`dotenv()` reads the nearest `.env` file and copies its KEY=VALUE pairs into the process's environment (where `std::env::var(...)` will find them).

It returns `Result<PathBuf, dotenvy::Error>`. We call `.ok()` to **discard the result on purpose** — if no `.env` exists (e.g. when running in Docker where env vars come from `env_file:`), that's fine and we move on. `.ok()` converts `Result` to `Option` and we throw the `Option` away.

> Why not crash if `.env` is missing? Because in containers the env vars are injected by Docker, not by a `.env` file. We want the same code path in both cases.

### Setting up logging

```rust
tracing_subscriber::fmt()
    .with_env_filter(
        EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| EnvFilter::new("info,aws=warn,hyper=warn")),
    )
    .init();
```

`tracing_subscriber::fmt()` builds a subscriber that writes log records to stderr in a human-readable format. `.with_env_filter(...)` controls verbosity:

- `EnvFilter::try_from_default_env()` reads `RUST_LOG` from the environment.
- `.unwrap_or_else(|_| EnvFilter::new("info,..."))` — **if** the variable is missing or malformed, fall back to a sane default ("info" globally, "warn" for the AWS SDK and hyper which are very noisy at info).

`.init()` installs this subscriber as the global default. From this point on, every `tracing::info!`, `tracing::warn!`, etc. anywhere in our code (or in any dependency that uses `tracing`) is routed through it.

### Loading config and building shared state

```rust
let cfg = config::Config::from_env()?;
let body_limit = cfg.max_upload_bytes + 1024 * 1024;
let state = AppState::new(cfg).await?;
```

- `Config::from_env()` reads all required env vars (see [03-src-config.md](03-src-config.md)). If any are missing it returns `Err`, the `?` operator returns from `main` with that error, and the program exits with a non-zero status code and a clear message.
- `body_limit` is set to `max_upload_bytes + 1 MiB`. The extra megabyte is headroom for multipart-form overhead (boundaries, headers per field, the `entity` and `entity_id` text fields). It's a hard cap enforced by axum **before** any extractor runs.
- `AppState::new(cfg).await?` builds the S3 client once (see [04-src-state.md](04-src-state.md)). `await` because client construction may be async (it isn't currently, but the API is `async` for future-proofing).

### Building the router

```rust
let protected = Router::new()
    .route("/process", post(handlers::upload::handle_upload))
    .route("/objects/{*key}", delete(handlers::delete::handle_delete))
    .route_layer(middleware::from_fn_with_state(
        state.clone(),
        auth::require_internal_token,
    ));
```

The two **protected** routes go on a sub-router that has the auth middleware applied via `route_layer`:

- `post(handle_upload)` registers a POST handler.
- `delete(handle_delete)` registers a DELETE handler.
- `"/objects/{*key}"` is axum 0.8 path syntax. `{name}` is a single segment; `{*name}` is a "catch-all" that swallows any remaining path including `/`. So `/objects/product/123/abc.webp` matches and the `Path` extractor in the handler sees `"product/123/abc.webp"`.
- `route_layer(...)` applies a layer **only to routes already on this router**, not to anything merged in afterward. Critical for "auth on these, not those."
- `middleware::from_fn_with_state(state.clone(), auth::require_internal_token)` turns our plain async function into middleware and gives it access to the state.

```rust
let app = Router::new()
    .route("/health", get(|| async { "ok" }))
    .merge(protected)
    .layer(DefaultBodyLimit::max(body_limit))
    .layer(TraceLayer::new_for_http())
    .with_state(state);
```

Then a top-level router:

- `get(|| async { "ok" })` is an inline async closure. It takes no arguments and returns `&'static str`. axum knows how to turn `&str` into a `200 OK` response with the string as the body. No auth on this — health checks must work without secrets.
- `.merge(protected)` — combines two routers. The protected routes keep their middleware; merging doesn't strip it.
- `.layer(DefaultBodyLimit::max(body_limit))` — runs on **every** request (including `/health`). Limits the incoming body size globally. Doesn't matter for `/health` (no body), but applying it once is simpler than per-route.
- `.layer(TraceLayer::new_for_http())` — wraps every request in a `tracing` span and emits a structured log line on completion.
- `.with_state(state)` — registers the `AppState` value. Handlers that need it list `State<AppState>` in their signature and axum extracts it from here.

> ### Layer execution order
> Layers are applied in reverse of registration order at runtime. So the outermost layer (runs first on the way in, last on the way out) is the **last** one written. Here that's `TraceLayer`. Body-size enforcement happens inside the trace span, which is what we want.

### Binding and serving

```rust
let addr = SocketAddr::from(([0, 0, 0, 0], 8090));
let listener = tokio::net::TcpListener::bind(addr).await?;
tracing::info!("ekoru-image-processor listening on {}", addr);

axum::serve(listener, app).await?;
Ok(())
```

- `[0, 0, 0, 0]` is the IPv4 "all interfaces" address. Inside a container, this means "listen on any address that container has," which is what we want — the gateway connects to us by container DNS name.
- `TcpListener::bind` reserves the port. If the port is already in use, the `?` returns an error and the program exits.
- `axum::serve(listener, app)` is the modern axum API (post-0.7). It loops accepting connections and dispatches them through the router. It runs until the listener errors or the task is cancelled.

`Ok(())` returns success from `main`. The program exits with status 0 normally; if any of the `?`s short-circuited, it exits with 1 and `anyhow` prints the error chain to stderr.

## Why this shape

| Decision                                                    | Why                                                              |
| ----------------------------------------------------------- | ---------------------------------------------------------------- |
| Separate "protected" sub-router rather than gating per-route | Easier to audit which routes require auth; one place to change.   |
| Body limit applied globally rather than only on `/process`  | Defense in depth. A bug that adds a new POST route doesn't bypass it. |
| `dotenv().ok()` instead of `.unwrap()`                      | We use Docker `env_file:` in prod, where no `.env` file exists.   |
| Logging set up _before_ config load                         | If config load fails, we want the error to be logged.             |
| `AppState` constructed once, passed via axum state          | One S3 client → one connection pool → way faster than per-request. |
