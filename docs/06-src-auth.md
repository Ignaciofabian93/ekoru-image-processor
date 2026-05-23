# `src/auth.rs` — `X-Internal-Token` middleware

## What this file is

A tiny piece of middleware that runs **before** the protected handlers. It checks for an `X-Internal-Token` header, compares it (in constant time) to the shared secret loaded from `INTERNAL_TOKEN`, and either lets the request through or short-circuits with `401 Unauthorized`.

This is the only authentication on the service. There's no JWT, no session, no user model. The service is private to the docker network and the only legitimate caller is the gateway, which has the same secret in its env file.

## The full file

```rust
use axum::{
    extract::{Request, State},
    middleware::Next,
    response::Response,
};

use crate::{error::AppError, state::AppState};

pub async fn require_internal_token(
    State(state): State<AppState>,
    req: Request,
    next: Next,
) -> Result<Response, AppError> {
    let presented = req
        .headers()
        .get("x-internal-token")
        .and_then(|v| v.to_str().ok());

    match presented {
        Some(tok) if constant_time_eq(tok.as_bytes(), state.cfg.internal_token.as_bytes()) => {
            Ok(next.run(req).await)
        }
        _ => Err(AppError::Unauthorized),
    }
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff: u8 = 0;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}
```

## The middleware concept (axum-flavored)

In NestJS, middleware is a class with `use(req, res, next)`. In axum it's an async function with this signature:

```rust
async fn my_middleware(
    /* extractors (State, Path, ...) */,
    req: Request,           // the incoming request
    next: Next,             // the rest of the pipeline
) -> /* something that implements IntoResponse */
```

You either call `next.run(req).await` to forward the request (and return the response), or return your own response to short-circuit. That's the whole protocol.

Our function returns `Result<Response, AppError>`:
- `Ok(Response)` for the forwarded case.
- `Err(AppError::Unauthorized)` for the rejected case — axum's `IntoResponse` impl on `Result<T, E>` then calls `AppError::into_response`, which gives us a JSON 401 (see [05-src-error.md](05-src-error.md)).

## Walkthrough

### Imports

```rust
use axum::{
    extract::{Request, State},
    middleware::Next,
    response::Response,
};

use crate::{error::AppError, state::AppState};
```

- `Request` — the inbound HTTP request, complete with headers/body/etc.
- `State` — same extractor handlers use; we need it to access `AppState` and get the configured token.
- `Next` — represents "the rest of the middleware/handler pipeline."
- `Response` — the outbound HTTP response.

### Reading the header

```rust
let presented = req
    .headers()
    .get("x-internal-token")
    .and_then(|v| v.to_str().ok());
```

Step by step:

1. `req.headers()` returns a `&HeaderMap`.
2. `.get("x-internal-token")` looks up the header (case-insensitive). Returns `Option<&HeaderValue>` — `None` if absent.
3. `.and_then(|v| v.to_str().ok())` — `HeaderValue` is stored as raw bytes. `.to_str()` returns `Result<&str, _>` (fails if the value contains non-ASCII bytes). `.ok()` converts to `Option<&str>`. The `and_then` chains the conversion, giving us `Option<&str>` overall: `Some(tok)` if the header is present and ASCII, `None` otherwise.

So `presented: Option<&str>` cleanly represents "the token we got, if any."

> **Why "x-internal-token" lowercase?** HTTP headers are case-insensitive, and the `HeaderMap` API normalizes to lowercase internally. Using lowercase consistently in code avoids any surprises.

### The match — accept or reject

```rust
match presented {
    Some(tok) if constant_time_eq(tok.as_bytes(), state.cfg.internal_token.as_bytes()) => {
        Ok(next.run(req).await)
    }
    _ => Err(AppError::Unauthorized),
}
```

This is a `match` with a **match guard** (the `if ...` clause).

- The first arm matches `Some(tok)` **and** the guard `constant_time_eq(...)` is true. Both conditions must hold.
- The second arm `_` is a catch-all: header missing, header malformed, or token wrong → unauthorized.

When accepted, `next.run(req).await` forwards the request to the next layer / the handler. It returns a `Response`, which we wrap in `Ok`.

Rejecting is just `Err(AppError::Unauthorized)`. The `IntoResponse` impl on `AppError` handles the rest.

### Constant-time comparison

```rust
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff: u8 = 0;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}
```

**Why constant time?** Naive byte-by-byte comparison short-circuits at the first mismatch. An attacker who can measure response time precisely could in principle learn the prefix of the secret one byte at a time by trying different first bytes and seeing which one takes a tiny bit longer (because it matched and the comparison moved on to byte two).

This implementation:

1. Bails fast on length mismatch (not a timing leak — token length isn't secret in this design; both sides know it's hex from `openssl rand -hex 32`).
2. XORs each pair of bytes into an accumulator (`diff`). `x ^ y` is 0 iff `x == y`; `|=` accumulates.
3. After the loop, `diff == 0` iff every pair matched. The loop **always** processes the full length regardless of where mismatches occur — same number of operations, same execution time.

This is fine to roll by hand for a few-byte comparison. For longer comparisons or in real adversarial settings you'd want the `subtle` crate's `ConstantTimeEq`. Here, with a 64-char token and an internal-only service, this hand-rolled version is plenty.

> **Bitwise ops refresher**
> `^` (XOR) is "bits differ in this position." `|=` is "set the bits on the left to also include the bits on the right." So `diff` ends with a 1-bit wherever **any** byte pair differed. Then `diff == 0` is a single integer comparison.

### `for (x, y) in a.iter().zip(b.iter())`

`iter()` produces an iterator yielding `&u8` (one byte at a time). `zip(other)` pairs items from two iterators, yielding `(&u8, &u8)` tuples. The `(x, y)` in the `for` pattern destructures each tuple.

`x ^ y` then operates on the dereferenced bytes (Rust auto-derefs single-byte references for the XOR operator).

## Why this shape

| Decision                                                | Why                                                                                          |
| ------------------------------------------------------- | -------------------------------------------------------------------------------------------- |
| Shared secret in a header, not JWT                      | Service is internal; gateway already terminates user auth. JWT adds a moving part with no security benefit. |
| Token loaded from env at startup, not per-request       | Trivial perf win; also forces a restart on rotation, which is fine.                          |
| Constant-time compare                                   | Cheap defense against timing attacks even though they're unlikely here.                       |
| Middleware returns `Result<Response, AppError>`         | Reuses the same `IntoResponse` plumbing as handlers — one response shape across the app.     |
| Only on protected sub-router, not the root              | `/health` must stay open so Docker's healthcheck doesn't need secrets.                       |
| Lowercase header lookup                                 | Defensive — header names are case-insensitive but it's safer to be explicit.                  |

## Common changes

| You want to…                                            | What to do                                                                                   |
| ------------------------------------------------------- | -------------------------------------------------------------------------------------------- |
| Rotate the token                                        | Update both env files (this service + gateway), restart both. Pick a maintenance window.     |
| Add IP allowlisting                                     | Check `req.extensions().get::<ConnectInfo<SocketAddr>>()` (requires `Router::into_make_service_with_connect_info`). |
| Allow multiple tokens (for staged rotation)             | Change `internal_token: String` to `internal_tokens: Vec<String>` in `Config`; check membership in the match guard. |
| Use a stronger constant-time compare                    | Add `subtle = "2"` to `Cargo.toml`; use `subtle::ConstantTimeEq` and check `.ct_eq(...).into()`. |
