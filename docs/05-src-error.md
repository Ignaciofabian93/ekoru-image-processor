# `src/error.rs` — one error type, mapped to HTTP

## What this file is

A single `AppError` enum that every handler can return, plus the code that turns an `AppError` into an HTTP response with the right status code and a JSON body.

This is Rust's answer to NestJS-style HTTP exceptions: instead of `throw new BadRequestException(...)`, you `return Err(AppError::MissingField(...))`. The compiler then forces every caller to handle the possibility of failure, and the `IntoResponse` impl handles converting the variant into the right HTTP shape.

## The full file

```rust
use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("missing field: {0}")]
    MissingField(String),

    #[error("invalid field: {0}")]
    InvalidField(String),

    #[error("payload too large")]
    PayloadTooLarge,

    #[error("unauthorized")]
    Unauthorized,

    #[error("multipart error: {0}")]
    Multipart(#[from] axum::extract::multipart::MultipartError),

    #[error("image processing error: {0}")]
    Image(#[from] image::ImageError),

    #[error("storage error: {0}")]
    Storage(String),

    #[error("internal error: {0}")]
    Internal(#[from] anyhow::Error),
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let status = match &self {
            AppError::MissingField(_) | AppError::InvalidField(_) | AppError::Multipart(_) => {
                StatusCode::BAD_REQUEST
            }
            AppError::PayloadTooLarge => StatusCode::PAYLOAD_TOO_LARGE,
            AppError::Unauthorized => StatusCode::UNAUTHORIZED,
            AppError::Image(_) => StatusCode::UNPROCESSABLE_ENTITY,
            AppError::Storage(_) | AppError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };

        if status.is_server_error() {
            tracing::error!(error = %self, "server error");
        } else {
            tracing::warn!(error = %self, "client error");
        }

        (status, Json(json!({ "error": self.to_string() }))).into_response()
    }
}
```

## Concepts in play

### Enums in Rust

Rust enums are **algebraic data types** — much more powerful than C-style integer constants. Each variant can carry its own typed payload. They're the standard tool for "the thing is one of these specific shapes."

```rust
pub enum AppError {
    PayloadTooLarge,                  // no payload
    MissingField(String),             // payload: a field name
    Multipart(MultipartError),        // payload: the wrapped library error
    // ...
}
```

You construct a variant like `AppError::MissingField("file".into())` and consume it with `match`.

### `thiserror` derive

```rust
#[derive(Debug, thiserror::Error)]
pub enum AppError { ... }
```

`thiserror::Error` is a derive macro that generates two trait impls:

- `Display` — formats the error as a human-readable string using the `#[error("...")]` attribute on each variant.
- `std::error::Error` — the marker trait that lets the type integrate with Rust's error ecosystem (e.g. `?` propagation, `anyhow::Error` wrapping, source-chain walking).

Without `thiserror` you'd write both impls by hand for every variant. With it, the `#[error("missing field: {0}")]` attribute tells the macro how to format that variant — `{0}` is "the first field of the variant," which here is the field name we passed in.

### `#[from]` for automatic conversion

```rust
#[error("multipart error: {0}")]
Multipart(#[from] axum::extract::multipart::MultipartError),
```

`#[from]` tells thiserror to **also** generate `impl From<MultipartError> for AppError`. Why care? Because the `?` operator uses `From` to convert error types automatically:

```rust
let field = multipart.next_field().await?;
// ^ next_field returns Result<_, MultipartError>
//   `?` calls From::from on the MultipartError to make it an AppError
//   then returns Err(that_app_error) from our function
```

So the handler signature can be `Result<_, AppError>` and we don't have to wrap each underlying error by hand.

We use `#[from]` on the variants whose payload is a library error type we want to forward. We don't use `#[from]` on `MissingField(String)` because lots of unrelated things have `String` payloads and we don't want `?` to silently convert any old `String` into a missing-field error.

### `IntoResponse` — the bridge to axum

```rust
impl IntoResponse for AppError {
    fn into_response(self) -> Response { ... }
}
```

`IntoResponse` is an axum trait that says "I know how to become an HTTP response." Implementing it means handlers can return `Result<_, AppError>` and axum knows what to do with the `Err` case.

Without this impl, you'd have to manually construct an HTTP response in every handler. With it, every handler just `?`s its way through fallible operations and the response shape is consistent across all endpoints.

## Walkthrough

### The match — error variant → HTTP status

```rust
let status = match &self {
    AppError::MissingField(_) | AppError::InvalidField(_) | AppError::Multipart(_) => {
        StatusCode::BAD_REQUEST
    }
    AppError::PayloadTooLarge => StatusCode::PAYLOAD_TOO_LARGE,
    AppError::Unauthorized => StatusCode::UNAUTHORIZED,
    AppError::Image(_) => StatusCode::UNPROCESSABLE_ENTITY,
    AppError::Storage(_) | AppError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
};
```

- `match &self` — we borrow `self` rather than moving it, because we need `self` again on the next line to format the message.
- `|` between patterns is "or." Three variants all map to 400.
- `(_)` is a wildcard that matches whatever payload the variant carries (we don't need the payload value for status code selection).
- The compiler **enforces exhaustiveness**: if you add a new variant and forget to add a match arm, the build fails. This is one of Rust's superpowers — no "oops I forgot to handle the new case."

The mapping itself is the standard HTTP semantics:

| Variant                              | Status | Why                                                       |
| ------------------------------------ | ------ | --------------------------------------------------------- |
| `MissingField`, `InvalidField`       | 400    | The client sent something we couldn't parse meaningfully. |
| `Multipart` (parse error)            | 400    | Malformed multipart body from the client.                 |
| `PayloadTooLarge`                    | 413    | Standard for size-limit hits.                             |
| `Unauthorized`                       | 401    | Missing / wrong auth token.                               |
| `Image` (decode error)               | 422    | Body was syntactically valid (we got bytes) but not a decodable image. 422 = "I understood, but it's not processable." |
| `Storage` (R2 failure), `Internal`   | 500    | Something on our side went wrong.                          |

### Logging

```rust
if status.is_server_error() {
    tracing::error!(error = %self, "server error");
} else {
    tracing::warn!(error = %self, "client error");
}
```

Differentiates between **our fault** (5xx → error log) and **their fault** (4xx → warn log). The `error = %self` syntax is a `tracing` macro pattern:

- `error = ...` — record a structured field called `error`.
- `%self` — format `self` using its `Display` impl (the human-readable message we built with `thiserror`).
- The trailing `"server error"` / `"client error"` is the log message.

`status.is_server_error()` is a method on `StatusCode` that returns true for any 5xx.

### Building the response

```rust
(status, Json(json!({ "error": self.to_string() }))).into_response()
```

A couple of things going on:

- `Json(...)` is an axum response type — it sets `Content-Type: application/json` and serializes the value.
- `json!({ ... })` is a macro from `serde_json` that builds an in-memory JSON value. Same shape as JS object literals.
- `self.to_string()` calls `Display`, giving us the human-readable message.
- `(status, response_body)` — axum has an `IntoResponse` impl for tuples of `(StatusCode, T)`. Building this tuple and calling `.into_response()` on it produces the final HTTP response with the status code we picked and the body we built.

So clients receive a response like:

```http
HTTP/1.1 400 Bad Request
Content-Type: application/json

{"error":"missing field: entity"}
```

## How handlers use this

```rust
pub async fn handle_upload(
    State(state): State<AppState>,
    mut multipart: Multipart,
) -> Result<Json<UploadResponse>, AppError> {
    while let Some(field) = multipart.next_field().await? {
        // ↑ `?` converts MultipartError → AppError via `#[from]`, returns 400 if it fails
    }

    let raw = image_bytes.ok_or_else(|| AppError::MissingField("file".into()))?;
    // ↑ ok_or_else turns Option::None into AppError::MissingField, then `?` returns 400

    let processed = processor::process(&raw, &preset)?;
    // ↑ processor returns Result<_, image::ImageError>; `?` converts to AppError::Image → 422

    let key = storage::r2::upload(...).await?;
    // ↑ storage::r2::upload returns Result<_, AppError> directly
}
```

No try/catch noise. Errors propagate via `?` and emerge as a clean HTTP response thanks to `IntoResponse`.

## Why this shape

| Decision                                                                  | Why                                                              |
| ------------------------------------------------------------------------- | ---------------------------------------------------------------- |
| One enum for the whole app, not per-handler error types                   | Less boilerplate; consistent response shape across all endpoints. |
| `#[from]` for library errors, but not for `String`-payload variants       | Library errors should auto-convert via `?`; string-payload variants need to be explicit so we don't accidentally wrap unrelated strings. |
| `match &self` (borrow) instead of `match self` (move)                     | We need to use `self` again afterward to format the message.     |
| Log 4xx as warn, 5xx as error                                             | Separates "client did something silly" (noise to filter) from "we have a bug" (page someone). |
| JSON body, even on errors                                                  | Consistent content-type so the gateway can always `await res.json()` and pluck `.error`. |
| Don't include stack traces in the response                                 | We're behind the gateway, but defense-in-depth: if the service ever leaks to the outside, we don't want to hand attackers internal details. Full chain stays in the logs. |

## Common changes

- **Add a new error case** → add a variant, add a match arm for its status code. Compiler will tell you if you missed something.
- **Add structured details to a response** → change `Json(json!(...))` to include additional fields. Consider also changing the body to a typed struct (`#[derive(Serialize)] struct ErrorBody { error: String, code: &'static str }`).
- **Stop logging certain client errors** → add an early return for those variants before the logging block.
