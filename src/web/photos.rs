//! Photo pages: uploading, showing and deleting photos of fabrics,
//! accessories and post plans.
//!
//! An upload is a `multipart/form-data` form (the only way a browser can send
//! a file). Every file in it is checked before any is saved, so an upload is
//! all-or-nothing. Saving is two steps: record the photo in the database (which
//! assigns its id), then write the file named after that id. If writing the file
//! fails, the record is removed again so the two never disagree.

use axum::extract::multipart::MultipartError;
use axum::extract::{Multipart, Path, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Redirect, Response};

use super::{AppError, AppState};
use crate::models::{AccessoryId, FabricId, Photo, PhotoId, PhotoOwner, PostPlanId};
use crate::photos::{ImageType, MAX_PHOTO_BYTES};

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

// ---------------------------------------------------------------------------
// What the pages show (templates/photos_section.html)
// ---------------------------------------------------------------------------

pub struct PhotoView {
    pub src: String,
    pub delete_action: String,
}

/// The photos box on an edit page.
pub struct PhotoSection {
    /// `None` on a "new ..." form: there's nothing to attach photos to yet.
    pub upload_action: Option<String>,
    pub photos: Vec<PhotoView>,
    /// Describes the pictures for screen readers ("Photo of Orange velvet").
    pub alt: String,
}

impl PhotoSection {
    /// For a form that hasn't been saved yet.
    pub fn unsaved() -> Self {
        Self {
            upload_action: None,
            photos: Vec::new(),
            alt: String::new(),
        }
    }

    pub fn new(owner: PhotoOwner, name: &str, photos: &[Photo]) -> Self {
        Self {
            upload_action: Some(format!("{}/photos", owner_href(owner))),
            photos: photos
                .iter()
                .map(|p| PhotoView {
                    src: photo_src(p),
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
            .or_insert_with(|| photo_src(&photo));
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
    mut multipart: Multipart,
) -> Result<Response, AppError> {
    let back = owner_href(owner);
    let reject = |status, message: &str| AppError::PhotoRejected {
        status,
        message: message.to_string(),
        back: back.clone(),
    };
    let too_big = || {
        reject(
            StatusCode::PAYLOAD_TOO_LARGE,
            &format!(
                "Photos can be up to {} MB each (and {} MB per upload). \
                 Try a smaller export of the photo.",
                MAX_PHOTO_BYTES / 1024 / 1024,
                MAX_UPLOAD_BYTES / 1024 / 1024
            ),
        )
    };
    let unreadable = |e: MultipartError| {
        if e.status() == StatusCode::PAYLOAD_TOO_LARGE {
            too_big()
        } else {
            reject(
                e.status(),
                "The upload didn't arrive in one piece. Try again.",
            )
        }
    };

    // 1. Read and check every file before saving anything.
    let mut files = Vec::new();
    while let Some(field) = multipart.next_field().await.map_err(unreadable)? {
        if field.name() != Some("photo") {
            continue;
        }
        let bytes = field.bytes().await.map_err(unreadable)?;
        if bytes.is_empty() {
            continue; // the file box was left empty
        }
        if bytes.len() > MAX_PHOTO_BYTES {
            return Err(too_big());
        }
        let image_type = ImageType::detect(&bytes)
            .map_err(|why| reject(StatusCode::UNPROCESSABLE_ENTITY, why.message()))?;
        files.push((image_type, bytes));
    }
    if files.is_empty() {
        return Err(reject(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Choose a photo to upload first.",
        ));
    }

    // 2. Save them: the record first (it assigns the id), then the file.
    for (image_type, bytes) in files {
        let photo = state.db.add_photo(owner, image_type).await?;
        if let Err(e) = state.photos.save(&photo, &bytes).await {
            let _ = state.db.delete_photo(photo.id).await;
            return Err(AppError::Internal(format!(
                "saving photo {}: {e}",
                photo.id.0
            )));
        }
    }
    Ok(Redirect::to(&back).into_response())
}

// ---------------------------------------------------------------------------
// Show and delete
// ---------------------------------------------------------------------------

pub async fn serve(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Response, AppError> {
    let photo = state.db.get_photo(PhotoId(id)).await?;
    let bytes = match state.photos.read(&photo).await {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(AppError::NotFound),
        Err(e) => return Err(AppError::Internal(format!("reading photo {id}: {e}"))),
    };
    Ok((
        [
            (header::CONTENT_TYPE, photo.image_type.mime()),
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
        .into_response())
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
