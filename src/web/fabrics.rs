//! Fabric pages: the stash and shopping list, plus the add/edit form.
//!
//! Every form follows the same pattern ("Post/Redirect/Get"): the form POSTs,
//! and on success the server answers with a redirect to a normal page. That
//! way refreshing the page afterwards doesn't submit the form a second time.
//! On a mistake the form is shown again with the user's input and a message.

use std::collections::HashMap;

use askama::Template;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};

use super::blockers::{self, Blockers};
use super::browse::{Browse, Controls};
use super::colors;
use super::forms::{
    Choice, ColorRow, FormData, color_rows, color_rows_from_form, join_list, read_colors,
    split_list,
};
use super::photos::{self, ColorIdea, FormWithPhotos, PhotoSection};
use super::{AppError, AppState, load_usage, render};
use crate::db::{DbError, NewFabric};
use crate::models::{Accessory, Color, Fabric, FabricId, PhotoOwner};

// ---------------------------------------------------------------------------
// /fabrics
// ---------------------------------------------------------------------------

struct FabricRow {
    href: String,
    /// The fabric's first photo, if it has one.
    thumb: Option<String>,
    stock_action: String,
    name: String,
    colors: Vec<Color>,
    keywords: String,
    pairs_with: String,
    used_by: String,
}

/// "In the stash" or "Shopping list".
struct Section {
    heading: &'static str,
    /// The button that moves a fabric to the other section.
    move_label: &'static str,
    rows: Vec<FabricRow>,
}

#[derive(Template)]
#[template(path = "fabrics.html")]
struct FabricsPage {
    sections: Vec<Section>,
    grid: bool,
    controls: Controls,
    /// Shown when a filter is on, since some fabrics may be hidden.
    filtered: bool,
}

/// Names of the accessories that use `fabric`, for "used by" and for
/// explaining why it can't be deleted.
fn used_by(fabric: FabricId, accessories: &[Accessory]) -> Vec<String> {
    accessories
        .iter()
        .filter(|a| a.fabrics.iter().any(|u| u.fabric_id == fabric))
        .map(|a| a.name.clone())
        .collect()
}

pub async fn list(
    State(state): State<AppState>,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    let browse = Browse::read(&query, &headers);
    let fabrics = state.db.list_fabrics().await?;
    let accessories = state.db.list_accessories().await?;
    let thumbnails = photos::thumbnails(&state).await?;
    let name_of = |id: &FabricId| {
        fabrics
            .iter()
            .find(|f| f.id == *id)
            .map(|f| f.name.clone())
            .unwrap_or_default()
    };

    let (mut in_stock, mut shopping) = (Vec::new(), Vec::new());
    for fabric in fabrics
        .iter()
        .filter(|f| browse.keeps(&f.colors, &f.keywords))
    {
        let row = FabricRow {
            href: format!("/fabrics/{}", fabric.id.0),
            thumb: thumbnails.get(&PhotoOwner::Fabric(fabric.id)).cloned(),
            stock_action: format!("/fabrics/{}/stock", fabric.id.0),
            name: fabric.name.clone(),
            colors: fabric.colors.clone(),
            keywords: join_list(&fabric.keywords),
            pairs_with: fabric
                .pairings
                .iter()
                .map(name_of)
                .collect::<Vec<_>>()
                .join(", "),
            used_by: used_by(fabric.id, &accessories).join(", "),
        };
        if fabric.in_stock {
            in_stock.push(row);
        } else {
            shopping.push(row);
        }
    }
    browse.sort(&mut in_stock, |row| &row.colors);
    browse.sort(&mut shopping, |row| &row.colors);
    let sections = vec![
        Section {
            heading: "In the stash",
            move_label: "Used up",
            rows: in_stock,
        },
        Section {
            heading: "Shopping list",
            move_label: "Bought it",
            rows: shopping,
        },
    ];
    let mut response = render(FabricsPage {
        sections,
        grid: browse.is_grid(),
        controls: browse.controls(
            "/fabrics",
            vec![],
            &[],
            fabrics.iter().flat_map(|f| &f.keywords),
        ),
        filtered: browse.tone.is_some(),
    })?
    .into_response();
    if let Some((name, value)) = browse.cookie() {
        response.headers_mut().insert(name, value);
    }
    Ok(response)
}

/// Flip a fabric between the stash and the shopping list.
pub async fn toggle_stock(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Redirect, AppError> {
    let fabric = state.db.get_fabric(FabricId(id)).await?;
    state
        .db
        .set_fabric_in_stock(fabric.id, !fabric.in_stock)
        .await?;
    Ok(Redirect::to("/fabrics"))
}

// ---------------------------------------------------------------------------
// The add / edit form
// ---------------------------------------------------------------------------

#[derive(Template)]
#[template(path = "fabric_form.html")]
struct FabricFormPage {
    heading: String,
    /// Where the form posts to.
    action: String,
    /// Set when editing, for the delete button.
    delete_action: Option<String>,
    name: String,
    description: String,
    colors: Vec<ColorRow>,
    keywords: String,
    in_stock: bool,
    pair_choices: Vec<Choice>,
    /// What's in the way of deleting it, shown instead of the delete button.
    blocked: Blockers,
    photos: PhotoSection,
    /// Main colors of its photos, offered beside the color rows.
    color_ideas: Vec<ColorIdea>,
    /// Shades already picked elsewhere in the stash, to reuse.
    your_shades: Vec<ColorIdea>,
    errors: Vec<String>,
}

impl FabricFormPage {
    /// The form for `editing` (or a new fabric), with every other fabric
    /// offered as a pairing.
    fn new(editing: Option<&Fabric>, all: &[Fabric]) -> Self {
        let pair_choices = all
            .iter()
            .filter(|f| Some(f.id) != editing.map(|e| e.id))
            .map(|f| {
                let ticked = editing.is_some_and(|e| e.pairings.contains(&f.id));
                Choice::new(f.id.0, f.name.clone(), ticked)
            })
            .collect();
        match editing {
            Some(f) => Self {
                heading: format!("Edit {}", f.name),
                action: format!("/fabrics/{}", f.id.0),
                delete_action: Some(format!("/fabrics/{}/delete", f.id.0)),
                name: f.name.clone(),
                description: f.description.clone(),
                colors: color_rows(&f.colors),
                keywords: join_list(&f.keywords),
                in_stock: f.in_stock,
                pair_choices,
                blocked: Blockers::default(),
                photos: PhotoSection::unsaved(),
                color_ideas: Vec::new(),
                your_shades: Vec::new(),
                errors: Vec::new(),
            },
            None => Self {
                heading: "New fabric".into(),
                action: "/fabrics".into(),
                delete_action: None,
                name: String::new(),
                description: String::new(),
                colors: color_rows(&[]),
                keywords: String::new(),
                in_stock: true,
                pair_choices,
                blocked: Blockers::default(),
                photos: PhotoSection::unsaved(),
                color_ideas: Vec::new(),
                your_shades: Vec::new(),
                errors: Vec::new(),
            },
        }
    }

    /// Put the submitted values back into the form, so a mistake doesn't
    /// wipe out what was typed.
    fn refill(mut self, form: &FormData, errors: Vec<String>) -> Self {
        self.name = form.text("name");
        self.description = form.text("description");
        self.colors = color_rows_from_form(form);
        self.keywords = form.text("keywords");
        self.in_stock = form.checked("in_stock");
        let ticked: Vec<&str> = form.all("pairs").collect();
        for choice in &mut self.pair_choices {
            choice.selected = ticked.contains(&choice.value.as_str());
        }
        self.errors = errors;
        self
    }
}

/// What a valid form submission contains.
struct FabricInput {
    fabric: NewFabric,
    pairings: Vec<FabricId>,
}

fn read_form(form: &FormData) -> Result<FabricInput, Vec<String>> {
    let mut errors = Vec::new();
    let name = form.text("name");
    if name.is_empty() {
        errors.push("Give the fabric a name.".to_string());
    }
    let colors = read_colors(form).unwrap_or_else(|color_errors| {
        errors.extend(color_errors);
        Vec::new()
    });
    let mut pairings = Vec::new();
    for value in form.all("pairs") {
        match value.parse() {
            Ok(id) => pairings.push(FabricId(id)),
            Err(_) => errors.push(format!("{value:?} isn't a fabric.")),
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }
    Ok(FabricInput {
        fabric: NewFabric {
            name,
            description: form.text("description"),
            colors,
            keywords: split_list(&form.text("keywords")),
            in_stock: form.checked("in_stock"),
        },
        pairings,
    })
}

/// The edit form for `fabric`, including what would block deleting it.
async fn edit_page(state: &AppState, fabric: &Fabric) -> Result<FabricFormPage, AppError> {
    let fabrics = state.db.list_fabrics().await?;
    let mut page = FabricFormPage::new(Some(fabric), &fabrics);
    page.blocked = blockers::for_fabric(fabric.id, &load_usage(state).await?);
    let photos = state.db.list_photos(PhotoOwner::Fabric(fabric.id)).await?;
    page.photos = PhotoSection::gallery(PhotoOwner::Fabric(fabric.id), &fabric.name, &photos);
    page.color_ideas = photos::color_ideas(state, &photos, &fabric.colors).await;
    Ok(page)
}

/// Show the form again with a status code saying what went wrong.
/// Show the form with `status`. Every form page goes through here, so this
/// is where "Your shades" (shades already picked elsewhere) are added.
async fn form_response(
    state: &AppState,
    status: StatusCode,
    mut page: FabricFormPage,
) -> Result<Response, AppError> {
    let fabrics = state.db.list_fabrics().await?;
    let accessories = state.db.list_accessories().await?;
    page.your_shades = colors::your_shades(&fabrics, &accessories);
    Ok((status, render(page)?).into_response())
}

pub async fn new_form(State(state): State<AppState>) -> Result<Response, AppError> {
    let fabrics = state.db.list_fabrics().await?;
    form_response(&state, StatusCode::OK, FabricFormPage::new(None, &fabrics)).await
}

pub async fn create(
    State(state): State<AppState>,
    FormWithPhotos { form, photos }: FormWithPhotos,
) -> Result<Response, AppError> {
    let chosen = !photos.is_empty();
    let checked = photos::with_photos(
        read_form(&form),
        photos::prepare_all(&state, photos).await,
        chosen,
    );
    let (input, files) = match checked {
        Ok(ok) => ok,
        Err(errors) => {
            let fabrics = state.db.list_fabrics().await?;
            let page = FabricFormPage::new(None, &fabrics).refill(&form, errors);
            // 422: the request arrived fine, but its content needs fixing.
            return form_response(&state, StatusCode::UNPROCESSABLE_ENTITY, page).await;
        }
    };
    let id = state.db.create_fabric(&input.fabric).await?;
    state.db.set_fabric_pairings(id, &input.pairings).await?;
    photos::save_all(&state, PhotoOwner::Fabric(id), files).await?;
    Ok(Redirect::to("/fabrics").into_response())
}

pub async fn edit_form(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Response, AppError> {
    let fabric = state.db.get_fabric(FabricId(id)).await?;
    form_response(&state, StatusCode::OK, edit_page(&state, &fabric).await?).await
}

pub async fn update(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    FormWithPhotos { form, photos }: FormWithPhotos,
) -> Result<Response, AppError> {
    let fabric = state.db.get_fabric(FabricId(id)).await?;
    let chosen = !photos.is_empty();
    let checked = photos::with_photos(
        read_form(&form),
        photos::prepare_all(&state, photos).await,
        chosen,
    );
    let (input, files) = match checked {
        Ok(ok) => ok,
        Err(errors) => {
            let page = edit_page(&state, &fabric).await?.refill(&form, errors);
            return form_response(&state, StatusCode::UNPROCESSABLE_ENTITY, page).await;
        }
    };
    state.db.update_fabric(fabric.id, &input.fabric).await?;
    state
        .db
        .set_fabric_pairings(fabric.id, &input.pairings)
        .await?;
    photos::save_all(&state, PhotoOwner::Fabric(fabric.id), files).await?;
    Ok(Redirect::to("/fabrics").into_response())
}

pub async fn delete(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Response, AppError> {
    // Note the photos first: deleting the fabric deletes their records.
    let photo_files = state
        .db
        .list_photos(PhotoOwner::Fabric(FabricId(id)))
        .await?;
    match state.db.delete_fabric(FabricId(id)).await {
        Ok(()) => {
            photos::remove_files(&state, &photo_files).await;
            Ok(Redirect::to("/fabrics").into_response())
        }
        Err(DbError::InUse { .. }) => {
            // Show the edit page with the chain of what's in the way.
            let fabric = state.db.get_fabric(FabricId(id)).await?;
            let mut page = edit_page(&state, &fabric).await?;
            page.errors = vec![format!(
                "{} is still in use, so it can't be deleted yet.",
                fabric.name
            )];
            // 409 Conflict: the request clashes with the current data.
            form_response(&state, StatusCode::CONFLICT, page).await
        }
        Err(e) => Err(e.into()),
    }
}
