//! Outfit pages: pieces worn together, like Glinda's crown, dress and wand.
//!
//! An outfit has a name, notes, photos of the whole look, and its pieces,
//! which stay ordinary accessories. Its status isn't stored: it's worked out
//! from the pieces (ready only when every piece is), so it can never be out
//! of date. Same Post/Redirect/Get forms as the fabric and accessory pages.

use askama::Template;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};

use super::blockers::{Blockers, Link};
use super::forms::{Choice, FormData, KINDS, STATUSES, label_of};
use super::photos::{self, FormWithPhotos, PhotoSection};
use super::{AppError, AppState, render};
use crate::db::NewOutfit;
use crate::models::{
    Accessory, AccessoryId, AccessoryStatus, Color, Fabric, Outfit, OutfitId, PhotoOwner,
    without_repeats,
};

/// The badge class for a status, matching the accessory list's colors.
fn status_class(status: AccessoryStatus) -> &'static str {
    match status {
        AccessoryStatus::Idea => "warn",
        AccessoryStatus::InProgress => "active",
        AccessoryStatus::Ready => "good",
    }
}

/// "Ready", or "1 of 3 ready" while some pieces are still to do.
fn status_text(outfit: &Outfit, accessories: &[Accessory]) -> String {
    match outfit.ready_count(accessories) {
        (_, 0) => "No pieces yet".to_string(),
        (ready, total) if ready == total => "Ready".to_string(),
        (ready, total) => format!("{ready} of {total} ready"),
    }
}

/// Every piece's colors (each piece's own order), without repeats.
fn outfit_colors(pieces: &[&Accessory], fabrics: &[Fabric]) -> Vec<Color> {
    let all: Vec<Color> = pieces
        .iter()
        .flat_map(|a| a.effective_colors(fabrics))
        .collect();
    without_repeats(&all)
}

fn accessory_link(a: &Accessory) -> Link {
    Link {
        label: a.name.clone(),
        href: format!("/accessories/{}", a.id.0),
    }
}

/// Links to the outfits `accessory` is part of, for its edit page.
pub fn outfits_with(accessory: AccessoryId, outfits: &[Outfit]) -> Vec<Link> {
    outfits
        .iter()
        .filter(|o| o.pieces.contains(&accessory))
        .map(|o| Link {
            label: o.name.clone(),
            href: format!("/outfits/{}", o.id.0),
        })
        .collect()
}

// ---------------------------------------------------------------------------
// /outfits
// ---------------------------------------------------------------------------

struct OutfitRow {
    href: String,
    thumb: Option<String>,
    name: String,
    pieces: Vec<Link>,
    colors: Vec<Color>,
    status: String,
    status_class: &'static str,
}

#[derive(Template)]
#[template(path = "outfits.html")]
struct OutfitsPage {
    rows: Vec<OutfitRow>,
}

pub async fn list(State(state): State<AppState>) -> Result<Response, AppError> {
    let outfits = state.db.list_outfits().await?;
    let accessories = state.db.list_accessories().await?;
    let fabrics = state.db.list_fabrics().await?;
    let thumbnails = photos::thumbnails(&state).await?;
    let rows = outfits
        .iter()
        .map(|o| {
            let pieces = o.piece_items(&accessories);
            OutfitRow {
                href: format!("/outfits/{}", o.id.0),
                thumb: thumbnails.get(&PhotoOwner::Outfit(o.id)).cloned(),
                name: o.name.clone(),
                colors: outfit_colors(&pieces, &fabrics),
                pieces: pieces.into_iter().map(accessory_link).collect(),
                status: status_text(o, &accessories),
                status_class: status_class(o.status(&accessories)),
            }
        })
        .collect();
    Ok(render(OutfitsPage { rows })?.into_response())
}

// ---------------------------------------------------------------------------
// The add / edit form
// ---------------------------------------------------------------------------

/// One kind's checkboxes ("Hats": Glinda crown, Witch hat...).
struct PieceGroup {
    label: &'static str,
    choices: Vec<Choice>,
}

impl PieceGroup {
    /// Groups with a ticked piece start open; the rest stay folded up so
    /// the form isn't one long list.
    fn open(&self) -> bool {
        self.choices.iter().any(|c| c.selected)
    }
}

#[derive(Template)]
#[template(path = "outfit_form.html")]
struct OutfitFormPage {
    heading: String,
    action: String,
    cancel_href: String,
    delete_action: Option<String>,
    name: String,
    description: String,
    /// Shown on the edit page above the form.
    status: Option<(String, &'static str)>,
    groups: Vec<PieceGroup>,
    /// Never blocked (posts copy the pieces), but the shared delete partial
    /// expects it.
    blocked: Blockers,
    photos: PhotoSection,
    errors: Vec<String>,
}

/// Every accessory as a checkbox, grouped by kind in the usual kind order,
/// with the outfit's pieces ticked.
fn piece_groups(accessories: &[Accessory], ticked: &[AccessoryId]) -> Vec<PieceGroup> {
    KINDS
        .iter()
        .map(|&(kind, _, label)| PieceGroup {
            label,
            choices: accessories
                .iter()
                .filter(|a| a.kind == kind)
                .map(|a| {
                    let count = if a.quantity > 1 {
                        format!(", ×{}", a.quantity)
                    } else {
                        String::new()
                    };
                    let text = format!(
                        "{} ({}{count})",
                        a.name,
                        label_of(&STATUSES, a.status).to_lowercase()
                    );
                    Choice::new(a.id.0, text, ticked.contains(&a.id))
                })
                .collect(),
        })
        .filter(|group| !group.choices.is_empty())
        .collect()
}

impl OutfitFormPage {
    fn new(editing: Option<&Outfit>, accessories: &[Accessory]) -> Self {
        Self {
            heading: editing.map_or("New outfit".into(), |o| format!("Edit {}", o.name)),
            action: editing.map_or("/outfits".into(), |o| format!("/outfits/{}", o.id.0)),
            cancel_href: "/outfits".into(),
            delete_action: editing.map(|o| format!("/outfits/{}/delete", o.id.0)),
            name: editing.map(|o| o.name.clone()).unwrap_or_default(),
            description: editing.map(|o| o.description.clone()).unwrap_or_default(),
            status: editing.map(|o| {
                (
                    status_text(o, accessories),
                    status_class(o.status(accessories)),
                )
            }),
            groups: piece_groups(accessories, editing.map_or(&[], |o| &o.pieces)),
            blocked: Blockers::default(),
            photos: PhotoSection::unsaved(),
            errors: Vec::new(),
        }
    }

    /// Put the submitted values back, so a mistake doesn't lose them.
    fn refill(mut self, form: &FormData, errors: Vec<String>) -> Self {
        self.name = form.text("name");
        self.description = form.text("description");
        let ticked: Vec<&str> = form.all("pieces").collect();
        for group in &mut self.groups {
            for choice in &mut group.choices {
                choice.selected = ticked.contains(&choice.value.as_str());
            }
        }
        self.errors = errors;
        self
    }
}

fn read_form(form: &FormData, accessories: &[Accessory]) -> Result<NewOutfit, Vec<String>> {
    let mut errors = Vec::new();
    let name = form.text("name");
    if name.is_empty() {
        errors.push("Give the outfit a name.".to_string());
    }
    let mut pieces = Vec::new();
    for value in form.all("pieces") {
        match value
            .parse::<i64>()
            .ok()
            .filter(|id| accessories.iter().any(|a| a.id.0 == *id))
        {
            Some(id) => pieces.push(AccessoryId(id)),
            None => errors.push("One of the pieces isn't in the inventory any more.".to_string()),
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }
    Ok(NewOutfit {
        name,
        description: form.text("description"),
        pieces,
    })
}

async fn edit_page(state: &AppState, outfit: &Outfit) -> Result<OutfitFormPage, AppError> {
    let accessories = state.db.list_accessories().await?;
    let mut page = OutfitFormPage::new(Some(outfit), &accessories);
    let owner = PhotoOwner::Outfit(outfit.id);
    let photos = state.db.list_photos(owner).await?;
    page.photos = PhotoSection::gallery(owner, &outfit.name, &photos);
    Ok(page)
}

fn form_response(status: StatusCode, page: OutfitFormPage) -> Result<Response, AppError> {
    Ok((status, render(page)?).into_response())
}

pub async fn new_form(State(state): State<AppState>) -> Result<Response, AppError> {
    let accessories = state.db.list_accessories().await?;
    form_response(StatusCode::OK, OutfitFormPage::new(None, &accessories))
}

pub async fn create(
    State(state): State<AppState>,
    FormWithPhotos { form, photos }: FormWithPhotos,
) -> Result<Response, AppError> {
    let accessories = state.db.list_accessories().await?;
    let chosen = !photos.is_empty();
    let checked = photos::with_photos(
        read_form(&form, &accessories),
        photos::prepare_all(&state, photos).await,
        chosen,
    );
    let (input, files) = match checked {
        Ok(ok) => ok,
        Err(errors) => {
            let page = OutfitFormPage::new(None, &accessories).refill(&form, errors);
            return form_response(StatusCode::UNPROCESSABLE_ENTITY, page);
        }
    };
    let id = state.db.create_outfit(&input).await?;
    photos::save_all(&state, PhotoOwner::Outfit(id), files).await?;
    Ok(Redirect::to(&format!("/outfits/{}", id.0)).into_response())
}

pub async fn edit_form(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Response, AppError> {
    let outfit = state.db.get_outfit(OutfitId(id)).await?;
    form_response(StatusCode::OK, edit_page(&state, &outfit).await?)
}

pub async fn update(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    FormWithPhotos { form, photos }: FormWithPhotos,
) -> Result<Response, AppError> {
    let outfit = state.db.get_outfit(OutfitId(id)).await?;
    let accessories = state.db.list_accessories().await?;
    let chosen = !photos.is_empty();
    let checked = photos::with_photos(
        read_form(&form, &accessories),
        photos::prepare_all(&state, photos).await,
        chosen,
    );
    let (input, files) = match checked {
        Ok(ok) => ok,
        Err(errors) => {
            let page = edit_page(&state, &outfit).await?.refill(&form, errors);
            return form_response(StatusCode::UNPROCESSABLE_ENTITY, page);
        }
    };
    state.db.update_outfit(outfit.id, &input).await?;
    photos::save_all(&state, PhotoOwner::Outfit(outfit.id), files).await?;
    Ok(Redirect::to("/outfits").into_response())
}

/// Always allowed: posts copied the pieces when the outfit was chosen, so
/// nothing points at an outfit except its own photos.
pub async fn delete(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Response, AppError> {
    // Note the photos first: deleting the outfit deletes their records.
    let photo_files = state
        .db
        .list_photos(PhotoOwner::Outfit(OutfitId(id)))
        .await?;
    state.db.delete_outfit(OutfitId(id)).await?;
    photos::remove_files(&state, &photo_files).await;
    Ok(Redirect::to("/outfits").into_response())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{AccessoryKind, Source};

    fn piece(id: i64, name: &str, kind: AccessoryKind, status: AccessoryStatus) -> Accessory {
        Accessory {
            id: AccessoryId(id),
            kind,
            name: name.into(),
            description: String::new(),
            colors: vec![Color::from(name)],
            colors_from_fabrics: false,
            tags: vec![],
            fabrics: vec![],
            source: Source::Handmade,
            status,
            pair_id: None,
            make_days: None,
            quantity: 1,
        }
    }

    #[test]
    fn pieces_are_grouped_by_kind_and_status_is_explained() {
        use AccessoryKind::*;
        use AccessoryStatus::*;
        let stock = [
            piece(1, "Wand", Prop, Ready),
            piece(2, "Crown", Hat, InProgress),
            piece(3, "Red bow", Bow, Ready),
        ];
        let groups = piece_groups(&stock, &[AccessoryId(2)]);
        let labels: Vec<&str> = groups.iter().map(|g| g.label).collect();
        assert_eq!(
            labels,
            ["Bow", "Hat", "Prop"],
            "kinds with nothing are left out"
        );
        assert_eq!(groups[1].choices[0].label, "Crown (in progress)");
        assert!(groups[1].choices[0].selected);
        assert!(!groups[2].choices[0].selected);

        let glinda = Outfit {
            id: OutfitId(1),
            name: "Glinda".into(),
            description: String::new(),
            pieces: vec![AccessoryId(2), AccessoryId(1)],
        };
        assert_eq!(status_text(&glinda, &stock), "1 of 2 ready");
        assert_eq!(status_class(glinda.status(&stock)), "active");
        let empty = Outfit {
            pieces: vec![],
            ..glinda.clone()
        };
        assert_eq!(status_text(&empty, &stock), "No pieces yet");

        let pieces = glinda.piece_items(&stock);
        let names: Vec<String> = outfit_colors(&pieces, &[])
            .into_iter()
            .map(|c| c.name)
            .collect();
        assert_eq!(names, ["Crown", "Wand"]);
        assert_eq!(
            outfits_with(AccessoryId(1), &[glinda])[0].href,
            "/outfits/1"
        );
    }
}
