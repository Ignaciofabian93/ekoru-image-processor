# `src/processor/mod.rs` — resize + WebP encode

## What this file is

The image-processing logic. Two responsibilities:

1. Pick the **right resize preset** based on the entity type (avatar = 400×400, product = 800×800, etc.).
2. Take raw image bytes, decode them, resize while preserving aspect ratio, re-encode as WebP, return the bytes plus the final width/height.

It's pure (no I/O, no network), which makes it easy to test and easy to swap out — the rest of the app only talks to it through `config_for(entity)` and `process(bytes, &preset)`.

## The full file

```rust
use image::{imageops::FilterType, ImageFormat};
use std::io::Cursor;

pub struct ProcessConfig {
    pub max_width: u32,
    pub max_height: u32,
}

pub struct ProcessedImage {
    pub bytes: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

pub fn config_for(entity: &str) -> ProcessConfig {
    match entity {
        "user_avatar" => ProcessConfig { max_width: 400, max_height: 400 },
        "user_cover" => ProcessConfig { max_width: 1200, max_height: 400 },
        "product" => ProcessConfig { max_width: 800, max_height: 800 },
        "service" => ProcessConfig { max_width: 800, max_height: 600 },
        "community" => ProcessConfig { max_width: 1000, max_height: 700 },
        "asset" => ProcessConfig { max_width: 1920, max_height: 1080 },
        _ => ProcessConfig { max_width: 800, max_height: 800 },
    }
}

pub fn process(bytes: &[u8], config: &ProcessConfig) -> Result<ProcessedImage, image::ImageError> {
    let img = image::load_from_memory(bytes)?;
    let resized = img.resize(config.max_width, config.max_height, FilterType::Lanczos3);
    let width = resized.width();
    let height = resized.height();

    let mut output = Cursor::new(Vec::new());
    resized.write_to(&mut output, ImageFormat::WebP)?;

    Ok(ProcessedImage {
        bytes: output.into_inner(),
        width,
        height,
    })
}
```

## File location: `mod.rs` vs `processor.rs`

This file lives at `src/processor/mod.rs` rather than `src/processor.rs`. Both forms are valid in Rust 2021 and produce the same module. The `mod.rs` form is convenient when you want the module to grow into a folder containing sub-modules (e.g. `src/processor/webp.rs`, `src/processor/jpeg.rs`). If we ever split processing strategies into separate files, this layout makes adding them painless.

## Walkthrough

### Imports

```rust
use image::{imageops::FilterType, ImageFormat};
use std::io::Cursor;
```

- `image::imageops::FilterType` — the resampling algorithm enum (Lanczos3, Triangle, Nearest, …).
- `image::ImageFormat` — enum of supported encode/decode formats.
- `std::io::Cursor` — a wrapper that makes an in-memory `Vec<u8>` look like a file (i.e. implements `Read + Write + Seek`). Required because the `image` encoder writes to anything that implements `Write` — and we want it to write to memory, not a file.

### The data types

```rust
pub struct ProcessConfig {
    pub max_width: u32,
    pub max_height: u32,
}

pub struct ProcessedImage {
    pub bytes: Vec<u8>,
    pub width: u32,
    pub height: u32,
}
```

Two plain structs:

- `ProcessConfig` describes how to resize (the box the image should fit inside).
- `ProcessedImage` is the result — encoded bytes plus the final dimensions (the image was scaled, so the final width/height differ from the originals).

`u32` is "unsigned 32-bit integer" — image dimensions can't be negative. The `image` crate uses `u32` for dimensions, so we mirror it. Going up to ~4 billion pixels per side is plenty.

### `config_for` — entity → resize box

```rust
pub fn config_for(entity: &str) -> ProcessConfig {
    match entity {
        "user_avatar" => ProcessConfig { max_width: 400, max_height: 400 },
        "user_cover" => ProcessConfig { max_width: 1200, max_height: 400 },
        ...
        _ => ProcessConfig { max_width: 800, max_height: 800 },
    }
}
```

Plain string matching. Each arm constructs a `ProcessConfig` struct literal.

The `_` arm is the catch-all default. Picked because:

- The handler validates `entity` is non-empty and free of `/` and `..`, but doesn't restrict it to a known list. We don't want unknown values to crash; we want a sensible default.
- 800×800 is roughly average across our presets — won't massively under- or over-resize anything reasonable.

If you'd rather reject unknown entities outright, change `_` to return a `Result<ProcessConfig, _>` and update the handler to forward that error. We chose lenient because the gateway is in control of `entity` values and unknown ones suggest a feature in flight, not malice.

### `process` — the real work

```rust
pub fn process(bytes: &[u8], config: &ProcessConfig) -> Result<ProcessedImage, image::ImageError> {
```

Takes:

- `bytes: &[u8]` — borrowed slice of the input image bytes. `&[u8]` (a "byte slice") is the universal "some bytes I'll read but not modify" type.
- `config: &ProcessConfig` — borrowed reference to the resize preset.

Returns:

- `Result<ProcessedImage, image::ImageError>` — success gives back the processed image; failure is whatever the `image` crate complains about (corrupt input, unsupported format, encoding error, …).

It's a **synchronous** function. There's no I/O — only CPU work. Calling it from an async handler is fine because the work is short-lived (typical image: tens of ms). If we ever start handling very large images that take hundreds of ms, we'd wrap the call in `tokio::task::spawn_blocking` to keep the async executor responsive.

#### Decoding

```rust
let img = image::load_from_memory(bytes)?;
```

`image::load_from_memory` autodetects the format from magic bytes (JPEG starts with `FF D8 FF`, PNG with `89 50 4E 47`, …) and decodes the whole file into a `DynamicImage`. `DynamicImage` is an enum wrapping every supported pixel layout (RGB8, RGBA8, RGB16, …).

The `?` propagates any decode error as `image::ImageError`. In the handler, the `#[from]` conversion in `AppError` turns that into `AppError::Image` → HTTP 422.

#### Resizing

```rust
let resized = img.resize(config.max_width, config.max_height, FilterType::Lanczos3);
```

`DynamicImage::resize` scales the image so it fits **inside** the `max_width × max_height` box while preserving aspect ratio. Important: it never upscales. If the input is already smaller than the box, it's returned unchanged in dimensions.

`FilterType::Lanczos3` is the resampling algorithm:

| Filter      | Quality | Speed       | Notes                                            |
| ----------- | ------- | ----------- | ------------------------------------------------ |
| `Nearest`   | Bad     | Fastest     | Pixelated, only for thumbnails of pixel art.     |
| `Triangle`  | OK      | Fast        | Linear interpolation. Slight blur.               |
| `CatmullRom`| Good    | Medium      | Sharper than triangle.                            |
| `Gaussian`  | OK      | Medium      | Soft blur.                                       |
| `Lanczos3`  | Best    | Slowest     | Three-lobe sinc. Standard for photographs.       |

For user-uploaded photos, Lanczos3 is the right default. The few ms of extra CPU per image are unnoticeable.

#### Capturing dimensions

```rust
let width = resized.width();
let height = resized.height();
```

We pull these out **before** encoding because `write_to` consumes the image (`resized` is moved into it). After the next call, `resized` is no longer accessible.

#### Encoding

```rust
let mut output = Cursor::new(Vec::new());
resized.write_to(&mut output, ImageFormat::WebP)?;
```

- `Cursor::new(Vec::new())` builds an empty in-memory buffer that pretends to be a file. As bytes are written, the inner `Vec` grows.
- `resized.write_to(&mut output, ImageFormat::WebP)?` encodes the image and writes to our buffer. The `?` propagates any encoding error.

> **Why a `Cursor` and not just a `Vec<u8>` directly?**
> `Vec<u8>` doesn't implement the `Write` trait the way `image::write_to` needs (it wants `Write + Seek` for some formats). `Cursor<Vec<u8>>` does, and is the standard pattern for "write to a growable buffer in memory."

#### Returning the result

```rust
Ok(ProcessedImage {
    bytes: output.into_inner(),
    width,
    height,
})
```

`Cursor::into_inner()` consumes the cursor and returns the underlying `Vec<u8>` — the bytes the encoder wrote. We wrap that plus dimensions in a `ProcessedImage` and `Ok` it.

## WebP quality

Right now the encoder produces **lossless** WebP, because the `image` crate's bundled WebP encoder (`image-webp`) doesn't yet support lossy encoding. Lossless WebP still compresses photographic content well — typically 30–60% of the original JPEG/PNG size — but it's not as good as lossy WebP at the same visual quality.

If you need lossy WebP (smaller files):

1. Add `webp = "0.3"` to `Cargo.toml`.
2. In `process`, replace the `write_to` block with:
   ```rust
   let encoder = webp::Encoder::from_image(&resized)
       .map_err(image::ImageError::IoError)?;
   let webp = encoder.encode(80.0); // quality 0..=100
   let bytes = webp.to_vec();
   ```

We defaulted to lossless because (a) it doesn't add a C dependency and (b) for the sizes we resize to, the lossless WebPs are already small enough.

## Why this shape

| Decision                                                  | Why                                                                  |
| --------------------------------------------------------- | -------------------------------------------------------------------- |
| Pure function, no I/O                                     | Easy to test; doesn't tie us to a specific async runtime.            |
| Always preserve aspect ratio (no cropping)                | Cropping decisions are UI-policy; the gateway can crop later if needed (e.g. CSS `object-fit`). |
| `Lanczos3` filter                                          | Best quality for photographs; the CPU cost is negligible at our sizes. |
| Capture width/height before encoding                       | `write_to` consumes the image; getting them after would require recomputing or re-decoding. |
| Return a struct, not a tuple                               | Named fields are self-documenting at call sites.                     |
| One file format (WebP) for all outputs                     | Predictable downstream; modern browsers/apps all support WebP.       |

## Common changes

| You want to…                                                | What to do                                                                                  |
| ----------------------------------------------------------- | ------------------------------------------------------------------------------------------- |
| Crop instead of letterbox                                   | Use `img.resize_to_fill(w, h, filter)` instead of `resize`. Produces an exact `w × h` output. |
| Generate a thumbnail plus a full-size image                 | Run `process` twice with different presets; return both from the handler.                    |
| Add EXIF stripping / privacy                                 | `image::load_from_memory` already discards EXIF on re-encode. Lat/long etc. are gone.       |
| Add JPEG fallback for very old browsers                     | Branch in `process` on a "format" preset; encode as `ImageFormat::Jpeg` when requested.      |
| Cap input dimensions before resizing (DOS protection)       | Check `img.width() * img.height()` against a max megapixel count, return custom error.       |
