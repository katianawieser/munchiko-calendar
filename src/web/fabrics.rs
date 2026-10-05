//! Fabric pages: the stash and shopping list, plus the add/edit form.
//!
//! Every form follows the same pattern ("Post/Redirect/Get"): the form POSTs,
//! and on success the server answers with a redirect to a normal page. That
//! way refreshing the page afterwards doesn't submit the form a second time.
//! On a mistake the form is shown again with the user's input and a message.

use askama::Template;
use axum::Form;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};

use super::forms::{Choice, FormData, join_list, split_list};
use super::{AppError, AppState, render};
use crate::db::{DbError, NewFabric};
use crate::models::{Accessory, Fabric, FabricId};

// ---------------------------------------------------------------------------
// /fabrics
// ---------------------------------------------------------------------------

struct FabricRow {
    href: String,
    stock_action: String,
    name: String,
    colors: Vec<String>,
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

pub async fn list(State(state): State<AppState>) -> Result<Response, AppError> {
    let fabrics = state.db.list_fabrics().await?;
    let accessories = state.db.list_accessories().await?;
    let name_of = |id: &FabricId| {
        fabrics
            .iter()
            .find(|f| f.id == *id)
            .map(|f| f.name.clone())
            .unwrap_or_default()
    };

    let (mut in_stock, mut shopping) = (Vec::new(), Vec::new());
    for fabric in &fabrics {
        let row = FabricRow {
            href: format!("/fabrics/{}", fabric.id.0),
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
    Ok(render(FabricsPage { sections })?.into_response())
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
    colors: String,
    keywords: String,
    in_stock: bool,
    pair_choices: Vec<Choice>,
    used_by: String,
    errors: Vec<String>,
}

impl FabricFormPage {
    /// The form for `editing` (or a new fabric), with every other fabric
    /// offered as a pairing.
    fn new(editing: Option<&Fabric>, all: &[Fabric], accessories: &[Accessory]) -> Self {
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
                colors: join_list(&f.colors),
                keywords: join_list(&f.keywords),
                in_stock: f.in_stock,
                pair_choices,
                used_by: used_by(f.id, accessories).join(", "),
                errors: Vec::new(),
            },
            None => Self {
                heading: "New fabric".into(),
                action: "/fabrics".into(),
                delete_action: None,
                name: String::new(),
                description: String::new(),
                colors: String::new(),
                keywords: String::new(),
                in_stock: true,
                pair_choices,
                used_by: String::new(),
                errors: Vec::new(),
            },
        }
    }

    /// Put the submitted values back into the form, so a mistake doesn't
    /// wipe out what was typed.
    fn refill(mut self, form: &FormData, errors: Vec<String>) -> Self {
        self.name = form.text("name");
        self.description = form.text("description");
        self.colors = form.text("colors");
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
            colors: split_list(&form.text("colors")),
            keywords: split_list(&form.text("keywords")),
            in_stock: form.checked("in_stock"),
        },
        pairings,
    })
}

/// Show the form again with a status code saying what went wrong.
fn form_response(status: StatusCode, page: FabricFormPage) -> Result<Response, AppError> {
    Ok((status, render(page)?).into_response())
}

pub async fn new_form(State(state): State<AppState>) -> Result<Response, AppError> {
    let fabrics = state.db.list_fabrics().await?;
    form_response(StatusCode::OK, FabricFormPage::new(None, &fabrics, &[]))
}

pub async fn create(
    State(state): State<AppState>,
    Form(pairs): Form<Vec<(String, String)>>,
) -> Result<Response, AppError> {
    let form = FormData::new(pairs);
    let input = match read_form(&form) {
        Ok(input) => input,
        Err(errors) => {
            let fabrics = state.db.list_fabrics().await?;
            let page = FabricFormPage::new(None, &fabrics, &[]).refill(&form, errors);
            // 422: the request arrived fine, but its content needs fixing.
            return form_response(StatusCode::UNPROCESSABLE_ENTITY, page);
        }
    };
    let id = state.db.create_fabric(&input.fabric).await?;
    state.db.set_fabric_pairings(id, &input.pairings).await?;
    Ok(Redirect::to("/fabrics").into_response())
}

pub async fn edit_form(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Response, AppError> {
    let fabric = state.db.get_fabric(FabricId(id)).await?;
    let fabrics = state.db.list_fabrics().await?;
    let accessories = state.db.list_accessories().await?;
    form_response(
        StatusCode::OK,
        FabricFormPage::new(Some(&fabric), &fabrics, &accessories),
    )
}

pub async fn update(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Form(pairs): Form<Vec<(String, String)>>,
) -> Result<Response, AppError> {
    let form = FormData::new(pairs);
    let fabric = state.db.get_fabric(FabricId(id)).await?;
    let input = match read_form(&form) {
        Ok(input) => input,
        Err(errors) => {
            let fabrics = state.db.list_fabrics().await?;
            let accessories = state.db.list_accessories().await?;
            let page =
                FabricFormPage::new(Some(&fabric), &fabrics, &accessories).refill(&form, errors);
            return form_response(StatusCode::UNPROCESSABLE_ENTITY, page);
        }
    };
    state.db.update_fabric(fabric.id, &input.fabric).await?;
    state
        .db
        .set_fabric_pairings(fabric.id, &input.pairings)
        .await?;
    Ok(Redirect::to("/fabrics").into_response())
}

pub async fn delete(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Response, AppError> {
    match state.db.delete_fabric(FabricId(id)).await {
        Ok(()) => Ok(Redirect::to("/fabrics").into_response()),
        Err(DbError::InUse { .. }) => {
            // Explain which accessories are in the way, on the edit page.
            let fabric = state.db.get_fabric(FabricId(id)).await?;
            let fabrics = state.db.list_fabrics().await?;
            let accessories = state.db.list_accessories().await?;
            let mut page = FabricFormPage::new(Some(&fabric), &fabrics, &accessories);
            page.errors = vec![format!(
                "Can't delete {}: it's used by {}. Change or delete those first.",
                fabric.name, page.used_by
            )];
            // 409 Conflict: the request clashes with the current data.
            form_response(StatusCode::CONFLICT, page)
        }
        Err(e) => Err(e.into()),
    }
}
