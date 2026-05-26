use image::{
    imageops::FilterType, metadata::Orientation, DynamicImage, ImageDecoder, ImageFormat,
    ImageReader,
};
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
    let reader = ImageReader::new(Cursor::new(bytes)).with_guessed_format()?;
    let mut decoder = reader.into_decoder()?;
    let orientation = decoder
        .orientation()
        .unwrap_or(Orientation::NoTransforms);
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
