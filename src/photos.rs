//! Photo files: recognising image types and keeping the files on disk.
//!
//! Design notes:
//! - The type of an upload is decided from the file's first bytes (its "magic
//!   number"), not from its name or the type the browser claims. Both of those
//!   are just labels the sender chooses; the bytes are what a browser will
//!   actually try to display.
//! - Files are named `<photo id>.<extension>` in one folder. The name the user's
//!   file had is never used for the path, so an upload can't write anywhere
//!   else (e.g. a name like `../../munchiko.db`).
//! - Only the record of a photo is in the database ([`crate::models::Photo`]);
//!   this module never touches the database, and the database never touches files.

use std::io;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::models::{Photo, PhotoId};

/// Largest photo accepted. Phone photos are usually 2–6 MB.
pub const MAX_PHOTO_BYTES: usize = 15 * 1024 * 1024;

/// The image formats every current browser can display.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ImageType {
    Jpeg,
    Png,
    Webp,
    Gif,
}

/// Why an upload isn't accepted as a photo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotAPhoto {
    /// iPhone's default format. Safari shows it but other browsers don't.
    Heic,
    Unknown,
}

impl NotAPhoto {
    pub fn message(self) -> &'static str {
        match self {
            NotAPhoto::Heic => {
                "That's a HEIC photo (the iPhone default), which most browsers can't show. \
                 Export it as JPEG first (in Photos: File → Export), then upload that."
            }
            NotAPhoto::Unknown => "That file isn't a JPEG, PNG, WebP or GIF image.",
        }
    }
}

impl ImageType {
    const ALL: [ImageType; 4] = [
        ImageType::Jpeg,
        ImageType::Png,
        ImageType::Webp,
        ImageType::Gif,
    ];

    /// Recognise an image from its first bytes.
    pub fn detect(bytes: &[u8]) -> Result<ImageType, NotAPhoto> {
        // ISO media files (HEIC, AVIF, MP4...) say "ftyp" at bytes 4..8,
        // followed by a "brand" naming the format.
        let iso_brand = bytes.get(4..12).and_then(|b| b.strip_prefix(b"ftyp"));
        if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
            Ok(ImageType::Jpeg)
        } else if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
            Ok(ImageType::Png)
        } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
            Ok(ImageType::Gif)
        } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
            Ok(ImageType::Webp)
        } else if matches!(
            iso_brand,
            Some(b"heic" | b"heix" | b"hevc" | b"heim" | b"heis" | b"mif1" | b"msf1")
        ) {
            Err(NotAPhoto::Heic)
        } else {
            Err(NotAPhoto::Unknown)
        }
    }

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

    pub async fn save(&self, photo: &Photo, bytes: &[u8]) -> io::Result<()> {
        tokio::fs::create_dir_all(&self.dir).await?;
        tokio::fs::write(self.path(photo.id, photo.image_type), bytes).await
    }

    pub async fn read(&self, photo: &Photo) -> io::Result<Vec<u8>> {
        tokio::fs::read(self.path(photo.id, photo.image_type)).await
    }

    /// Delete the file. Already gone counts as success.
    pub async fn remove(&self, photo: &Photo) -> io::Result<()> {
        match tokio::fs::remove_file(self.path(photo.id, photo.image_type)).await {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{FabricId, PhotoOwner};

    #[test]
    fn recognises_images_by_their_first_bytes() {
        assert_eq!(
            ImageType::detect(b"\xFF\xD8\xFF\xE0rest"),
            Ok(ImageType::Jpeg)
        );
        assert_eq!(
            ImageType::detect(b"\x89PNG\r\n\x1a\nrest"),
            Ok(ImageType::Png)
        );
        assert_eq!(ImageType::detect(b"GIF89a..."), Ok(ImageType::Gif));
        assert_eq!(
            ImageType::detect(b"RIFF\0\0\0\0WEBPVP8 "),
            Ok(ImageType::Webp)
        );
    }

    #[test]
    fn refuses_heic_with_its_own_message_and_anything_else() {
        assert_eq!(
            ImageType::detect(b"\0\0\0\x18ftypheic\0\0\0\0"),
            Err(NotAPhoto::Heic)
        );
        // A text file renamed to .jpg is still a text file.
        assert_eq!(ImageType::detect(b"hello"), Err(NotAPhoto::Unknown));
        assert_eq!(
            ImageType::detect(b"<svg onload=..."),
            Err(NotAPhoto::Unknown)
        );
        assert_eq!(ImageType::detect(b""), Err(NotAPhoto::Unknown));
        // RIFF alone could be a WAV sound file.
        assert_eq!(
            ImageType::detect(b"RIFF\0\0\0\0WAVE"),
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

        store.save(&photo, b"png bytes").await.unwrap();
        assert!(dir.path().join("photos/7.png").exists());
        assert_eq!(store.read(&photo).await.unwrap(), b"png bytes");

        store.remove(&photo).await.unwrap();
        assert!(!dir.path().join("photos/7.png").exists());
        store.remove(&photo).await.unwrap(); // already gone is fine
    }
}
