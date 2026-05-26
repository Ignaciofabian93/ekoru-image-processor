# EXIF orientation — why uploaded photos rotate, and how we fix it

## The symptom

Photos uploaded through the gateway — especially portrait photos taken on a phone — were being stored rotated 90° counter-clockwise. The top of the image ended up on the left. In the upload UI everything looked fine; the moment the processed WebP came back from R2, it was sideways.

## Why this happens — EXIF Orientation

When you take a "portrait" photo on a phone, the camera sensor almost never rotates the actual pixel data. The sensor records the image in its native landscape orientation and writes an **EXIF `Orientation` tag** (a number from 1 to 8) into the JPEG header. That tag tells anything displaying the image how to rotate it for viewing.

| Value | Meaning                                  |
| ----- | ---------------------------------------- |
| 1     | Normal (no transform)                    |
| 2     | Mirrored horizontally                    |
| 3     | Rotated 180°                             |
| 4     | Mirrored vertically                      |
| 5     | Mirrored horizontally + rotated 90° CCW  |
| 6     | Rotated 90° CW   ← typical iPhone portrait |
| 7     | Mirrored horizontally + rotated 90° CW   |
| 8     | Rotated 90° CCW                          |

Two things conspire against us:

1. **Naive decoders ignore the tag.** They decode the raw pixel buffer as it is on disk — sideways. That's the rotation the user sees.
2. **Re-encoding to WebP drops the EXIF metadata.** Even if a downstream viewer *would* honour the tag, by the time we've round-tripped through `image::write_to(_, ImageFormat::WebP)`, the metadata is gone. The pixel buffer is the only thing left, and it's still sideways.

Together: decoder ignores the tag → buffer is sideways → encoder strips the tag → the stored file is permanently sideways. Exactly the `-90°` shift seen in the bug report.

## Where the bug lived

[src/processor/mod.rs](../src/processor/mod.rs), in `process`:

```rust
let img = image::load_from_memory(bytes)?;
let resized = img.resize(...);
```

`image::load_from_memory` decodes pixels but never reads EXIF. From the moment that line runs, the orientation information is lost — the rest of the pipeline (resize, WebP encode) has no way to recover it.

## The fix

`image` 0.25 exposes two helpers exactly for this:

- `ImageDecoder::orientation()` — reads the EXIF Orientation tag from the decoder, returning an `Orientation` enum (or `NoTransforms` if there isn't one).
- `DynamicImage::apply_orientation(orientation)` — physically rotates and/or flips the pixel buffer to match the tag.

The trick is that `image::load_from_memory` skips both steps. We need to go one level lower: build an `ImageReader`, turn it into a decoder, ask the decoder for its orientation, then decode into a `DynamicImage` and apply the orientation **before** resize/encode.

The new `process`:

```rust
use image::{
    imageops::FilterType, metadata::Orientation, DynamicImage, ImageDecoder, ImageFormat,
    ImageReader,
};
use std::io::Cursor;

pub fn process(bytes: &[u8], config: &ProcessConfig) -> Result<ProcessedImage, image::ImageError> {
    let reader = ImageReader::new(Cursor::new(bytes)).with_guessed_format()?;
    let mut decoder = reader.into_decoder()?;
    let orientation = decoder
        .orientation()
        .unwrap_or(Orientation::NoTransforms);
    let mut img = DynamicImage::from_decoder(decoder)?;
    img.apply_orientation(orientation);

    let resized = img.resize(config.max_width, config.max_height, FilterType::Lanczos3);
    // ... resize + WebP encode as before
}
```

### Line-by-line

```rust
let reader = ImageReader::new(Cursor::new(bytes)).with_guessed_format()?;
```

Wraps the input bytes in `ImageReader` — `image`'s higher-level reading API. `with_guessed_format()` sniffs the magic bytes (JPEG starts `FF D8 FF`, PNG `89 50 4E 47`, etc.) and locks in the right format. It returns `io::Result`, which the `?` operator converts into `image::ImageError` via the `From<io::Error>` impl that already exists on `ImageError`.

```rust
let mut decoder = reader.into_decoder()?;
```

`into_decoder` consumes the reader and gives us back a trait object implementing `ImageDecoder`. We need the decoder (rather than going straight to `DynamicImage`) because the orientation method lives on the decoder.

```rust
let orientation = decoder
    .orientation()
    .unwrap_or(Orientation::NoTransforms);
```

Ask the decoder for the EXIF Orientation. For JPEGs from phones this will usually be `Rotate90` or similar. For formats with no EXIF (PNG, WebP, GIF) the decoder returns `Ok(Orientation::NoTransforms)`. If parsing fails for any reason — corrupt EXIF, unreadable metadata — `unwrap_or` gives us the safe identity transform so the upload still succeeds.

> **Rust note — `Result::unwrap_or`.**
> Like `Option::unwrap_or`, but on `Result`. Returns the `Ok` value if present, otherwise the fallback. It's the right move here because a bad orientation tag shouldn't fail an upload; the worst-case fallback is "render as encoded."

```rust
let mut img = DynamicImage::from_decoder(decoder)?;
```

Now actually decode the pixels. This is the equivalent of `load_from_memory`, but using the decoder we already consumed orientation metadata from. Note `mut` — we're about to mutate the buffer in place.

```rust
img.apply_orientation(orientation);
```

Physically rotates / flips the pixel buffer to match the tag. After this call, the image is upright. The original EXIF tag is irrelevant from here on; the pixels themselves tell the truth.

The rest of the function (`resize`, capture dimensions, `write_to` WebP, return) is unchanged.

## Why apply orientation *before* resize, not after

Two reasons:

1. **Aspect-ratio sizing presets work in the user's coordinate system.** `config_for("user_cover")` is 1200×400 landscape. If you resize a sideways portrait into that box first, then rotate, you get a tall sliver. By applying orientation first, the resize sees the same dimensions the user does, and the preset boxes match intuition.
2. **It's one operation.** `apply_orientation` is `O(width × height)` on the *decoded* image. Doing it on the smaller, resized image would be cheaper in absolute terms, but you'd still have to do it — and the cost is negligible compared to the resize itself (Lanczos3 is the dominant cost in this pipeline).

## Why `Cargo.toml` didn't need changes

The dependency was already `image = { version = "0.25", features = ["webp"] }`. The `Orientation` enum, `ImageDecoder::orientation()`, and `DynamicImage::apply_orientation()` all ship as part of the core crate — no extra features needed.

## What it doesn't fix

- **PNGs / WebPs uploaded as portraits.** PNG has no EXIF Orientation tag. WebP does support orientation but most exporters don't write it. For these formats, if the encoder produced a sideways pixel buffer, we have no way to know — and no one does. In practice this never happens because phones write JPEG.
- **Files already stored sideways in R2.** This fix only affects new uploads. Existing sideways assets stay sideways until re-uploaded. If we ever need to backfill, the approach would be a one-off job that lists R2 objects, downloads each, re-processes, and re-uploads — but every photo in R2 has already lost its EXIF, so we have no orientation information to recover. The pragmatic fix is "re-upload from the original device."

## Verifying the fix manually

Take a portrait photo on a phone, transfer it to disk without any "auto-rotate" step (most desktop file copies preserve the EXIF), and check:

```sh
exiftool -Orientation problem.jpg
# → Orientation: Rotate 90 CW
```

Then upload it through the gateway. The stored WebP should be upright. If it isn't, the photo's EXIF is unusual — open it in your OS's image viewer and confirm it shows the right way up there too (most viewers honour EXIF, so this is a quick sanity check on the source).

## Where in the codebase

- The fix: [src/processor/mod.rs](../src/processor/mod.rs) — the `process` function.
- Where `process` is called from: [src/handlers/upload.rs](../src/handlers/upload.rs) in `handle_upload`. No changes needed there; the orientation work is encapsulated.
- The gateway repo (`ekoru-gateway`) was not modified — it never holds the decoded pixels, it just proxies multipart bytes to this service.
