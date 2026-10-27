//! Photo pages: uploading, showing and deleting photos of fabrics,
//! accessories and post plans.
//!
//! A browser sends files as a `multipart/form-data` form. Photos arrive two
//! ways: with the fabric/accessory form itself ([`FormWithPhotos`], saved by its
//! Save button), or on their own (a post's photos, [`upload_for_post`]).
//!
//! Every file is checked, converted if needed and given a thumbnail before any
//! is saved ([`prepare_all`]), so an upload is all-or-nothing. Saving is two
//! steps ([`save_all`]): record the photo in the database (which assigns its id),
//! then write the files named after that id. If writing fails, the record is
//! removed again so the two never disagree.

use axum::Form;
use axum::body::Bytes;
use axum::extract::multipart::MultipartError;
use axum::extract::{FromRequest, Multipart, Path, Request, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Redirect, Response};

use super::forms::FormData;
use super::{AppError, AppState};
use crate::models::{AccessoryId, Color, FabricId, Photo, PhotoId, PhotoOwner, PostPlanId};
use crate::palette;
use crate::photos::{self, Foreign, ImageType, MAX_PHOTO_BYTES, NotAPhoto, Upload, process};

/// One upload can carry a few photos at once.
pub const MAX_UPLOAD_BYTES: usize = 4 * MAX_PHOTO_BYTES;

/// The page a photo is shown on, and where to go back to.
pub fn owner_href(owner: PhotoOwner) -> String {
    match owner {
        PhotoOwner::Fabric(id) => format!("/fabrics/{}", id.0),
        PhotoOwner::Accessory(id) => format!("/accessories/{}", id.0),
        PhotoOwner::PostPlan(id) => format!("/posts/{}", id.0),
    }
}

pub fn photo_src(photo: &Photo) -> String {
    format!("/photos/{}", photo.id.0)
}

pub fn thumbnail_src(photo: &Photo) -> String {
    format!("/photos/{}/thumb", photo.id.0)
}

// ---------------------------------------------------------------------------
// What the pages show (templates/photos_section.html)
// ---------------------------------------------------------------------------

pub struct PhotoView {
    /// The full-size photo (opened on click).
    pub src: String,
    pub thumb_src: String,
    pub delete_action: String,
}

/// The photos box on an edit page.
pub struct PhotoSection {
    /// A separate upload form (post pages). Fabric and accessory pages take
    /// photos in their main form instead (`templates/photo_input.html`).
    pub upload_action: Option<String>,
    pub photos: Vec<PhotoView>,
    /// Describes the pictures for screen readers ("Photo of Orange velvet").
    pub alt: String,
}

impl PhotoSection {
    /// For a form that hasn't been saved yet: no photos so far.
    pub fn unsaved() -> Self {
        Self {
            upload_action: None,
            photos: Vec::new(),
            alt: String::new(),
        }
    }

    /// The saved photos, with their delete buttons. New ones are added
    /// through the page's main form.
    pub fn gallery(owner: PhotoOwner, name: &str, photos: &[Photo]) -> Self {
        Self {
            upload_action: None,
            ..Self::new(owner, name, photos)
        }
    }

    /// The saved photos plus an upload form of their own.
    pub fn new(owner: PhotoOwner, name: &str, photos: &[Photo]) -> Self {
        Self {
            upload_action: Some(format!("{}/photos", owner_href(owner))),
            photos: photos
                .iter()
                .map(|p| PhotoView {
                    src: photo_src(p),
                    thumb_src: thumbnail_src(p),
                    delete_action: format!("/photos/{}/delete", p.id.0),
                })
                .collect(),
            alt: format!("Photo of {name}"),
        }
    }

    pub async fn load(state: &AppState, owner: PhotoOwner, name: &str) -> Result<Self, AppError> {
        Ok(Self::new(owner, name, &state.db.list_photos(owner).await?))
    }
}

/// The first photo of each owner, for thumbnails on list pages.
pub async fn thumbnails(
    state: &AppState,
) -> Result<std::collections::HashMap<PhotoOwner, String>, AppError> {
    let mut first = std::collections::HashMap::new();
    for photo in state.db.list_all_photos().await? {
        // Oldest first, so keep the first one seen for each owner.
        first
            .entry(photo.owner)
            .or_insert_with(|| thumbnail_src(&photo));
    }
    Ok(first)
}

// ---------------------------------------------------------------------------
// Upload
// ---------------------------------------------------------------------------

pub async fn upload_for_fabric(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    multipart: Multipart,
) -> Result<Response, AppError> {
    let fabric = state.db.get_fabric(FabricId(id)).await?;
    upload(&state, PhotoOwner::Fabric(fabric.id), multipart).await
}

pub async fn upload_for_accessory(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    multipart: Multipart,
) -> Result<Response, AppError> {
    let accessory = state.db.get_accessory(AccessoryId(id)).await?;
    upload(&state, PhotoOwner::Accessory(accessory.id), multipart).await
}

pub async fn upload_for_post(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    multipart: Multipart,
) -> Result<Response, AppError> {
    let plan = state.db.get_post_plan(PostPlanId(id)).await?;
    upload(&state, PhotoOwner::PostPlan(plan.id), multipart).await
}

async fn upload(
    state: &AppState,
    owner: PhotoOwner,
    multipart: Multipart,
) -> Result<Response, AppError> {
    let back = owner_href(owner);
    let photos = read_photos(multipart, &back).await?;
    if photos.is_empty() {
        return Err(rejected(
            &back,
            StatusCode::UNPROCESSABLE_ENTITY,
            "Choose a photo to upload first.",
        ));
    }
    let files = prepare_all(state, photos)
        .await
        .map_err(|problem| rejected(&back, problem.status, &problem.message))?;
    save_all(state, owner, files).await?;
    Ok(Redirect::to(&back).into_response())
}

fn rejected(back: &str, status: StatusCode, message: &str) -> AppError {
    AppError::PhotoRejected {
        status,
        message: message.to_string(),
        back: back.to_string(),
    }
}

fn too_big_message() -> String {
    format!(
        "Photos can be up to {} MB each (and {} MB per upload). \
         Try a smaller export of the photo.",
        MAX_PHOTO_BYTES / 1024 / 1024,
        MAX_UPLOAD_BYTES / 1024 / 1024
    )
}

/// A multipart body that couldn't be read: too big, or cut off.
fn unreadable(e: MultipartError, back: &str) -> AppError {
    if e.status() == StatusCode::PAYLOAD_TOO_LARGE {
        rejected(back, StatusCode::PAYLOAD_TOO_LARGE, &too_big_message())
    } else {
        rejected(
            back,
            e.status(),
            "The upload didn't arrive in one piece. Try again.",
        )
    }
}

/// The files from every "photo" field (an empty file box sends an empty one).
async fn read_photos(mut multipart: Multipart, back: &str) -> Result<Vec<Bytes>, AppError> {
    let mut photos = Vec::new();
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| unreadable(e, back))?
    {
        if field.name() == Some("photo") {
            let bytes = field.bytes().await.map_err(|e| unreadable(e, back))?;
            if !bytes.is_empty() {
                photos.push(bytes);
            }
        }
    }
    Ok(photos)
}

/// Why a photo was turned down: a message for the page, and the status code
/// for an upload of its own (413 too large, 422 not a usable photo).
pub struct PhotoProblem {
    pub status: StatusCode,
    pub message: String,
}

/// Check, convert and thumbnail every photo; the first problem stops it.
pub async fn prepare_all(
    state: &AppState,
    photos: Vec<Bytes>,
) -> Result<Vec<Prepared>, PhotoProblem> {
    let mut files = Vec::new();
    for bytes in photos {
        if bytes.len() > MAX_PHOTO_BYTES {
            return Err(PhotoProblem {
                status: StatusCode::PAYLOAD_TOO_LARGE,
                message: too_big_message(),
            });
        }
        let prepared = prepare(state, bytes).await.map_err(|why| PhotoProblem {
            status: StatusCode::UNPROCESSABLE_ENTITY,
            message: why.message().to_string(),
        })?;
        files.push(prepared);
    }
    Ok(files)
}

/// Save prepared photos for `owner`: the record first (it assigns the id),
/// then the files.
pub async fn save_all(
    state: &AppState,
    owner: PhotoOwner,
    files: Vec<Prepared>,
) -> Result<(), AppError> {
    for file in files {
        let photo = state.db.add_photo(owner, file.image_type).await?;
        let saved = async {
            state.photos.save(&photo, &file.bytes).await?;
            state.photos.save_thumbnail(&photo, &file.thumbnail).await
        };
        if let Err(e) = saved.await {
            state.photos.remove(&photo).await.ok();
            let _ = state.db.delete_photo(photo.id).await;
            return Err(AppError::Internal(format!(
                "saving photo {}: {e}",
                photo.id.0
            )));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Forms that carry photos (fabric and accessory forms)
// ---------------------------------------------------------------------------

/// A submitted form plus any photos chosen in it.
///
/// Accepts both `multipart/form-data` (what a browser sends for a form with a
/// file box) and a plain form, so handlers don't care which arrived.
pub struct FormWithPhotos {
    pub form: FormData,
    pub photos: Vec<Bytes>,
}

impl<S: Send + Sync> FromRequest<S> for FormWithPhotos {
    type Rejection = AppError;

    async fn from_request(request: Request, state: &S) -> Result<Self, Self::Rejection> {
        // Where to send someone back to if the upload itself is unusable:
        // the edit page they came from, or the "new" page.
        let path = request.uri().path().to_string();
        let back = if path.ends_with(|c: char| c.is_ascii_digit()) {
            path
        } else {
            format!("{path}/new")
        };
        let multipart = request
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.starts_with("multipart/form-data"));

        if !multipart {
            let Form(pairs) = Form::<Vec<(String, String)>>::from_request(request, state)
                .await
                .map_err(|_| AppError::BadRequest)?;
            return Ok(Self {
                form: FormData::new(pairs),
                photos: Vec::new(),
            });
        }

        let mut multipart = Multipart::from_request(request, state)
            .await
            .map_err(|_| AppError::BadRequest)?;
        let (mut pairs, mut photos) = (Vec::new(), Vec::new());
        while let Some(field) = multipart
            .next_field()
            .await
            .map_err(|e| unreadable(e, &back))?
        {
            let name = field.name().unwrap_or_default().to_string();
            if name == "photo" {
                let bytes = field.bytes().await.map_err(|e| unreadable(e, &back))?;
                if !bytes.is_empty() {
                    photos.push(bytes);
                }
            } else {
                let value = field.text().await.map_err(|e| unreadable(e, &back))?;
                pairs.push((name, value));
            }
        }
        Ok(Self {
            form: FormData::new(pairs),
            photos,
        })
    }
}

/// Combine a form's own result with its photos' result, so mistakes in both
/// are reported together. A browser can't put chosen files back into a file
/// box, so after any mistake the photos have to be chosen again; say so.
pub fn with_photos<T>(
    input: Result<T, Vec<String>>,
    photos: Result<Vec<Prepared>, PhotoProblem>,
    photos_were_chosen: bool,
) -> Result<(T, Vec<Prepared>), Vec<String>> {
    match (input, photos) {
        (Ok(input), Ok(photos)) => Ok((input, photos)),
        (input, photos) => {
            let mut errors = input.err().unwrap_or_default();
            if let Err(problem) = photos {
                errors.push(problem.message);
            }
            if photos_were_chosen {
                errors.push(
                    "Nothing was saved yet, so choose your photos again before saving.".to_string(),
                );
            }
            Err(errors)
        }
    }
}

/// A photo that has passed every check and is ready to save.
pub struct Prepared {
    image_type: ImageType,
    bytes: Vec<u8>,
    thumbnail: Vec<u8>,
}

/// Recognise the upload, convert it if browsers can't show it, and make its
/// thumbnail. Decoding the whole image for the thumbnail also proves it's a
/// real, undamaged image and not just a file that starts like one.
async fn prepare(state: &AppState, upload: Bytes) -> Result<Prepared, NotAPhoto> {
    let (image_type, bytes) = match photos::detect(&upload)? {
        Upload::Ready(image_type) => (image_type, upload.to_vec()),
        Upload::Convert(Foreign::Heic) => (ImageType::Jpeg, state.heic.to_jpeg(&upload).await?),
        Upload::Convert(Foreign::Bmp | Foreign::Tiff) => (
            ImageType::Png,
            on_cpu(move || process::to_png(&upload)).await?,
        ),
    };
    let (bytes, thumbnail) = on_cpu(move || {
        let thumbnail = process::thumbnail(&bytes)?;
        Ok((bytes, thumbnail))
    })
    .await?;
    Ok(Prepared {
        image_type,
        bytes,
        thumbnail,
    })
}

/// Run slow image work on a thread meant for it (see `photos::process`).
async fn on_cpu<T: Send + 'static>(
    work: impl FnOnce() -> image::ImageResult<T> + Send + 'static,
) -> Result<T, NotAPhoto> {
    match tokio::task::spawn_blocking(work).await {
        Ok(Ok(result)) => Ok(result),
        // The image couldn't be decoded, or decoding it crashed the thread.
        Ok(Err(_)) | Err(_) => Err(NotAPhoto::Unreadable),
    }
}

// ---------------------------------------------------------------------------
// Show and delete
// ---------------------------------------------------------------------------

fn image_response(mime: &'static str, bytes: Vec<u8>) -> Response {
    (
        [
            (header::CONTENT_TYPE, mime),
            // Never let the browser guess a different type from the contents.
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
            // A photo's id is never reused, so its image never changes.
            (
                header::CACHE_CONTROL,
                "private, max-age=31536000, immutable",
            ),
        ],
        bytes,
    )
        .into_response()
}

async fn read_original(state: &AppState, photo: &Photo) -> Result<Vec<u8>, AppError> {
    match state.photos.read(photo).await {
        Ok(bytes) => Ok(bytes),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(AppError::NotFound),
        Err(e) => Err(AppError::Internal(format!(
            "reading photo {}: {e}",
            photo.id.0
        ))),
    }
}

pub async fn serve(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Response, AppError> {
    let photo = state.db.get_photo(PhotoId(id)).await?;
    let bytes = read_original(&state, &photo).await?;
    Ok(image_response(photo.image_type.mime(), bytes))
}

pub async fn serve_thumbnail(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Response, AppError> {
    let photo = state.db.get_photo(PhotoId(id)).await?;
    match thumbnail_of(&state, &photo).await? {
        Some(jpeg) => Ok(image_response("image/jpeg", jpeg)),
        // Can't be shrunk: show the full photo rather than nothing.
        None => Ok(image_response(
            photo.image_type.mime(),
            read_original(&state, &photo).await?,
        )),
    }
}

/// The photo's thumbnail. Photos uploaded before thumbnails existed get one
/// made now, which is kept for next time. `None` if one can't be made.
async fn thumbnail_of(state: &AppState, photo: &Photo) -> Result<Option<Vec<u8>>, AppError> {
    match state.photos.read_thumbnail(photo).await {
        Ok(jpeg) => return Ok(Some(jpeg)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            return Err(AppError::Internal(format!(
                "reading thumbnail {}: {e}",
                photo.id.0
            )));
        }
    }
    let original = read_original(state, photo).await?;
    let Ok(jpeg) = on_cpu(move || process::thumbnail(&original)).await else {
        return Ok(None);
    };
    if let Err(e) = state.photos.save_thumbnail(photo, &jpeg).await {
        eprintln!("couldn't save the thumbnail for photo {}: {e}", photo.id.0);
    }
    Ok(Some(jpeg))
}

// ---------------------------------------------------------------------------
// Color ideas from photos (M6d)
// ---------------------------------------------------------------------------

/// A main color of an item's photos, offered as a one-click color row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColorIdea {
    pub name: String,
    pub hex: String,
}

/// How many colors to offer.
const MAX_COLOR_IDEAS: usize = 6;

/// The main colors across `photos` (read from their thumbnails, which is
/// quick), minus colors the item already has a swatch for under that name.
/// Never fails the page: if a photo can't be read, it's skipped.
pub async fn color_ideas(state: &AppState, photos: &[Photo], existing: &[Color]) -> Vec<ColorIdea> {
    let mut thumbnails = Vec::new();
    for photo in photos {
        match thumbnail_of(state, photo).await {
            Ok(Some(jpeg)) => thumbnails.push(jpeg),
            Ok(None) => {}
            Err(e) => eprintln!("color ideas: skipping photo {}: {e:?}", photo.id.0),
        }
    }
    if thumbnails.is_empty() {
        return Vec::new();
    }
    let swatches = tokio::task::spawn_blocking(move || {
        // All the photos' pixels together, as if they were one picture.
        let images: Vec<_> = thumbnails
            .iter()
            .filter_map(|jpeg| process::decode(jpeg).ok())
            .map(|image| image.to_rgb8())
            .collect();
        palette::main_colors(
            images.iter().flat_map(|image| image.pixels().map(|p| p.0)),
            MAX_COLOR_IDEAS,
        )
    })
    .await
    .unwrap_or_default();
    new_ideas(&swatches, existing)
}

/// Leave out names that already have a swatch: nothing to add. A name with
/// no swatch yet stays, since clicking it fills in the exact shade.
fn new_ideas(swatches: &[palette::Swatch], existing: &[Color]) -> Vec<ColorIdea> {
    swatches
        .iter()
        .filter(|s| !existing.iter().any(|c| c.name == s.name && c.hex.is_some()))
        .map(|s| ColorIdea {
            name: s.name.to_string(),
            hex: s.hex(),
        })
        .collect()
}

pub async fn delete(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Redirect, AppError> {
    let photo = state.db.get_photo(PhotoId(id)).await?;
    state.db.delete_photo(photo.id).await?;
    remove_files(&state, std::slice::from_ref(&photo)).await;
    Ok(Redirect::to(&owner_href(photo.owner)))
}

/// Delete photo files whose records are already gone (after deleting a photo,
/// or a fabric/accessory/post whose photo records went with it). A file that
/// can't be removed is only logged: the page the user asked for has worked.
pub async fn remove_files(state: &AppState, photos: &[Photo]) {
    for photo in photos {
        if let Err(e) = state.photos.remove(photo).await {
            eprintln!("couldn't remove the file for photo {}: {e}", photo.id.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ideas_skip_colors_that_already_have_a_swatch() {
        let swatch = |name: &'static str, rgb| palette::Swatch {
            rgb,
            share: 0.5,
            name,
        };
        let found = [
            swatch("teal", [0, 128, 128]),
            swatch("pink", [245, 150, 185]),
        ];
        let existing = [Color::with_hex("teal", "#008080"), Color::named("pink")];
        assert_eq!(
            new_ideas(&found, &existing),
            vec![ColorIdea {
                name: "pink".into(),
                hex: "#f596b9".into()
            }],
            "pink has no swatch yet, so it's still offered"
        );
    }
}
