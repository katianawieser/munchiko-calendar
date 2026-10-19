//! Photo files: recognising uploads, converting them, and keeping the files on disk.
//!
//! Design notes:
//! - The type of an upload is decided from the file's first bytes (its "magic
//!   number"), not from its name or the type the browser claims. Both of those
//!   are just labels the sender chooses; the bytes are what a browser will
//!   actually try to display.
//! - Only formats every browser can show are stored ([`ImageType`]). Others are
//!   converted on upload: BMP and TIFF to PNG by [`process`] (pure Rust), HEIC to
//!   JPEG by [`heic`] (macOS's `sips` tool, when it's there).
//! - Each photo also gets a small JPEG thumbnail for list pages ([`process::thumbnail`]).
//! - Files are named `<photo id>.<extension>` (and `<photo id>.thumb.jpg`) in one
//!   folder. The name the user's file had is never used for the path, so an upload
//!   can't write anywhere else (e.g. a name like `../../munchiko.db`).
//! - Only the record of a photo is in the database ([`crate::models::Photo`]);
//!   this module never touches the database, and the database never touches files.

pub mod heic;
pub mod process;

use std::io;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::models::{Photo, PhotoId};

/// Largest photo accepted. Phone photos are usually 2–6 MB.
pub const MAX_PHOTO_BYTES: usize = 15 * 1024 * 1024;

/// The formats photos are stored in: ones every current browser can display.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ImageType {
    Jpeg,
    Png,
    Webp,
    Gif,
}

/// Formats that are accepted but converted before they're stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Foreign {
    /// Windows bitmaps; huge and not shown by every browser. Converted to PNG.
    Bmp,
    /// Scanner and camera files; Safari only. Converted to PNG.
    Tiff,
    /// iPhone's default; Safari only. Converted to JPEG (see [`heic`]).
    Heic,
}

/// What an uploaded file turned out to be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Upload {
    /// Can be stored as it is.
    Ready(ImageType),
    /// Needs converting first.
    Convert(Foreign),
}

/// Why an upload isn't accepted as a photo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotAPhoto {
    /// Not an image format the app knows.
    Unknown,
    /// Starts like an image but can't be decoded (damaged, or not really one).
    Unreadable,
    /// HEIC, and this computer has no converter.
    HeicUnsupported,
    /// HEIC, but the converter failed.
    ConversionFailed,
}

impl NotAPhoto {
    pub fn message(self) -> &'static str {
        match self {
            NotAPhoto::Unknown => {
                "That file isn't a photo the app can use. \
                 It takes JPEG, PNG, WebP, GIF, HEIC, BMP and TIFF."
            }
            NotAPhoto::Unreadable => {
                "That file looks like an image but couldn't be read. It may be damaged; \
                 try exporting it again."
            }
            NotAPhoto::HeicUnsupported => {
                "That's a HEIC photo (the iPhone default). This computer can't convert HEIC, \
                 so export it as JPEG first (in Photos: File → Export), then upload that."
            }
            NotAPhoto::ConversionFailed => {
                "That HEIC photo couldn't be converted. Export it as JPEG first \
                 (in Photos: File → Export), then upload that."
            }
        }
    }
}

/// Recognise an upload from its first bytes.
pub fn detect(bytes: &[u8]) -> Result<Upload, NotAPhoto> {
    // ISO media files (HEIC, AVIF, MP4...) say "ftyp" at bytes 4..8,
    // followed by a "brand" naming the format.
    let iso_brand = bytes.get(4..12).and_then(|b| b.strip_prefix(b"ftyp"));
    let upload = if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Upload::Ready(ImageType::Jpeg)
    } else if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Upload::Ready(ImageType::Png)
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Upload::Ready(ImageType::Gif)
    } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        Upload::Ready(ImageType::Webp)
    } else if bytes.starts_with(b"BM") && bytes.len() > 26 {
        // "BM" alone is too common a start; a real bitmap header is 26+ bytes.
        Upload::Convert(Foreign::Bmp)
    } else if bytes.starts_with(b"II*\0") || bytes.starts_with(b"MM\0*") {
        // Little-endian ("Intel") or big-endian ("Motorola") TIFF.
        Upload::Convert(Foreign::Tiff)
    } else if matches!(
        iso_brand,
        Some(b"heic" | b"heix" | b"hevc" | b"heim" | b"heis" | b"mif1" | b"msf1")
    ) {
        Upload::Convert(Foreign::Heic)
    } else {
        return Err(NotAPhoto::Unknown);
    };
    Ok(upload)
}

impl ImageType {
    const ALL: [ImageType; 4] = [
        ImageType::Jpeg,
        ImageType::Png,
        ImageType::Webp,
        ImageType::Gif,
    ];

    /// The `Content-Type` to serve it with (also what the database stores).
    pub fn mime(self) -> &'static str {
        match self {
            ImageType::Jpeg => "image/jpeg",
            ImageType::Png => "image/png",
            ImageType::Webp => "image/webp",
            ImageType::Gif => "image/gif",
        }
    }

    pub fn from_mime(mime: &str) -> Option<ImageType> {
        Self::ALL.into_iter().find(|t| t.mime() == mime)
    }

    pub fn extension(self) -> &'static str {
        match self {
            ImageType::Jpeg => "jpg",
            ImageType::Png => "png",
            ImageType::Webp => "webp",
            ImageType::Gif => "gif",
        }
    }
}

/// The folder the photo files live in.
#[derive(Debug, Clone)]
pub struct PhotoStore {
    dir: PathBuf,
}

impl PhotoStore {
    /// Nothing is created until the first photo is saved.
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    fn path(&self, id: PhotoId, image_type: ImageType) -> PathBuf {
        self.dir
            .join(format!("{}.{}", id.0, image_type.extension()))
    }

    /// Thumbnails are always JPEG (see [`process::thumbnail`]).
    fn thumbnail_path(&self, id: PhotoId) -> PathBuf {
        self.dir.join(format!("{}.thumb.jpg", id.0))
    }

    pub async fn save(&self, photo: &Photo, bytes: &[u8]) -> io::Result<()> {
        tokio::fs::create_dir_all(&self.dir).await?;
        tokio::fs::write(self.path(photo.id, photo.image_type), bytes).await
    }

    pub async fn read(&self, photo: &Photo) -> io::Result<Vec<u8>> {
        tokio::fs::read(self.path(photo.id, photo.image_type)).await
    }

    pub async fn save_thumbnail(&self, photo: &Photo, jpeg: &[u8]) -> io::Result<()> {
        tokio::fs::create_dir_all(&self.dir).await?;
        tokio::fs::write(self.thumbnail_path(photo.id), jpeg).await
    }

    /// `NotFound` for photos saved before thumbnails existed.
    pub async fn read_thumbnail(&self, photo: &Photo) -> io::Result<Vec<u8>> {
        tokio::fs::read(self.thumbnail_path(photo.id)).await
    }

    /// Delete the photo and its thumbnail. Already gone counts as success.
    pub async fn remove(&self, photo: &Photo) -> io::Result<()> {
        for path in [
            self.path(photo.id, photo.image_type),
            self.thumbnail_path(photo.id),
        ] {
            match tokio::fs::remove_file(path).await {
                Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
                _ => {}
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{FabricId, PhotoOwner};

    #[test]
    fn recognises_browser_formats_by_their_first_bytes() {
        let ready = |t| Ok(Upload::Ready(t));
        assert_eq!(detect(b"\xFF\xD8\xFF\xE0rest"), ready(ImageType::Jpeg));
        assert_eq!(detect(b"\x89PNG\r\n\x1a\nrest"), ready(ImageType::Png));
        assert_eq!(detect(b"GIF89a..."), ready(ImageType::Gif));
        assert_eq!(detect(b"RIFF\0\0\0\0WEBPVP8 "), ready(ImageType::Webp));
    }

    #[test]
    fn recognises_formats_that_need_converting() {
        let convert = |f| Ok(Upload::Convert(f));
        assert_eq!(
            detect(b"\0\0\0\x18ftypheic\0\0\0\0"),
            convert(Foreign::Heic)
        );
        assert_eq!(
            detect(b"\0\0\0\x18ftypmif1\0\0\0\0"),
            convert(Foreign::Heic)
        );
        assert_eq!(detect(b"II*\0rest"), convert(Foreign::Tiff));
        assert_eq!(detect(b"MM\0*rest"), convert(Foreign::Tiff));
        let mut bmp = b"BM".to_vec();
        bmp.resize(54, 0);
        assert_eq!(detect(&bmp), convert(Foreign::Bmp));
    }

    #[test]
    fn refuses_anything_else() {
        // A text file renamed to .jpg is still a text file.
        assert_eq!(detect(b"hello"), Err(NotAPhoto::Unknown));
        assert_eq!(detect(b"BMW owner's manual"), Err(NotAPhoto::Unknown));
        assert_eq!(detect(b"<svg onload=..."), Err(NotAPhoto::Unknown));
        assert_eq!(detect(b""), Err(NotAPhoto::Unknown));
        // RIFF alone could be a WAV sound file; ftyp alone could be a video.
        assert_eq!(detect(b"RIFF\0\0\0\0WAVE"), Err(NotAPhoto::Unknown));
        assert_eq!(
            detect(b"\0\0\0\x18ftypmp42\0\0\0\0"),
            Err(NotAPhoto::Unknown)
        );
    }

    #[test]
    fn mime_types_round_trip() {
        for t in ImageType::ALL {
            assert_eq!(ImageType::from_mime(t.mime()), Some(t));
        }
        assert_eq!(ImageType::from_mime("image/svg+xml"), None);
    }

    #[tokio::test]
    async fn files_are_saved_read_and_removed_by_id() {
        let dir = tempfile::tempdir().unwrap();
        // A folder that doesn't exist yet is created on the first save.
        let store = PhotoStore::new(dir.path().join("photos"));
        let photo = Photo {
            id: PhotoId(7),
            owner: PhotoOwner::Fabric(FabricId(1)),
            image_type: ImageType::Png,
        };

        assert_eq!(
            store.read_thumbnail(&photo).await.unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
        store.save(&photo, b"png bytes").await.unwrap();
        store.save_thumbnail(&photo, b"jpeg bytes").await.unwrap();
        assert!(dir.path().join("photos/7.png").exists());
        assert!(dir.path().join("photos/7.thumb.jpg").exists());
        assert_eq!(store.read(&photo).await.unwrap(), b"png bytes");
        assert_eq!(store.read_thumbnail(&photo).await.unwrap(), b"jpeg bytes");

        store.remove(&photo).await.unwrap();
        assert!(!dir.path().join("photos/7.png").exists());
        assert!(!dir.path().join("photos/7.thumb.jpg").exists());
        store.remove(&photo).await.unwrap(); // already gone is fine
    }
}
