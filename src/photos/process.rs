//! Image processing with the `image` crate: thumbnails and converting BMP/TIFF
//! to PNG. Pure functions on bytes (no files, no async), so they're easy to test.
//!
//! Decoding a photo is slow (tens of milliseconds to a second for a phone photo)
//! and uses the CPU the whole time, so the web handlers run these inside
//! `tokio::task::spawn_blocking`, keeping the server free for other requests.

use std::io::Cursor;

use image::{DynamicImage, ImageDecoder, ImageFormat, ImageReader, ImageResult, Limits, RgbImage};

/// Thumbnails fit inside a square this many pixels wide. The biggest place
/// they're shown is 10rem (160 CSS px); 400 stays sharp on high-density screens.
pub const THUMBNAIL_SIZE: u32 = 400;

/// JPEG quality for thumbnails (out of 100): small files, no visible blockiness.
const THUMBNAIL_QUALITY: u8 = 80;

/// Largest width or height accepted. A file can claim to be enormous while being
/// tiny (a "decompression bomb"); these limits refuse it before memory is used.
const MAX_SIDE: u32 = 16_384;

/// Read an image, turned the right way up.
///
/// Phones save photos sideways and record "rotate 90°" in the file's EXIF
/// data instead of turning the pixels. Browsers follow that note when showing
/// the original, so the thumbnail must follow it too or it would come out sideways.
pub fn decode(bytes: &[u8]) -> ImageResult<DynamicImage> {
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_SIDE);
    limits.max_image_height = Some(MAX_SIDE);
    let mut reader = ImageReader::new(Cursor::new(bytes)).with_guessed_format()?;
    reader.limits(limits);
    let mut decoder = reader.into_decoder()?;
    let orientation = decoder.orientation()?;
    let mut image = DynamicImage::from_decoder(decoder)?;
    image.apply_orientation(orientation);
    Ok(image)
}

/// A small JPEG version of the photo, for lists and galleries.
pub fn thumbnail(bytes: &[u8]) -> ImageResult<Vec<u8>> {
    let image = decode(bytes)?;
    // `thumbnail` keeps the shape (a tall photo becomes 300×400, not 400×400),
    // but it would also enlarge a small image, so only shrink big ones.
    let small = if image.width() > THUMBNAIL_SIZE || image.height() > THUMBNAIL_SIZE {
        image.thumbnail(THUMBNAIL_SIZE, THUMBNAIL_SIZE)
    } else {
        image
    };
    let mut jpeg = Vec::new();
    let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, THUMBNAIL_QUALITY);
    flatten_onto_white(&small).write_with_encoder(encoder)?;
    Ok(jpeg)
}

/// Convert any image the crate can read (here: BMP or TIFF) to PNG.
/// PNG is lossless, so nothing is lost, and it keeps transparency.
pub fn to_png(bytes: &[u8]) -> ImageResult<Vec<u8>> {
    let mut png = Vec::new();
    decode(bytes)?.write_to(&mut Cursor::new(&mut png), ImageFormat::Png)?;
    Ok(png)
}

/// JPEG has no transparency. Simply dropping it would turn see-through areas
/// black (or whatever color the hidden pixels happen to be), so blend them
/// onto white, the page's background.
fn flatten_onto_white(image: &DynamicImage) -> RgbImage {
    if !image.color().has_alpha() {
        return image.to_rgb8();
    }
    let rgba = image.to_rgba8();
    RgbImage::from_fn(rgba.width(), rgba.height(), |x, y| {
        let [r, g, b, a] = rgba.get_pixel(x, y).0;
        // Mix each channel with white (255) in proportion to how see-through it is.
        let blend = |c: u8| ((c as u16 * a as u16 + 255 * (255 - a as u16)) / 255) as u8;
        image::Rgb([blend(r), blend(g), blend(b)])
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use image::{Rgb, Rgba, RgbaImage};

    /// A `width`×`height` image of one color, saved in `format`.
    pub(crate) fn sample(width: u32, height: u32, format: ImageFormat) -> Vec<u8> {
        let image = RgbImage::from_pixel(width, height, Rgb([200, 100, 50]));
        let mut bytes = Vec::new();
        DynamicImage::ImageRgb8(image)
            .write_to(&mut Cursor::new(&mut bytes), format)
            .unwrap();
        bytes
    }

    #[test]
    fn thumbnails_are_small_jpegs_that_keep_the_shape() {
        let thumb = thumbnail(&sample(1200, 1600, ImageFormat::Png)).unwrap();
        assert!(thumb.starts_with(&[0xFF, 0xD8, 0xFF]), "a JPEG");
        let small = decode(&thumb).unwrap();
        assert_eq!((small.width(), small.height()), (300, 400));
    }

    #[test]
    fn small_images_are_not_blown_up() {
        let thumb = thumbnail(&sample(40, 20, ImageFormat::Png)).unwrap();
        let small = decode(&thumb).unwrap();
        assert_eq!((small.width(), small.height()), (40, 20));
    }

    #[test]
    fn bmp_and_tiff_become_png_with_the_same_pixels() {
        for format in [ImageFormat::Bmp, ImageFormat::Tiff] {
            let png = to_png(&sample(30, 10, format)).unwrap();
            assert!(png.starts_with(b"\x89PNG"), "{format:?}");
            let back = decode(&png).unwrap().to_rgb8();
            assert_eq!(back.dimensions(), (30, 10));
            assert_eq!(back.get_pixel(0, 0), &Rgb([200, 100, 50]));
        }
    }

    #[test]
    fn see_through_pixels_blend_onto_white() {
        let image = DynamicImage::ImageRgba8(RgbaImage::from_fn(3, 1, |x, _| match x {
            0 => Rgba([0, 0, 0, 0]),     // fully transparent black
            1 => Rgba([0, 0, 0, 255]),   // solid black
            _ => Rgba([255, 0, 0, 128]), // half see-through red
        }));
        let flat = flatten_onto_white(&image);
        assert_eq!(flat.get_pixel(0, 0), &Rgb([255, 255, 255]));
        assert_eq!(flat.get_pixel(1, 0), &Rgb([0, 0, 0]));
        assert_eq!(flat.get_pixel(2, 0), &Rgb([255, 127, 127]));
    }

    #[test]
    fn damaged_or_fake_images_are_refused() {
        // Starts like a JPEG, but isn't one.
        assert!(thumbnail(b"\xFF\xD8\xFF\xE0 pretend this is a photo").is_err());
        let mut cut = sample(100, 100, ImageFormat::Png);
        cut.truncate(cut.len() / 2);
        assert!(thumbnail(&cut).is_err());
    }

    #[test]
    fn impossibly_large_images_are_refused_before_decoding() {
        // A valid BMP header claiming 100,000 × 1 pixels.
        let mut bmp = sample(1, 1, ImageFormat::Bmp);
        bmp[18..22].copy_from_slice(&100_000u32.to_le_bytes());
        assert!(decode(&bmp).is_err());
    }
}
