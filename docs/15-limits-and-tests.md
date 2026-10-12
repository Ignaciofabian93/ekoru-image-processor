# Decode limits and tests (2026-10-10)

Docs 02 to 10 show the files as they were before this change. This note covers what changed and why.

## Decode limits (IMG-1)

A PNG of a few hundred bytes can declare 50,000×50,000 pixels. Decoding it would ask for about 10 GB. The 10 MB upload limit cannot stop it, because the file is tiny.

`processor::open` reads only the header and refuses the image before any pixel is decoded:

| Check | Value | Where |
|---|---|---|
| Longest side | `MAX_SOURCE_SIDE` = 10,000 px | `image::Limits`, strict in every decoder |
| Decoded size | `MAX_DECODE_ALLOC` = 256 MiB | `decoder.total_bytes()`, checked by hand |

The second check is explicit because `Limits::max_alloc` is only a hint to most decoders, and `DynamicImage::from_decoder` does not check it. With both checks, the largest square that passes is 8000×8000 RGBA (256 MB). A 48 MP phone photo (8000×6000) passes.

A refused image comes back as `ImageError::Limits`. `AppError` answers it with **400** and the message `image too large: at most 10000 px per side and 256 MB decoded`. Other decode failures stay 422.

## Processing off the async workers

Decoding and Lanczos resizing are CPU work that can take most of a second. `handle_upload` now runs `processor::process` in `tokio::task::spawn_blocking`, so a burst of uploads cannot stall the Tokio workers that serve `/health` and deletes.

## Storage behind an enum

`AppState` holds `store: Arc<Store>` instead of the S3 client. In production `Store::R2` wraps the AWS SDK client exactly as before. Under `cfg(test)` there is also `Store::Memory`, a map, so the handlers run with no network. `storage::r2::upload` and `delete` match on it.

## The router moved to `src/app.rs`

`app::router(state)` builds the same routes, layers and body limit that `main` used to build inline, so tests can call it. `main` now only loads config, builds the state and serves.

## Tests (IMG-2)

| Module | Covers |
|---|---|
| `processor` | presets per entity, resize into the box, small images scaled up, EXIF orientation 6 turns a landscape JPEG portrait, a 50,000-px bomb refused from its header, the memory cap (9000 and 10,000 refused, 8000 accepted), non-images refused |
| `auth` | constant-time comparison: equal, one byte off, different length, empty |
| `app` | `/health` open; missing or wrong token → 401; a real upload stored under `product/42/<uuid>.webp` with the public URL; missing file, entity or id and path tricks → 400; over the byte limit → 413; bomb → 400 with the message; non-image → 422; delete → 204 and gone; `..` in a delete key → 400 |

The fixtures are made in memory: PNGs with the `image` encoder, the bomb as a hand-built PNG header with real CRCs, and a JPEG with an EXIF APP1 segment spliced in after SOI.

Run them with `cargo test`. A Windows machine needs the MSVC build tools to link the test binary, and `cargo check --tests` works without them. Without Rust installed, use the same image as the Dockerfile:

```sh
docker run --rm -v "$PWD":/app -w /app rust:1.94-slim-bookworm \
  sh -c 'apt-get update -qq && apt-get install -y -qq pkg-config libssl-dev >/dev/null && cargo test'
```
