use image::{
    error::{LimitError, LimitErrorKind},
    imageops::FilterType,
    metadata::Orientation,
    DynamicImage, ImageDecoder, ImageError, ImageFormat, ImageReader, ImageResult, Limits,
};
use std::io::Cursor;

/// Longest side a source image may declare, in pixels. A 48 MP phone photo
/// (8000×6000) fits; anything larger is refused from its header, before a
/// single pixel is decoded (IMG-1).
pub const MAX_SOURCE_SIDE: u32 = 10_000;

/// Ceiling for the decoded pixels, checked against the size the header
/// declares. 8000×8000 RGBA (256 MB) is the largest square that fits; a 48 MP
/// photo at 8-bit RGBA is about 192 MB.
pub const MAX_DECODE_ALLOC: u64 = 256 * 1024 * 1024;

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
        "user_avatar" => ProcessConfig {
            max_width: 400,
            max_height: 400,
        },
        "user_cover" => ProcessConfig {
            max_width: 1200,
            max_height: 400,
        },
        "product" => ProcessConfig {
            max_width: 800,
            max_height: 800,
        },
        "service" => ProcessConfig {
            max_width: 800,
            max_height: 600,
        },
        "community" => ProcessConfig {
            max_width: 1000,
            max_height: 700,
        },
        "asset" => ProcessConfig {
            max_width: 1920,
            max_height: 1080,
        },
        _ => ProcessConfig {
            max_width: 800,
            max_height: 800,
        },
    }
}

/// The decoder limits every upload is held to. A file of a few KB can declare
/// 50,000×50,000 pixels (a decompression bomb); these make the decoder refuse
/// it with `ImageError::Limits`, which the API answers with 400.
fn decode_limits() -> Limits {
    // `Limits` is non-exhaustive, so start from the defaults and tighten.
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_SOURCE_SIDE);
    limits.max_image_height = Some(MAX_SOURCE_SIDE);
    limits.max_alloc = Some(MAX_DECODE_ALLOC);
    limits
}

/// Reads the header and refuses the image before any pixel is decoded when
/// it is too large. The side limits are strict in every decoder; the
/// allocation limit is only a hint to most of them, and `from_decoder` does
/// not check it, so the declared output size is checked here as well.
fn open(bytes: &[u8]) -> ImageResult<impl ImageDecoder + '_> {
    let mut reader = ImageReader::new(Cursor::new(bytes)).with_guessed_format()?;
    reader.limits(decode_limits());
    let decoder = reader.into_decoder()?;
    if decoder.total_bytes() > MAX_DECODE_ALLOC {
        return Err(ImageError::Limits(LimitError::from_kind(
            LimitErrorKind::InsufficientMemory,
        )));
    }
    Ok(decoder)
}

pub fn process(bytes: &[u8], config: &ProcessConfig) -> Result<ProcessedImage, image::ImageError> {
    let mut decoder = open(bytes)?;
    let orientation = decoder.orientation().unwrap_or(Orientation::NoTransforms);
    let mut img = DynamicImage::from_decoder(decoder)?;
    img.apply_orientation(orientation);

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

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use image::{codecs::jpeg::JpegEncoder, ImageBuffer, Rgb, RgbImage};

    /// A PNG of the given size, encoded in memory.
    pub(crate) fn png(width: u32, height: u32) -> Vec<u8> {
        let img: RgbImage = ImageBuffer::from_fn(width, height, |x, y| {
            Rgb([(x % 256) as u8, (y % 256) as u8, 128])
        });
        let mut out = Cursor::new(Vec::new());
        img.write_to(&mut out, ImageFormat::Png).unwrap();
        out.into_inner()
    }

    /// CRC-32 (ISO-HDLC), the checksum PNG chunks carry.
    fn crc32(data: &[u8]) -> u32 {
        let mut crc = 0xFFFF_FFFFu32;
        for &byte in data {
            crc ^= byte as u32;
            for _ in 0..8 {
                let mask = (crc & 1).wrapping_neg();
                crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
            }
        }
        !crc
    }

    fn chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut out = (data.len() as u32).to_be_bytes().to_vec();
        let mut body = kind.to_vec();
        body.extend_from_slice(data);
        out.extend_from_slice(&body);
        out.extend_from_slice(&crc32(&body).to_be_bytes());
        out
    }

    /// A tiny PNG whose header claims `side`×`side` RGBA pixels: the shape of
    /// a decompression bomb. Its pixel data is never reached.
    pub(crate) fn png_claiming(side: u32) -> Vec<u8> {
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&side.to_be_bytes());
        ihdr.extend_from_slice(&side.to_be_bytes());
        ihdr.extend_from_slice(&[8, 6, 0, 0, 0]); // 8-bit RGBA, no interlace
        let mut out = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        out.extend(chunk(b"IHDR", &ihdr));
        out.extend(chunk(
            b"IDAT",
            &[0x78, 0x9C, 0x03, 0x00, 0x00, 0x00, 0x00, 0x01],
        ));
        out.extend(chunk(b"IEND", &[]));
        out
    }

    /// A JPEG carrying an EXIF orientation tag, as phone cameras write it.
    fn jpeg_with_orientation(width: u32, height: u32, orientation: u16) -> Vec<u8> {
        let img: RgbImage = ImageBuffer::from_pixel(width, height, Rgb([200, 30, 30]));
        let mut jpeg = Vec::new();
        JpegEncoder::new(&mut jpeg).encode_image(&img).unwrap();

        // EXIF: "Exif\0\0", a big-endian TIFF header, one IFD with tag 0x0112.
        let mut tiff = b"MM\x00\x2A\x00\x00\x00\x08".to_vec();
        tiff.extend_from_slice(&1u16.to_be_bytes()); // one entry
        tiff.extend_from_slice(&0x0112u16.to_be_bytes()); // Orientation
        tiff.extend_from_slice(&3u16.to_be_bytes()); // SHORT
        tiff.extend_from_slice(&1u32.to_be_bytes()); // count
        tiff.extend_from_slice(&orientation.to_be_bytes());
        tiff.extend_from_slice(&[0, 0]); // value padding
        tiff.extend_from_slice(&0u32.to_be_bytes()); // no next IFD
        let mut app1 = b"Exif\x00\x00".to_vec();
        app1.extend(tiff);

        let mut out = jpeg[..2].to_vec(); // SOI
        out.extend_from_slice(&[0xFF, 0xE1]);
        out.extend_from_slice(&((app1.len() + 2) as u16).to_be_bytes());
        out.extend(app1);
        out.extend_from_slice(&jpeg[2..]);
        out
    }

    fn is_webp(bytes: &[u8]) -> bool {
        bytes.len() > 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP"
    }

    #[test]
    fn presets_bound_each_entity() {
        let cases = [
            ("user_avatar", 400, 400),
            ("user_cover", 1200, 400),
            ("product", 800, 800),
            ("service", 800, 600),
            ("community", 1000, 700),
            ("asset", 1920, 1080),
            ("anything-else", 800, 800),
        ];
        for (entity, w, h) in cases {
            let cfg = config_for(entity);
            assert_eq!((cfg.max_width, cfg.max_height), (w, h), "{entity}");
        }
    }

    #[test]
    fn resizes_into_the_box_keeping_the_aspect_ratio() {
        let out = process(&png(1600, 800), &config_for("product")).unwrap();
        assert_eq!((out.width, out.height), (800, 400));
        assert!(is_webp(&out.bytes));
    }

    #[test]
    fn scales_small_images_up_to_the_box() {
        // `resize` fits the box both ways, so a small source is enlarged. This
        // pins today's behaviour; skipping the upscale would be a choice.
        let out = process(&png(100, 50), &config_for("product")).unwrap();
        assert_eq!((out.width, out.height), (800, 400));
    }

    #[test]
    fn applies_exif_orientation_before_resizing() {
        // Orientation 6 = rotate 90° clockwise: a 40×20 sensor image is portrait.
        let out = process(&jpeg_with_orientation(40, 20, 6), &config_for("asset")).unwrap();
        assert!(out.height > out.width, "got {}×{}", out.width, out.height);
    }

    #[test]
    fn refuses_a_decompression_bomb_from_its_header() {
        let bomb = png_claiming(50_000);
        assert!(bomb.len() < 100);
        match process(&bomb, &config_for("product")) {
            Err(image::ImageError::Limits(_)) => {}
            Err(other) => panic!("expected a limits error, got {other}"),
            Ok(_) => panic!("a 50,000×50,000 image was decoded"),
        }
    }

    #[test]
    fn refuses_an_image_inside_the_sides_but_over_the_memory_cap() {
        // 10,000×10,000 RGBA passes the side limit but would need 400 MB.
        assert!(matches!(
            open(&png_claiming(MAX_SOURCE_SIDE)),
            Err(image::ImageError::Limits(_))
        ));
        assert!(matches!(
            open(&png_claiming(9_000)),
            Err(image::ImageError::Limits(_))
        ));
    }

    #[test]
    fn accepts_the_largest_square_under_the_cap() {
        // Header checks only: 8000×8000 RGBA is exactly 256,000,000 bytes.
        assert!(open(&png_claiming(8_000)).is_ok());
    }

    #[test]
    fn rejects_bytes_that_are_not_an_image() {
        assert!(process(b"definitely not an image", &config_for("product")).is_err());
    }
}
