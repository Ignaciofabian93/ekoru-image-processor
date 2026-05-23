# `src/handlers/delete.rs` — `DELETE /objects/{key}`

## What this file is

The handler for deleting objects from R2 by key. The gateway calls this when an image is replaced or its parent record is deleted, so we don't accumulate orphaned files in the bucket.

It's the simplest handler in the project — extract the key from the path, validate it, hand it to the storage layer, return `204 No Content`.

## The full file

```rust
use axum::{
    extract::{Path, State},
    http::StatusCode,
};

use crate::{error::AppError, state::AppState, storage};

pub async fn handle_delete(
    State(state): State<AppState>,
    Path(key): Path<String>,
) -> Result<StatusCode, AppError> {
    if key.is_empty() || key.contains("..") {
        return Err(AppError::InvalidField("key".into()));
    }

    storage::r2::delete(&state, &key).await?;
    Ok(StatusCode::NO_CONTENT)
}
```

## Walkthrough

### Imports

```rust
use axum::{
    extract::{Path, State},
    http::StatusCode,
};

use crate::{error::AppError, state::AppState, storage};
```

- `Path` — extractor that pulls captured path segments out of the URL.
- `StatusCode` — typed HTTP status codes. We use `StatusCode::NO_CONTENT` (204) as our success response.

### The signature

```rust
pub async fn handle_delete(
    State(state): State<AppState>,
    Path(key): Path<String>,
) -> Result<StatusCode, AppError> {
```

- `Path(key): Path<String>` — extracts the captured path segment(s) and parses them into a `String`. The route is registered as `"/objects/{*key}"` in `main.rs`; the `{*key}` is a "wildcard / catch-all" segment that matches anything (including slashes). So a request to `DELETE /objects/product/123/abc.webp` gives us `key = "product/123/abc.webp"`.
- Returns `Result<StatusCode, AppError>`. axum has an `IntoResponse` impl on `StatusCode` (just emits an empty-body response with that status), so we can return one directly.

> **Catch-all vs single segment**
> - `"/objects/{key}"` → matches `/objects/foo`, not `/objects/foo/bar`. `key = "foo"`.
> - `"/objects/{*key}"` → matches `/objects/foo`, `/objects/foo/bar`, `/objects/a/b/c`. `key` is whatever came after `/objects/`.
>
> We need the catch-all because R2 keys contain slashes (`entity/entity_id/uuid.webp`).

### Path validation

```rust
if key.is_empty() || key.contains("..") {
    return Err(AppError::InvalidField("key".into()));
}
```

Defense in depth:

- **Empty key** can't happen via the route (axum's wildcard requires at least one character), but we check anyway.
- **`..`** is the path-traversal sigil. R2 doesn't actually have a filesystem, so this isn't a classic LFI / path-traversal in the OS sense, but if anything downstream ever constructed a filesystem path from the key (a backup script, a logging pipeline), `..` could matter. Cheap to refuse, hard to harm.

We don't validate against `/` here because slashes are **expected** — keys legitimately contain them.

### Doing the delete

```rust
storage::r2::delete(&state, &key).await?;
Ok(StatusCode::NO_CONTENT)
```

- `storage::r2::delete` returns `Result<(), AppError>`. The `?` extracts `()` on success or propagates the error.
- `StatusCode::NO_CONTENT` is HTTP 204 — "request succeeded, no body." Standard for successful DELETEs.

A subtle behavior: R2 (like real S3) returns success when you delete a key that **doesn't exist**. So this handler returns 204 for both "deleted" and "wasn't there to begin with." That's intentional — delete is idempotent. The gateway can retry safely.

## Why this shape

| Decision                                            | Why                                                                                       |
| --------------------------------------------------- | ----------------------------------------------------------------------------------------- |
| `{*key}` wildcard route                              | Keys contain `/`; a normal single-segment capture wouldn't match them.                     |
| 204 No Content on success                            | Standard for DELETE with no body to return.                                                |
| 204 even when the object didn't exist                | R2 behaves this way; matches S3 semantics; makes delete idempotent and retry-safe.         |
| Validate against `..` but not `/`                    | `/` is part of every legitimate key; `..` never is.                                        |
| No transaction / cascade logic                       | The gateway is responsible for ordering: delete the DB row first, then call this. If the call fails, the DB delete still stands; the orphan is at worst a slow leak. |

## Common changes

| You want to…                                                       | What to do                                                                                |
| ------------------------------------------------------------------ | ----------------------------------------------------------------------------------------- |
| Return 404 when the object didn't exist                            | R2's response doesn't tell us. You'd need a prior `head_object` call (one extra request per delete). |
| Bulk delete multiple keys                                          | Accept `POST /objects:bulk-delete` with a JSON body of keys; use `delete_objects` (S3 batch API). |
| Add a soft-delete (move to a `trash/` prefix instead of deleting)  | In `storage::r2::delete`, call `copy_object` to `trash/<key>` then `delete_object` on the original. |
| Restrict who can delete (e.g. only "trash" prefix)                 | Add a prefix check on `key` and return `Unauthorized` for anything outside it.             |
