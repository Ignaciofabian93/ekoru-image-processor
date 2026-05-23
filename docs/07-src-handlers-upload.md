# `src/handlers/upload.rs` — `POST /process`

## What this file is

The handler for `POST /process`. It receives a multipart upload from the gateway, validates the fields, resizes + re-encodes the image, uploads to R2, and returns JSON with the resulting key/URL and size info.

It's the busiest handler in the service. Everything else (config, state, error, auth) exists in support of this function.

## The full file

```rust
use axum::{
    extract::{Multipart, State},
    Json,
};
use serde::Serialize;

use crate::{error::AppError, processor, state::AppState, storage};

#[derive(Serialize)]
pub struct UploadResponse {
    pub key: String,
    pub url: String,
    pub original_size: usize,
    pub processed_size: usize,
    pub width: u32,
    pub height: u32,
}

pub async fn handle_upload(
    State(state): State<AppState>,
    mut multipart: Multipart,
) -> Result<Json<UploadResponse>, AppError> {
    let mut image_bytes: Option<Vec<u8>> = None;
    let mut entity: Option<String> = None;
    let mut entity_id: Option<String> = None;

    while let Some(field) = multipart.next_field().await? {
        match field.name().unwrap_or("") {
            "file" => {
                let bytes = field.bytes().await?;
                if bytes.len() > state.cfg.max_upload_bytes {
                    return Err(AppError::PayloadTooLarge);
                }
                image_bytes = Some(bytes.to_vec());
            }
            "entity" => entity = Some(field.text().await?),
            "entity_id" => entity_id = Some(field.text().await?),
            _ => {
                let _ = field.bytes().await?;
            }
        }
    }

    let raw = image_bytes.ok_or_else(|| AppError::MissingField("file".into()))?;
    let entity = entity.ok_or_else(|| AppError::MissingField("entity".into()))?;
    let entity_id = entity_id.ok_or_else(|| AppError::MissingField("entity_id".into()))?;

    if entity.is_empty() || entity.contains('/') || entity.contains("..") {
        return Err(AppError::InvalidField("entity".into()));
    }
    if entity_id.is_empty() || entity_id.contains('/') || entity_id.contains("..") {
        return Err(AppError::InvalidField("entity_id".into()));
    }

    let original_size = raw.len();
    let preset = processor::config_for(&entity);
    let processed = processor::process(&raw, &preset)?;
    let processed_size = processed.bytes.len();

    let key = storage::r2::upload(&state, &processed.bytes, &entity, &entity_id).await?;
    let url = format!("{}/{}", state.cfg.r2_public_base_url, key);

    Ok(Json(UploadResponse {
        key,
        url,
        original_size,
        processed_size,
        width: processed.width,
        height: processed.height,
    }))
}
```

## Walkthrough

### Imports + the response struct

```rust
use axum::{
    extract::{Multipart, State},
    Json,
};
use serde::Serialize;

use crate::{error::AppError, processor, state::AppState, storage};
```

- `Multipart` — axum's extractor for `multipart/form-data` bodies. Gives us an iterator-like API over the fields.
- `State` — extracts our shared `AppState`.
- `Json` — axum response type that serializes to JSON and sets `Content-Type`.
- `serde::Serialize` — the trait we derive on the response struct so it can be turned into JSON.

```rust
#[derive(Serialize)]
pub struct UploadResponse {
    pub key: String,
    pub url: String,
    pub original_size: usize,
    pub processed_size: usize,
    pub width: u32,
    pub height: u32,
}
```

A plain struct with `#[derive(Serialize)]`. Field names become JSON keys verbatim (matching the snake_case in the README's API reference). All fields are `pub` because we construct it from outside the struct.

### The handler signature

```rust
pub async fn handle_upload(
    State(state): State<AppState>,
    mut multipart: Multipart,
) -> Result<Json<UploadResponse>, AppError> {
```

- `State(state): State<AppState>` — axum extracts a clone of the state. The `State(state)` pattern destructures the wrapper.
- `mut multipart: Multipart` — extracts the multipart body. **`mut`** because `next_field()` mutates the iterator's internal cursor.
- Returns `Result<Json<UploadResponse>, AppError>`. axum's `IntoResponse` blanket impl on `Result<T, E>` makes this work: `Ok(Json(...))` becomes the 200 JSON response, `Err(AppError)` becomes the JSON error response with the right status.

> **Why isn't the parameter order arbitrary?** Extractors that consume the request body (`Multipart`, `Bytes`, `Json<T>`) must come **last** in the signature. Anything before them must be a non-body extractor (`State`, `Path`, `Query`, headers, …). This is an axum rule because only one extractor can own the body.

### Setting up accumulators

```rust
let mut image_bytes: Option<Vec<u8>> = None;
let mut entity: Option<String> = None;
let mut entity_id: Option<String> = None;
```

We don't know what order the client will send the multipart fields in. Initialize three `Option`s to `None` and fill them as we encounter each field. After the loop, we validate that all three got filled.

`Vec<u8>` is "a growable, heap-allocated array of bytes" (the equivalent of a `Buffer` in Node).

### The field loop

```rust
while let Some(field) = multipart.next_field().await? {
```

`while let Some(...)` is a loop that keeps going as long as the pattern matches. `next_field()` returns `Result<Option<Field>, MultipartError>`:

- `?` extracts the inner `Option`, propagating a parse error if there is one.
- `Some(field)` means "got a field, process it."
- `None` means "no more fields," and the loop ends.

```rust
match field.name().unwrap_or("") {
    "file" => { ... }
    "entity" => entity = Some(field.text().await?),
    "entity_id" => entity_id = Some(field.text().await?),
    _ => {
        let _ = field.bytes().await?;
    }
}
```

- `field.name()` returns `Option<&str>` (a field is technically allowed to have no name in the spec). `.unwrap_or("")` gives us an empty string in that case, which won't match any of our branches and falls into the catch-all.
- Each branch reads the field's body in a way appropriate to its type: `file` as bytes, the others as text.
- The `_` arm consumes any **unknown** field's body. **Critical**: if you skip an unknown field without reading it, the multipart parser gets desynchronized and the next `next_field()` errors. So we always drain the body, even if we throw it away (`let _ = ...`).

#### Reading the file

```rust
"file" => {
    let bytes = field.bytes().await?;
    if bytes.len() > state.cfg.max_upload_bytes {
        return Err(AppError::PayloadTooLarge);
    }
    image_bytes = Some(bytes.to_vec());
}
```

- `.bytes().await` reads the entire field body into memory as `Bytes` (a cheap-clone, reference-counted byte buffer).
- We check size **after** reading. The global `DefaultBodyLimit` in `main.rs` would already have rejected requests bigger than `max_upload_bytes + 1 MiB`, but checking per-field gives a cleaner error code and message.
- `bytes.to_vec()` converts `Bytes` to `Vec<u8>` (an owned copy). We need an owned `Vec<u8>` to hand to the image decoder.

#### Reading text fields

```rust
"entity" => entity = Some(field.text().await?),
```

`.text()` reads the body as UTF-8 string. Returns `Result<String, MultipartError>` — errors if the bytes aren't valid UTF-8.

`Some(field.text().await?)` — the `?` unwraps the `Result` (propagating errors), and we wrap the resulting `String` in `Some`.

### Required-field check

```rust
let raw = image_bytes.ok_or_else(|| AppError::MissingField("file".into()))?;
let entity = entity.ok_or_else(|| AppError::MissingField("entity".into()))?;
let entity_id = entity_id.ok_or_else(|| AppError::MissingField("entity_id".into()))?;
```

`Option::ok_or_else(|| err)` converts `Option<T>` to `Result<T, E>`:
- `Some(v)` → `Ok(v)`
- `None` → `Err(err)` (the closure is called only in this case)

Then `?` propagates the error. So missing-field → `AppError::MissingField` → 400 with a JSON message.

> **Why `ok_or_else` instead of `ok_or`?** `ok_or(err)` evaluates the error eagerly. `ok_or_else(|| err)` is lazy — only constructs the error if the `Option` is `None`. Cheap difference here (just a string allocation), but the lazy form is idiomatic for non-trivial error construction.

Note the variable shadowing trick: `let entity = entity.ok_or_else(...)?` introduces a **new** `entity` variable (of type `String`) that shadows the old `entity` (of type `Option<String>`). After this line, we can use the plain `String` directly.

### Input validation

```rust
if entity.is_empty() || entity.contains('/') || entity.contains("..") {
    return Err(AppError::InvalidField("entity".into()));
}
if entity_id.is_empty() || entity_id.contains('/') || entity_id.contains("..") {
    return Err(AppError::InvalidField("entity_id".into()));
}
```

Defense against malicious or sloppy callers. The `entity` and `entity_id` become **parts of the R2 object key** (e.g. `product/123/uuid.webp`). Without validation, a value like `..` could let someone craft a key that escapes the intended prefix. We reject anything containing `/` or `..` or that's empty.

The gateway should never send bad values, but services should never trust their callers entirely.

### Processing

```rust
let original_size = raw.len();
let preset = processor::config_for(&entity);
let processed = processor::process(&raw, &preset)?;
let processed_size = processed.bytes.len();
```

- Grab the input size **before** processing for the response.
- `processor::config_for(&entity)` returns the resize preset for this entity type (see [09-src-processor.md](09-src-processor.md)).
- `processor::process(&raw, &preset)?` runs the actual resize + WebP encode. Returns `Result<ProcessedImage, image::ImageError>`. The `?` converts an `ImageError` to `AppError::Image` (via the `#[from]` in `AppError`), which maps to a `422 Unprocessable Entity` response.
- `processed.bytes.len()` is the post-processing size, for the response.

### Uploading to R2

```rust
let key = storage::r2::upload(&state, &processed.bytes, &entity, &entity_id).await?;
let url = format!("{}/{}", state.cfg.r2_public_base_url, key);
```

`storage::r2::upload(...)` returns the **key** (path within the bucket, e.g. `product/123/uuid.webp`). It already returns `Result<String, AppError>` so we don't need any conversion.

We build the public URL by concatenating the configured base URL and the key with a single `/` separator. The base URL has had its trailing slash trimmed in `Config::from_env` so this won't double up.

### Building the response

```rust
Ok(Json(UploadResponse {
    key,
    url,
    original_size,
    processed_size,
    width: processed.width,
    height: processed.height,
}))
```

Struct literal with field shorthand (`key,` instead of `key: key,`). Wrapped in `Json(...)` for the JSON content-type, then in `Ok(...)` because the return type is `Result<_, AppError>`.

axum serializes the `UploadResponse` via its `#[derive(Serialize)]` impl and the client gets the JSON we documented in the README.

## Why this shape

| Decision                                                                     | Why                                                                       |
| ---------------------------------------------------------------------------- | ------------------------------------------------------------------------- |
| Read the whole image into memory (no streaming)                              | The whole point is to resize, which requires the whole image anyway.       |
| Per-field size check despite global `DefaultBodyLimit`                       | Gives a clean 413 with a specific message; defense in depth.               |
| Drain unknown fields rather than ignoring them                                | Multipart parser desyncs otherwise — silent corruption of subsequent reads. |
| Reject `/` and `..` in `entity` and `entity_id`                              | They become part of the R2 object key; prevents key-injection.             |
| Return both `key` and `url`                                                   | `key` for delete operations / DB storage if you'd rather rebuild URLs at read time; `url` for direct rendering. |
| Return both `original_size` and `processed_size`                              | Useful for the gateway to log compression ratios and detect anomalies.     |
| Variable shadowing after `ok_or_else`                                         | Cleaner subsequent code; idiomatic Rust pattern.                           |

## Common changes

| You want to…                                              | What to do                                                                                |
| --------------------------------------------------------- | ----------------------------------------------------------------------------------------- |
| Reject specific MIME types                                | Check `field.content_type()` in the `"file"` arm.                                          |
| Limit accepted image dimensions                            | After `image::load_from_memory` in `processor::process`, check `img.width()` / `img.height()` and return a custom error. |
| Add EXIF stripping                                         | Use the `kamadak-exif` crate or upgrade to a more capable image lib; strip before re-encoding. |
| Return multiple sizes (thumbnail + full)                   | Loop over a `Vec<ProcessConfig>` in the handler; produce a `Vec<UploadResponse>`.          |
| Support PUT (update existing key) instead of always-new UUID | Accept a `key` field; in `storage::r2::upload`, use the provided key instead of generating one. |
