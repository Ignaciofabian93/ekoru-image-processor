# `src/config.rs` — environment loading and validation

## What this file is

One job: read all the environment variables the service needs, validate them, and return a struct holding them. Called exactly once, at the very start of `main`.

If anything is wrong, the program fails to start with a clear message saying what's missing. We **never** want to discover a missing env var halfway through serving a request; failing at startup is much friendlier.

## The full file

```rust
use anyhow::{Context, Result};

#[derive(Clone, Debug)]
pub struct Config {
    pub r2_account_id: String,
    pub r2_access_key_id: String,
    pub r2_secret_access_key: String,
    pub r2_bucket: String,
    pub r2_public_base_url: String,
    pub internal_token: String,
    pub max_upload_bytes: usize,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let max_upload_bytes = std::env::var("MAX_UPLOAD_BYTES")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(10 * 1024 * 1024);

        let cfg = Self {
            r2_account_id: required("R2_ACCOUNT_ID")?,
            r2_access_key_id: required("R2_ACCESS_KEY_ID")?,
            r2_secret_access_key: required("R2_SECRET_ACCESS_KEY")?,
            r2_bucket: required("R2_BUCKET")?,
            r2_public_base_url: required("R2_PUBLIC_BASE_URL")?
                .trim_end_matches('/')
                .to_string(),
            internal_token: required("INTERNAL_TOKEN")?,
            max_upload_bytes,
        };

        if cfg.internal_token.len() < 16 {
            anyhow::bail!("INTERNAL_TOKEN must be at least 16 characters long");
        }

        Ok(cfg)
    }
}

fn required(key: &str) -> Result<String> {
    std::env::var(key).with_context(|| format!("missing required env var: {key}"))
}
```

## Walkthrough

### Imports

```rust
use anyhow::{Context, Result};
```

- `Result` — `anyhow::Result<T>` is an alias for `Result<T, anyhow::Error>`. Shorter to type.
- `Context` — a trait that adds a `.context(...)` and `.with_context(|| ...)` method to any `Result<T, E>`. Lets us attach a human description to an underlying error.

### The struct

```rust
#[derive(Clone, Debug)]
pub struct Config {
    pub r2_account_id: String,
    ...
}
```

A plain struct, fields all public. We add two derived trait impls:

- **`Clone`** — lets us call `.clone()` on a `Config`. Needed because `AppState` wraps it in `Arc<Config>` and may need to clone the underlying value during setup.
- **`Debug`** — lets us print it with `{:?}` in logs. Useful for the "what config did we load?" log line you might want to add when debugging.

> ⚠️ Be careful with `Debug` on configs that hold secrets. Anyone who prints `cfg` to a log will leak `internal_token`. We don't currently print the whole struct anywhere, but if you add a debug print, redact the secret fields manually.

### The constructor

```rust
impl Config {
    pub fn from_env() -> Result<Self> { ... }
}
```

`impl Config { ... }` declares methods on the `Config` type. `from_env` is an **associated function** (no `self` parameter — it's a constructor, called as `Config::from_env()`, not `cfg.from_env()`).

`-> Result<Self>` means "returns either a `Config` or an error." `Self` inside an `impl Config` block is sugar for `Config`.

### `max_upload_bytes` — optional with a default

```rust
let max_upload_bytes = std::env::var("MAX_UPLOAD_BYTES")
    .ok()
    .and_then(|s| s.parse().ok())
    .unwrap_or(10 * 1024 * 1024);
```

Read left to right:

1. `std::env::var("MAX_UPLOAD_BYTES")` returns `Result<String, VarError>` (`Err` if the var isn't set).
2. `.ok()` converts `Result<String, _>` to `Option<String>`. We don't care _why_ it's missing, just whether we have a value.
3. `.and_then(|s| s.parse().ok())` — `and_then` on `Option` is like `flatMap`. If we have a `Some(string)`, parse it to `usize`. `.parse()` returns `Result<usize, _>`, which `.ok()` converts to `Option<usize>`. If parsing fails (non-numeric), we end up with `None`.
4. `.unwrap_or(10 * 1024 * 1024)` — if we got `None` at any point (missing or unparseable), use 10 MiB.

The `|s| s.parse().ok()` syntax is a **closure** — an anonymous function. `|args| body`.

### Building the struct

```rust
let cfg = Self {
    r2_account_id: required("R2_ACCOUNT_ID")?,
    ...
    r2_public_base_url: required("R2_PUBLIC_BASE_URL")?
        .trim_end_matches('/')
        .to_string(),
    ...
};
```

Each field reads its env var via the `required` helper. The `?` after each call short-circuits: if any required var is missing, we return `Err(...)` immediately and never build the struct.

`R2_PUBLIC_BASE_URL` gets an extra step:
- `.trim_end_matches('/')` returns a `&str` with any trailing slashes removed. Defensive: lets the user write `https://cdn.example.com/` or `https://cdn.example.com` and we treat them the same.
- `.to_string()` converts `&str` → `String` because the struct field is `String`. Trimming returns a borrowed slice; we need owned bytes to store.

> **`&str` vs `String`** — `&str` is a borrowed view into existing string data; `String` owns the bytes on the heap. Same relationship as `&[u8]` vs `Vec<u8>`. You can always go `&str → String` by calling `.to_string()` or `.to_owned()`; the reverse is free (`&my_string` coerces to `&str`).

### Validation after construction

```rust
if cfg.internal_token.len() < 16 {
    anyhow::bail!("INTERNAL_TOKEN must be at least 16 characters long");
}
```

We validate field-level constraints **after** assembling the struct. `anyhow::bail!` is a macro that's equivalent to `return Err(anyhow::anyhow!(...))`. It just makes the early-return more readable.

A 16-char minimum is a soft guarantee against people leaving `REPLACE_ME` or `dev-secret` as the value. It's not real security — it just refuses obviously bad inputs.

### `Ok(cfg)` and the `required` helper

```rust
Ok(cfg)
```

Returns the assembled `Config` wrapped in `Ok`.

```rust
fn required(key: &str) -> Result<String> {
    std::env::var(key).with_context(|| format!("missing required env var: {key}"))
}
```

Standalone helper (not a method) at module level. Takes a `&str` (borrowed string slice — no allocation), returns `Result<String>`.

- `std::env::var(key)` returns `Result<String, std::env::VarError>`.
- `.with_context(|| format!(...))` — from the `Context` trait. If the result is `Err`, wraps the error with a human description. The closure form (`|| format!(...)`) is "lazy" — the format string is only built if there _is_ an error. (The non-lazy form is `.context("static string")`.)

So when something is missing, the error message you get is like:

```
Error: missing required env var: R2_ACCOUNT_ID

Caused by:
    environment variable not found
```

The "Caused by" comes from `anyhow` chaining the original error. Much friendlier than a panic deep in the program.

## Why this shape

| Decision                                                                | Why                                                              |
| ----------------------------------------------------------------------- | ---------------------------------------------------------------- |
| One struct with all config, built once, passed around in `AppState`     | Easy to find what knobs exist; no hidden `std::env::var` calls scattered through the codebase. |
| Required vars are required (no defaults for secrets)                    | A missing secret should fail loudly, not silently fall back to something unsafe. |
| Optional `MAX_UPLOAD_BYTES` has a sane default                          | Not a secret; reasonable global default; only override when needed. |
| Trim trailing slash from `R2_PUBLIC_BASE_URL`                           | Avoids "https://cdn.example.com//key.webp" double-slash bugs.    |
| `anyhow::Result` here, not a custom error                                | Startup errors only need to be printed to stderr and exit. No callers need to switch on the variant. |

## Common changes

- **Add a new env var** → add a field to `Config`, read it in `from_env`, update `.env.example` and the README.
- **Make an existing var optional** → wrap it in `Option<String>` and use `.ok()` instead of `?`.
- **Validate a value's format** → add a check after building `cfg` and `anyhow::bail!` if invalid.
