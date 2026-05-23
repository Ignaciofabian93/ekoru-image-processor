# `src/state.rs` — shared application state

## What this file is

Defines the **state container** that gets injected into every HTTP handler that asks for it. It holds two things:

1. The S3 client used to talk to Cloudflare R2 — built **once** at startup.
2. The `Config` we loaded from env vars.

Both are wrapped in `Arc<T>` so they can be cheaply cloned per request.

## The full file

```rust
use std::sync::Arc;

use anyhow::Result;
use aws_config::BehaviorVersion;
use aws_sdk_s3::{
    config::{Credentials, Region},
    Client as S3Client,
};

use crate::config::Config;

#[derive(Clone)]
pub struct AppState {
    pub s3: Arc<S3Client>,
    pub cfg: Arc<Config>,
}

impl AppState {
    pub async fn new(cfg: Config) -> Result<Self> {
        let creds = Credentials::new(
            cfg.r2_access_key_id.clone(),
            cfg.r2_secret_access_key.clone(),
            None,
            None,
            "r2",
        );

        let s3_cfg = aws_sdk_s3::Config::builder()
            .behavior_version(BehaviorVersion::latest())
            .region(Region::new("auto"))
            .endpoint_url(format!(
                "https://{}.r2.cloudflarestorage.com",
                cfg.r2_account_id
            ))
            .credentials_provider(creds)
            .build();

        Ok(Self {
            s3: Arc::new(S3Client::from_conf(s3_cfg)),
            cfg: Arc::new(cfg),
        })
    }
}
```

## Why `Arc<T>` for everything

axum's state must be `Clone` because it's cloned **once per request** so the handler can take ownership of its copy without taking it away from anyone else.

- Cloning a 5 KB `Config` by value would copy 5 KB on every single request. Not the end of the world, but wasteful.
- Cloning an `S3Client` by value would clone the **entire connection pool** — at best, a few microseconds; at worst, race conditions in the pool.
- Cloning an `Arc<T>` is _always_ "bump an atomic integer." Constant time, ~5 ns. The underlying value is shared, not copied.

So `AppState` is `#[derive(Clone)]`, and the heavy stuff inside is `Arc<...>`. Cloning `AppState` clones two `Arc`s. Done.

> **Mental model**: `Arc<T>` is a smart pointer with a reference count. Each `.clone()` increments the count, each drop decrements it. When the count hits zero, the value is freed. Multiple threads can hold clones simultaneously and the atomic operations make that safe.

## Walkthrough

### Imports

```rust
use std::sync::Arc;

use anyhow::Result;
use aws_config::BehaviorVersion;
use aws_sdk_s3::{
    config::{Credentials, Region},
    Client as S3Client,
};

use crate::config::Config;
```

- `std::sync::Arc` — the standard library's reference-counted smart pointer.
- `aws_config::BehaviorVersion` — a versioning knob for the AWS SDK's default behaviors. Required since aws-sdk 1.x.
- `aws_sdk_s3::Client` — the actual S3 client, re-aliased to `S3Client` so it doesn't collide with the name `Client` used elsewhere.
- `Credentials`, `Region` — value types used when building the config.
- `crate::config::Config` — our own config struct. `crate::` always refers to the current crate's root (i.e. `src/lib.rs` or `src/main.rs`).

### Struct + `Clone`

```rust
#[derive(Clone)]
pub struct AppState {
    pub s3: Arc<S3Client>,
    pub cfg: Arc<Config>,
}
```

Two `Arc<>` fields. `#[derive(Clone)]` only works because **every field** implements `Clone`, and `Arc<T>` always does (regardless of whether `T` does).

### The constructor

```rust
impl AppState {
    pub async fn new(cfg: Config) -> Result<Self> { ... }
}
```

Takes ownership of `cfg` (no `&` in front, so it's moved in). Returns `Result<AppState>` so it can fail (though currently it doesn't — it's `async` and returns `Result` purely so we can add real async setup later without changing callers).

#### Building credentials

```rust
let creds = Credentials::new(
    cfg.r2_access_key_id.clone(),
    cfg.r2_secret_access_key.clone(),
    None,
    None,
    "r2",
);
```

The `Credentials::new` signature is `new(access_key_id, secret_access_key, session_token, expires_after, provider_name)`.

- `cfg.r2_access_key_id.clone()` — we `.clone()` because we still need `cfg` to be intact after this for use below. Without the clone we'd "move" the field out of `cfg`, breaking subsequent access. (This is a Rust ownership rule: you can move out of a `String` field, but then the field is gone.)
- `None, None` — no session token (R2 doesn't use them); no expiry (these are static creds).
- `"r2"` — a label used in debug logs to identify which credential provider was used. Purely informational.

#### Building the S3 config

```rust
let s3_cfg = aws_sdk_s3::Config::builder()
    .behavior_version(BehaviorVersion::latest())
    .region(Region::new("auto"))
    .endpoint_url(format!(
        "https://{}.r2.cloudflarestorage.com",
        cfg.r2_account_id
    ))
    .credentials_provider(creds)
    .build();
```

This is a **builder pattern**: start with `::builder()`, chain `.setter(...)` calls, finish with `.build()`. Common in the AWS SDK because S3 has dozens of config knobs and a single mega-constructor would be unwieldy.

- `.behavior_version(BehaviorVersion::latest())` — opts into the latest SDK defaults. Without it, the SDK prints a warning at startup. Pinning to `latest()` is fine for an internal service; in a library you'd pin a specific version (e.g. `v2024_03_28()`) so SDK upgrades don't change behavior under you.
- `.region(Region::new("auto"))` — R2 is global, not regional. The string `"auto"` tells the SDK "I don't care which region, just dispatch." Cloudflare accepts this.
- `.endpoint_url(...)` — **this** is what redirects the SDK away from `s3.amazonaws.com` and toward R2. Format: `https://<account-id>.r2.cloudflarestorage.com`. `format!` is the string interpolation macro (no `println!`-style printing — it returns a `String`).
- `.credentials_provider(creds)` — installs the static credentials we built above.
- `.build()` — consumes the builder and returns an `aws_sdk_s3::Config`.

#### Constructing the client and the state

```rust
Ok(Self {
    s3: Arc::new(S3Client::from_conf(s3_cfg)),
    cfg: Arc::new(cfg),
})
```

`S3Client::from_conf(s3_cfg)` creates the actual client (constructs the connection pool, sets up retry policy, etc.). We wrap it in `Arc::new(...)` and assign.

`Arc::new(cfg)` moves our `cfg` (the one we received as a parameter, which is now intact since we only `.clone()`d field values out of it) into an `Arc`.

`Ok(Self { ... })` returns success. `Self` here = `AppState`.

## Why this shape

| Decision                                                  | Why                                                                                       |
| --------------------------------------------------------- | ----------------------------------------------------------------------------------------- |
| One shared S3 client, not one per request                  | The client owns a connection pool. Building one per request would defeat the pool and slow every call. |
| `Arc<S3Client>` rather than passing `&S3Client`            | Handlers run on different tokio tasks. References across tasks are usually not workable; `Arc` is the standard way to share. |
| `Arc<Config>` not just `Config`                            | Cheaper to clone (one pointer instead of N strings).                                       |
| `async fn new` even though no awaits                       | Future-proofing — if we later add e.g. a warm-up call (`head_bucket`) to verify creds at startup, no caller change is needed. |
| `region("auto")`                                           | R2 documentation requirement.                                                              |
| `behavior_version(latest())` instead of pinned             | Silences the SDK startup warning; we don't ship a library so latest is fine.               |

## How other code uses `AppState`

In `main.rs`:

```rust
let state = AppState::new(cfg).await?;
// ...
.with_state(state)
```

In a handler (e.g. `handlers/upload.rs`):

```rust
pub async fn handle_upload(
    State(state): State<AppState>,  // axum extracts a clone of the state
    mut multipart: Multipart,
) -> Result<Json<UploadResponse>, AppError> {
    // state.s3 — the shared S3 client
    // state.cfg.r2_bucket — etc.
}
```

The `State<AppState>` extractor in the handler signature is how axum knows to clone the state and hand it over. The `State(state)` part destructures the wrapper so you can use `state` directly.
