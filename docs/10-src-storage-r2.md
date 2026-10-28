# `src/storage/r2.rs` — Cloudflare R2 client

## What this file is

The thin layer that talks to Cloudflare R2 using the AWS S3 SDK. Two public functions:

- `upload(state, bytes, entity, entity_id)` → `Result<String, AppError>` — uploads the bytes, returns the resulting object key.
- `delete(state, key)` → `Result<(), AppError>` — deletes the object.

It doesn't construct an S3 client — the shared one lives in `AppState` (see [04-src-state.md](04-src-state.md)). It just borrows that client to make calls.

## The full file

```rust
use aws_sdk_s3::primitives::ByteStream;
use uuid::Uuid;

use crate::{error::AppError, state::AppState};

pub async fn upload(
    state: &AppState,
    bytes: &[u8],
    entity: &str,
    entity_id: &str,
) -> Result<String, AppError> {
    let key = format!("{entity}/{entity_id}/{}.webp", Uuid::new_v4());

    state
        .s3
        .put_object()
        .bucket(&state.cfg.r2_bucket)
        .key(&key)
        .body(ByteStream::from(bytes.to_vec()))
        .content_type("image/webp")
        .cache_control("public, max-age=31536000, immutable")
        .send()
        .await
        .map_err(|e| AppError::Storage(format!("put_object: {e}")))?;

    Ok(key)
}

pub async fn delete(state: &AppState, key: &str) -> Result<(), AppError> {
    state
        .s3
        .delete_object()
        .bucket(&state.cfg.r2_bucket)
        .key(key)
        .send()
        .await
        .map_err(|e| AppError::Storage(format!("delete_object: {e}")))?;

    Ok(())
}
```

## R2 as S3

Cloudflare R2 implements the S3 wire protocol. From the AWS SDK's perspective it's a regular S3 endpoint — you just point the `endpoint_url` at R2 instead of AWS (done in [`state.rs`](../src/state.rs)) and use static credentials issued by Cloudflare. Every method (`put_object`, `delete_object`, `head_object`, `get_object`, etc.) works the same.

Two minor differences in practice:
- R2's region is conceptually `"auto"` (Cloudflare's name for "we don't have AWS-style regions").
- R2 lacks some niche S3 features (Object Lambda, Glacier, …) — none of which we use.

## Walkthrough

### Imports

```rust
use aws_sdk_s3::primitives::ByteStream;
use uuid::Uuid;

use crate::{error::AppError, state::AppState};
```

- `ByteStream` — the SDK's body type for `put_object`. Wraps a `Vec<u8>`, file handle, or async stream.
- `Uuid` — for generating unique object key suffixes.

### `upload` — function signature

```rust
pub async fn upload(
    state: &AppState,
    bytes: &[u8],
    entity: &str,
    entity_id: &str,
) -> Result<String, AppError> {
```

Borrowed inputs everywhere. We don't need to own any of these — we just look at them long enough to make the request.

- `&AppState` — to get at `state.s3` (the client) and `state.cfg.r2_bucket` (the bucket name).
- `&[u8]` — the image bytes.
- Two `&str` — entity type and ID, used to build the key.

Returns the key as `String` (we generate it, the handler needs to own it for the response).

### Building the key

```rust
let key = format!("{entity}/{entity_id}/{}.webp", Uuid::new_v4());
```

`format!` is the string-formatting macro. Returns a new `String`.

- `{entity}` and `{entity_id}` use **named interpolation** — they pull from variables of those names in scope. (Stabilized in Rust 1.58.)
- `{}` is a **positional** placeholder — filled by the first non-named argument, here `Uuid::new_v4()`.
- `Uuid::new_v4()` generates a random version-4 UUID. Its `Display` impl produces the standard hyphenated form (`f81d4fae-7dec-11d0-a765-00a0c91e6bf6`).

Resulting key pattern: `entity/entity_id/uuid.webp`. Examples:

- `user_avatar/42/8c4e1a90-5c5d-4f0c-89e8-aaf21ba9f9bb.webp`
- `product/9001/0190b8c9-4f8b-7e3a-8c8d-1234567890ab.webp`

Why this layout:

| Element     | Why                                                                                                 |
| ----------- | --------------------------------------------------------------------------------------------------- |
| `entity/`   | Browseability in the R2 dashboard — group like with like.                                            |
| `entity_id/`| Easy to find all images for a given record.                                                          |
| UUID        | Guarantees no collisions even if the same record is uploaded to many times. Also avoids guessable URLs (some defense against scrapers). |
| `.webp`     | Useful for any tooling that infers content type from extension (CDN cache rules, logging dashboards). |

### Building and sending the request

```rust
state
    .s3
    .put_object()
    .bucket(&state.cfg.r2_bucket)
    .key(&key)
    .body(ByteStream::from(bytes.to_vec()))
    .content_type("image/webp")
    .cache_control("public, max-age=31536000, immutable")
    .send()
    .await
    .map_err(|e| AppError::Storage(format!("put_object: {e}")))?;
```

The AWS SDK uses the **fluent builder** pattern: `put_object()` returns a builder, each `.something(...)` configures one field, `.send()` actually fires the HTTP request.

- `.put_object()` — starts a `PutObject` builder.
- `.bucket(&state.cfg.r2_bucket)` — which bucket. Borrowing here is fine; the builder copies into its internal state.
- `.key(&key)` — the object key (the path within the bucket).
- `.body(ByteStream::from(bytes.to_vec()))` — the actual bytes. We `bytes.to_vec()` to give the stream an owned copy because the SDK takes ownership of the body. This is one place where we accept the allocation — there's no way around it in the current SDK shape.
- `.content_type("image/webp")` — sets the `Content-Type` header on the stored object. Critical: without this, R2 returns the file with `Content-Type: application/octet-stream` and browsers download instead of displaying it.
- `.cache_control("public, max-age=31536000, immutable")` — sets the `Cache-Control` response header R2 will return when serving this object.
  - `public` — any cache may store it.
  - `max-age=31536000` — one year (in seconds).
  - `immutable` — promises the content at this URL will never change. Browsers skip revalidation entirely. Safe to assert because every upload gets a brand-new UUID — old URLs always point to old content.
- `.send()` — returns a `Future` that resolves to a `Result<PutObjectOutput, SdkError<PutObjectError>>`.
- `.await` — waits for the future to complete.
- `.map_err(|e| AppError::Storage(format!("put_object: {}", DisplayErrorContext(&e))))` — converts the SDK error into our error type with a descriptive prefix. `DisplayErrorContext` prints the whole source chain, so the log shows R2's reason (`AccessDenied`, `NoSuchBucket`, `SignatureDoesNotMatch`…); a plain `{e}` prints only "service error". The gateway only logs this text; users get a generic error.
- `?` — propagates the error if any. We don't need the `PutObjectOutput` (it contains `etag`, `version_id`, etc. that we don't use), so we just discard it on the success path.

### `Ok(key)`

We return the key. The handler combines it with `R2_PUBLIC_BASE_URL` to produce the public URL.

### `delete` — same shape, fewer fields

```rust
pub async fn delete(state: &AppState, key: &str) -> Result<(), AppError> {
    state
        .s3
        .delete_object()
        .bucket(&state.cfg.r2_bucket)
        .key(key)
        .send()
        .await
        .map_err(|e| AppError::Storage(format!("delete_object: {e}")))?;

    Ok(())
}
```

Mirror of `upload` minus the body. `delete_object` is idempotent in S3/R2 — deleting a key that doesn't exist returns success. The handler relies on that to be retry-safe.

## Why this shape

| Decision                                                                | Why                                                                                  |
| ----------------------------------------------------------------------- | ------------------------------------------------------------------------------------ |
| Borrow `&AppState` instead of `&S3Client + &Config`                     | Fewer parameters, and `AppState` is the canonical place to find both.                |
| UUID v4 in the key, not a hash of the bytes                              | We want every upload to produce a new URL (so cache invalidation is "just upload"). Hashing would dedupe across users, which has surprising privacy implications. |
| `Cache-Control: immutable, max-age=1y`                                   | Combined with UUID keys, this is the ideal cache config: viewers never revalidate; updates are a new URL. |
| `.to_vec()` on the body bytes                                            | The SDK wants owned bytes for the body. Worth the allocation for clarity.            |
| `map_err` to `AppError::Storage` instead of `#[from]`                    | The SDK errors are nested generics (`SdkError<PutObjectError>`) and we'd need many `#[from]` impls. Wrapping them in `DisplayErrorContext` is simpler and keeps R2's reason in the message. |
| No retries here                                                          | The AWS SDK retries by default for transient errors. If you need custom retry policy, configure it on the SDK's `RetryConfig` in `state.rs`. |
| `delete_object` always returns 204 even when missing                     | Matches S3 semantics; lets the handler be idempotent without extra logic.            |

## Common changes

| You want to…                                                | What to do                                                                                  |
| ----------------------------------------------------------- | ------------------------------------------------------------------------------------------- |
| List objects under a prefix (e.g. "all images for product 9001") | Add a `list(state, prefix)` function calling `state.s3.list_objects_v2().prefix(...)`. Iterate `output.contents()`. |
| Check whether an object exists                              | Use `head_object`: `state.s3.head_object().bucket(...).key(...).send().await`. Returns 404 → object doesn't exist. |
| Generate presigned URLs (e.g. for direct browser uploads)   | Use `put_object().presigned(PresigningConfig::expires_in(...))`.                            |
| Override `Cache-Control` per entity                          | Add a `cache_control: &'static str` field to `ProcessConfig`; thread it through to `upload`. |
| Add metadata (e.g. uploader user ID)                         | Chain `.metadata("uploaded-by", user_id)` before `.send()`. Stored as `x-amz-meta-uploaded-by` header on the object. |
| Switch to a different bucket per call                       | Accept `bucket: &str` as a parameter; replace `&state.cfg.r2_bucket` with it.               |
