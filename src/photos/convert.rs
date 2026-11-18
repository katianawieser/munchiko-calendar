//! Converting HEIC (the iPhone's default) and AVIF photos to JPEG.
//!
//! There's no dependable pure-Rust decoder for either: HEIC stores its pixels
//! with the HEVC video codec, which is complex and patent-encumbered, and
//! AVIF decoding in the `image` crate needs a C library. macOS ships a
//! command-line tool, `sips` ("scriptable image processing system"), that uses
//! the system's own decoders, so on a Mac the app hands the conversion to it.
//! On other systems there's no converter and the upload page explains how to
//! save a JPEG instead.
//!
//! The converter is a value chosen once at startup ([`PhotoConverter::system`])
//! and kept in the app's state, so tests can pick "no converter" and the
//! decision lives in one place.

use std::path::{Path, PathBuf};
use std::time::Duration;

use super::{Foreign, ImageType, NotAPhoto, Upload, detect};

/// Where macOS keeps the tool.
const SIPS: &str = "/usr/bin/sips";

/// A conversion takes about a second; anything much longer is stuck.
const TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PhotoConverter {
    Unavailable,
    /// Path to the `sips` program.
    Sips(PathBuf),
}

impl PhotoConverter {
    /// `sips` on a Mac that has it; otherwise none.
    pub fn system() -> Self {
        if cfg!(target_os = "macos") && Path::new(SIPS).exists() {
            PhotoConverter::Sips(PathBuf::from(SIPS))
        } else {
            PhotoConverter::Unavailable
        }
    }

    pub fn describe(&self) -> &'static str {
        match self {
            PhotoConverter::Unavailable => "not available (save them as JPEG first)",
            PhotoConverter::Sips(_) => "converted to JPEG with macOS sips",
        }
    }

    /// Turn a HEIC or AVIF photo (`format`) into JPEG bytes.
    pub async fn to_jpeg(&self, format: Foreign, photo: &[u8]) -> Result<Vec<u8>, NotAPhoto> {
        let PhotoConverter::Sips(sips) = self else {
            return Err(NotAPhoto::NoConverter(format));
        };
        let jpeg = run_sips(sips, format, photo).await.map_err(|e| {
            eprintln!("{} conversion failed: {e}", format.name());
            NotAPhoto::ConversionFailed(format)
        })?;
        // Trust, but verify: make sure a JPEG really came out.
        match detect(&jpeg) {
            Ok(Upload::Ready(ImageType::Jpeg)) => Ok(jpeg),
            _ => Err(NotAPhoto::ConversionFailed(format)),
        }
    }
}

/// `sips` works on files, so write the upload to a private temporary folder,
/// convert it there, and read the result back. The folder (and both files) is
/// deleted when `dir` goes out of scope, even if something fails.
async fn run_sips(sips: &Path, format: Foreign, photo: &[u8]) -> Result<Vec<u8>, String> {
    let dir = tempfile::tempdir().map_err(|e| e.to_string())?;
    // A matching extension, in case sips goes by the name.
    let extension = match format {
        Foreign::Avif => "avif",
        _ => "heic",
    };
    let input = dir.path().join(format!("upload.{extension}"));
    let output = dir.path().join("converted.jpg");
    tokio::fs::write(&input, photo)
        .await
        .map_err(|e| e.to_string())?;

    // The arguments are a fixed list plus our own two paths, never anything the
    // user typed, so nothing they send can change what the command does.
    let run = tokio::process::Command::new(sips)
        .args(["-s", "format", "jpeg", "-s", "formatOptions", "90"])
        .arg(&input)
        .arg("--out")
        .arg(&output)
        .kill_on_drop(true) // if the timeout fires, stop the process too
        .output();
    let result = tokio::time::timeout(TIMEOUT, run)
        .await
        .map_err(|_| "sips took too long".to_string())?
        .map_err(|e| e.to_string())?;
    if !result.status.success() {
        return Err(format!(
            "sips failed: {}",
            String::from_utf8_lossy(&result.stderr).trim()
        ));
    }
    // sips reports success even when it skips a file it can't read ("not a
    // valid file - skipping"), so a missing output file is the real sign of
    // failure. (The caller also checks that the output is a JPEG.)
    tokio::fs::read(&output)
        .await
        .map_err(|e| format!("sips produced no file: {e}"))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[tokio::test]
    async fn without_a_converter_heic_is_explained() {
        assert_eq!(
            PhotoConverter::Unavailable
                .to_jpeg(Foreign::Avif, b"anything")
                .await,
            Err(NotAPhoto::NoConverter(Foreign::Avif))
        );
    }

    #[tokio::test]
    async fn garbage_fails_cleanly_instead_of_crashing() {
        let converter = PhotoConverter::system();
        if converter == PhotoConverter::Unavailable {
            return; // not a Mac: nothing to try
        }
        assert_eq!(
            converter
                .to_jpeg(Foreign::Heic, b"\0\0\0\x18ftypheic not really")
                .await,
            Err(NotAPhoto::ConversionFailed(Foreign::Heic))
        );
    }

    /// On a Mac, make a real HEIC (with sips itself) and convert it back.
    #[tokio::test]
    async fn sips_turns_heic_into_jpeg() {
        let converter = PhotoConverter::system();
        let PhotoConverter::Sips(sips) = &converter else {
            return; // not a Mac: nothing to try
        };
        let heic = sample_heic(sips).await;
        assert_eq!(
            detect(&heic),
            Ok(Upload::Convert(super::super::Foreign::Heic))
        );

        let jpeg = converter.to_jpeg(Foreign::Heic, &heic).await.unwrap();
        let image = super::super::process::decode(&jpeg).unwrap();
        assert_eq!((image.width(), image.height()), (64, 48));
    }

    /// A 64×48 HEIC photo, made by converting a PNG with sips.
    pub(crate) async fn sample_heic(sips: &Path) -> Vec<u8> {
        sample_as(sips, "heic")
    }

    /// A 64×48 photo in `format` ("heic", "avif"), made from a PNG by sips.
    fn sample_as(sips: &Path, format: &str) -> Vec<u8> {
        let dir = tempfile::tempdir().unwrap();
        let png = dir.path().join("in.png");
        let out = dir.path().join(format!("out.{format}"));
        let bytes = super::super::process::tests::sample(64, 48, image::ImageFormat::Png);
        std::fs::write(&png, bytes).unwrap();
        let status = std::process::Command::new(sips)
            .args(["-s", "format", format])
            .arg(&png)
            .arg("--out")
            .arg(&out)
            .output()
            .unwrap()
            .status;
        assert!(status.success());
        std::fs::read(out).unwrap()
    }

    /// On a Mac, make a real AVIF (with sips) and convert it to JPEG.
    #[tokio::test]
    async fn sips_turns_avif_into_jpeg() {
        let converter = PhotoConverter::system();
        let PhotoConverter::Sips(sips) = &converter else {
            return; // not a Mac: nothing to try
        };
        let avif = sample_as(sips, "avif");
        assert_eq!(detect(&avif), Ok(Upload::Convert(Foreign::Avif)));
        let jpeg = converter.to_jpeg(Foreign::Avif, &avif).await.unwrap();
        let image = super::super::process::decode(&jpeg).unwrap();
        assert_eq!((image.width(), image.height()), (64, 48));
    }
}
