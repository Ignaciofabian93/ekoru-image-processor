# Documentation

These docs walk through **every file** in the project, explaining:

- What the file is for
- Which libraries / crates it uses and what each one does
- The code, block by block, with the Rust idioms called out
- Why the code is written that way (alternatives that were rejected and why)

They're written for someone comfortable with NestJS/TypeScript who is new to Rust. When a Rust concept appears for the first time, it gets a short side-bar explanation; later docs assume you've read the earlier ones.

## Reading order

If you're going through this end-to-end, read in this order:

| #  | File                                             | Topic                                                            |
| -- | ------------------------------------------------ | ---------------------------------------------------------------- |
| 01 | [01-cargo-toml.md](01-cargo-toml.md)             | `Cargo.toml` — Rust's package manifest, every dependency listed  |
| 02 | [02-src-main.md](02-src-main.md)                 | `src/main.rs` — the entry point, router, middleware wiring       |
| 03 | [03-src-config.md](03-src-config.md)             | `src/config.rs` — environment variable loading + validation      |
| 04 | [04-src-state.md](04-src-state.md)               | `src/state.rs` — the shared S3 client and `Arc` pattern          |
| 05 | [05-src-error.md](05-src-error.md)               | `src/error.rs` — `AppError` enum and HTTP response mapping       |
| 06 | [06-src-auth.md](06-src-auth.md)                 | `src/auth.rs` — `X-Internal-Token` middleware                    |
| 07 | [07-src-handlers-upload.md](07-src-handlers-upload.md) | `src/handlers/upload.rs` — POST /process                   |
| 08 | [08-src-handlers-delete.md](08-src-handlers-delete.md) | `src/handlers/delete.rs` — DELETE /objects/{key}           |
| 09 | [09-src-processor.md](09-src-processor.md)       | `src/processor/mod.rs` — resize + WebP encode                    |
| 10 | [10-src-storage-r2.md](10-src-storage-r2.md)     | `src/storage/r2.rs` — Cloudflare R2 calls via the AWS SDK        |
| 11 | [11-dockerfile.md](11-dockerfile.md)             | `Dockerfile` — multi-stage build                                 |
| 12 | [12-compose-files.md](12-compose-files.md)       | `compose.yml`, `compose.staging.yml`, `compose.prod.yml`         |
| 13 | [13-env-files.md](13-env-files.md)               | `.env`, `.env.staging`, `.env.prod`, `.env.example`              |
| 14 | [14-exif-orientation.md](14-exif-orientation.md) | EXIF orientation — why portrait photos rotated, and the fix       |
| 15 | [15-limits-and-tests.md](15-limits-and-tests.md) | 2026-10-10: decode limits, blocking pool, storage enum, tests    |

## Quick-reference: Rust idioms used everywhere

These will show up repeatedly. Skim now, refer back as needed.

### `Result<T, E>` and `?`

Rust has no exceptions. A function that can fail returns `Result<T, E>`:

```rust
fn read_token() -> Result<String, std::io::Error> { ... }
```

`Result` is an enum with two variants: `Ok(T)` for success, `Err(E)` for failure.

The `?` operator unwraps `Ok` or **returns early** with the `Err`. It's the rough equivalent of `await`-then-`try/catch` in TypeScript:

```rust
let token = read_token()?;  // if Err, returns Err from this function; if Ok, assigns String
```

### `Arc<T>`

"Atomically reference-counted pointer." Lets multiple threads share the same heap allocation safely. Cloning an `Arc<T>` is cheap — it just bumps an atomic counter. We use it to share one S3 client and one `Config` across all HTTP handlers.

### `#[derive(...)]`

A macro that auto-generates trait implementations at compile time. `#[derive(Clone)]` adds a `.clone()` method, `#[derive(Debug)]` adds a `{:?}` formatter for logs, etc. Saves boilerplate.

### `async fn` and `.await`

Same shape as TypeScript: declare async, call with `.await`. Under the hood Rust uses an executor (`tokio`) to schedule futures. Unlike TS, you must explicitly _choose_ an executor; we use `#[tokio::main]` to set one up for the whole program.

### Modules

Each `.rs` file is a module. `mod foo;` in `main.rs` includes `src/foo.rs` or `src/foo/mod.rs`. `pub` makes an item visible outside its module — without it, items are private by default.

### Traits

Equivalent to TypeScript interfaces but more powerful. The most common ones in this project:

- `IntoResponse` (axum) — "this type knows how to become an HTTP response."
- `Error` / `Display` (std) — "this type can be formatted as a human-readable error string."
- `From<X>` (std) — "you can build a `Y` from an `X`." Powers the `?` operator's type conversions.

That's enough to get started. Each doc explains more idioms as they come up.
