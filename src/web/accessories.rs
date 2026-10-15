//! Accessory pages: the bow / scarf / costume inventory, quick status
//! buttons, and the add/edit form. Same Post/Redirect/Get pattern as the
//! fabric pages (see `fabrics.rs`).

use std::collections::HashMap;

use askama::Template;
use axum::Form;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};

use super::blockers::{self, Blockers};
use super::forms::{
    Choice, ColorRow, FormData, KINDS, NotANumber, ROLES, STATUSES, choices, color_rows,
    color_rows_from_form, label_of, parse_choice, read_colors,
};
use super::photos::{self, PhotoSection};
use super::{AppError, AppState, load_usage, render};
use crate::db::{DbError, NewAccessory};
use crate::models::{
    Accessory, AccessoryId, AccessoryKind, AccessoryStatus, Color, Fabric, FabricId, FabricUse,
    PhotoOwner, Source,
};

fn describe_source(source: &Source) -> String {
    match source {
        Source::Handmade => "Handmade".into(),
        Source::Bought { brand: Some(brand) } => format!("Bought ({brand})"),
        Source::Bought { brand: None } => "Bought".into(),
    }
}

/// "Orange velvet (main), Black satin (middle)"
fn describe_fabrics(accessory: &Accessory, fabrics: &[Fabric]) -> String {
    accessory
        .fabrics
        .iter()
        .map(|u| {
            let name = fabrics
                .iter()
                .find(|f| f.id == u.fabric_id)
                .map_or("?", |f| f.name.as_str());
            let role = label_of(&ROLES, u.role)
                .trim_end_matches(" fabric")
                .to_lowercase();
            format!("{name} ({role})")
        })
        .collect::<Vec<_>>()
        .join(", ")
}

// ---------------------------------------------------------------------------
// /accessories
// ---------------------------------------------------------------------------

/// The one-click next step for an accessory, if there is one.
struct NextStep {
    action: String,
    status: &'static str,
    label: &'static str,
}

struct AccessoryRow {
    href: String,
    /// The accessory's first photo, if it has one.
    thumb: Option<String>,
    name: String,
    kind: &'static str,
    status: &'static str,
    status_class: &'static str,
    next_step: Option<NextStep>,
    fabrics: String,
    partner: String,
    source: String,
    colors: Vec<Color>,
}

#[derive(Template)]
#[template(path = "accessories.html")]
struct AccessoriesPage {
    rows: Vec<AccessoryRow>,
    kind_filter: Vec<Choice>,
    status_filter: Vec<Choice>,
    /// This page's address including filters, so status buttons come back here.
    back: String,
}

/// Build a filter dropdown: an "All" choice plus one per enum value.
fn filter_choices<T: Copy + PartialEq>(
    table: &[(T, &str, &str)],
    all_label: &str,
    current: Option<T>,
) -> Vec<Choice> {
    let mut list = vec![Choice::new("", all_label, current.is_none())];
    for (item, value, label) in table {
        list.push(Choice::new(value, *label, current == Some(*item)));
    }
    list
}

pub async fn list(
    State(state): State<AppState>,
    Query(query): Query<HashMap<String, String>>,
) -> Result<Response, AppError> {
    let kind = query.get("kind").and_then(|v| parse_choice(&KINDS, v));
    let status = query.get("status").and_then(|v| parse_choice(&STATUSES, v));

    let accessories = state.db.list_accessories().await?;
    let fabrics = state.db.list_fabrics().await?;
    let thumbnails = photos::thumbnails(&state).await?;

    let rows = accessories
        .iter()
        .filter(|a| kind.is_none_or(|k| a.kind == k))
        .filter(|a| status.is_none_or(|s| a.status == s))
        .map(|a| {
            let next_step = match a.status {
                AccessoryStatus::Idea => Some(("in_progress", "Start")),
                AccessoryStatus::InProgress => Some(("ready", "Mark ready")),
                AccessoryStatus::Ready => None,
            }
            .map(|(status, label)| NextStep {
                action: format!("/accessories/{}/status", a.id.0),
                status,
                label,
            });
            AccessoryRow {
                href: format!("/accessories/{}", a.id.0),
                thumb: thumbnails.get(&PhotoOwner::Accessory(a.id)).cloned(),
                name: a.name.clone(),
                kind: label_of(&KINDS, a.kind),
                status: label_of(&STATUSES, a.status),
                status_class: match a.status {
                    AccessoryStatus::Idea => "warn",
                    AccessoryStatus::InProgress => "active",
                    AccessoryStatus::Ready => "good",
                },
                next_step,
                fabrics: describe_fabrics(a, &fabrics),
                partner: a
                    .pair_id
                    .and_then(|p| accessories.iter().find(|x| x.id == p))
                    .map(|p| p.name.clone())
                    .unwrap_or_default(),
                source: describe_source(&a.source),
                colors: a.effective_colors(&fabrics),
            }
        })
        .collect();

    // Rebuild this page's address so the status buttons can return to it.
    let mut params = Vec::new();
    if let Some(k) = kind {
        params.push(format!(
            "kind={}",
            KINDS.iter().find(|(x, _, _)| *x == k).unwrap().1
        ));
    }
    if let Some(s) = status {
        params.push(format!(
            "status={}",
            STATUSES.iter().find(|(x, _, _)| *x == s).unwrap().1
        ));
    }
    let back = if params.is_empty() {
        "/accessories".to_string()
    } else {
        format!("/accessories?{}", params.join("&"))
    };

    Ok(render(AccessoriesPage {
        rows,
        kind_filter: filter_choices(&KINDS, "All kinds", kind),
        status_filter: filter_choices(&STATUSES, "Any status", status),
        back,
    })?
    .into_response())
}

/// The quick "Start" / "Mark ready" buttons.
pub async fn set_status(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Form(pairs): Form<Vec<(String, String)>>,
) -> Result<Response, AppError> {
    let form = FormData::new(pairs);
    let status = parse_choice(&STATUSES, &form.text("status")).ok_or(AppError::BadRequest)?;
    state
        .db
        .set_accessory_status(AccessoryId(id), status)
        .await?;

    // Only ever redirect within this page, never to an address someone
    // slipped into the form (an "open redirect").
    let back = form.text("back");
    let back = if back.starts_with("/accessories") {
        back
    } else {
        "/accessories".into()
    };
    Ok(Redirect::to(&back).into_response())
}

// ---------------------------------------------------------------------------
// The add / edit form
// ---------------------------------------------------------------------------

/// One `<select>` per fabric role.
struct RoleSelect {
    field: &'static str,
    label: &'static str,
    choices: Vec<Choice>,
}

#[derive(Template)]
#[template(path = "accessory_form.html")]
struct AccessoryFormPage {
    heading: String,
    action: String,
    delete_action: Option<String>,
    kind_choices: Vec<Choice>,
    status_choices: Vec<Choice>,
    name: String,
    description: String,
    colors: Vec<ColorRow>,
    colors_from_fabrics: bool,
    bought: bool,
    brand: String,
    make_days: String,
    fabric_selects: Vec<RoleSelect>,
    partner_choices: Vec<Choice>,
    blocked: Blockers,
    photos: PhotoSection,
    errors: Vec<String>,
}

/// "— none —" followed by every fabric, with `current` selected.
fn fabric_choices(fabrics: &[Fabric], current: Option<FabricId>) -> Vec<Choice> {
    let mut list = vec![Choice::new("", "— none —", current.is_none())];
    for f in fabrics {
        let label = if f.in_stock {
            f.name.clone()
        } else {
            format!("{} (to buy)", f.name)
        };
        list.push(Choice::new(f.id.0, label, Some(f.id) == current));
    }
    list
}

impl AccessoryFormPage {
    fn new(editing: Option<&Accessory>, accessories: &[Accessory], fabrics: &[Fabric]) -> Self {
        let editing_id = editing.map(|a| a.id);
        let fabric_selects = ROLES
            .iter()
            .map(|(role, field, label)| {
                // The form has one fabric per role; show the first one saved.
                let current = editing
                    .and_then(|a| a.fabrics.iter().find(|u| u.role == *role))
                    .map(|u| u.fabric_id);
                RoleSelect {
                    field,
                    label,
                    choices: fabric_choices(fabrics, current),
                }
            })
            .collect();

        let current_partner = editing.and_then(|a| a.pair_id);
        let mut partner_choices = vec![Choice::new("", "— none —", current_partner.is_none())];
        for other in accessories.iter().filter(|a| Some(a.id) != editing_id) {
            let label = format!(
                "{} ({})",
                other.name,
                label_of(&KINDS, other.kind).to_lowercase()
            );
            partner_choices.push(Choice::new(
                other.id.0,
                label,
                Some(other.id) == current_partner,
            ));
        }

        let (kind, status) = editing.map_or((AccessoryKind::Bow, AccessoryStatus::Idea), |a| {
            (a.kind, a.status)
        });
        let (bought, brand) = match editing.map(|a| &a.source) {
            Some(Source::Bought { brand }) => (true, brand.clone().unwrap_or_default()),
            _ => (false, String::new()),
        };

        Self {
            heading: editing.map_or("New accessory".into(), |a| format!("Edit {}", a.name)),
            action: editing.map_or("/accessories".into(), |a| {
                format!("/accessories/{}", a.id.0)
            }),
            delete_action: editing.map(|a| format!("/accessories/{}/delete", a.id.0)),
            kind_choices: choices(&KINDS, kind),
            status_choices: choices(&STATUSES, status),
            name: editing.map(|a| a.name.clone()).unwrap_or_default(),
            description: editing.map(|a| a.description.clone()).unwrap_or_default(),
            colors: color_rows(editing.map_or(&[], |a| &a.colors)),
            // New accessories start with the box ticked: less typing.
            colors_from_fabrics: editing.is_none_or(|a| a.colors_from_fabrics),
            bought,
            brand,
            make_days: editing
                .and_then(|a| a.make_days)
                .map(|d| d.to_string())
                .unwrap_or_default(),
            fabric_selects,
            partner_choices,
            blocked: Blockers::default(),
            photos: PhotoSection::unsaved(),
            errors: Vec::new(),
        }
    }

    /// Put the submitted values back after a mistake.
    fn refill(mut self, form: &FormData, errors: Vec<String>) -> Self {
        let select = |list: &mut Vec<Choice>, value: String| {
            for choice in list.iter_mut() {
                choice.selected = choice.value == value;
            }
        };
        select(&mut self.kind_choices, form.text("kind"));
        select(&mut self.status_choices, form.text("status"));
        select(&mut self.partner_choices, form.text("partner"));
        for role in &mut self.fabric_selects {
            select(&mut role.choices, form.text(role.field));
        }
        self.name = form.text("name");
        self.description = form.text("description");
        self.colors = color_rows_from_form(form);
        self.colors_from_fabrics = form.checked("colors_from_fabrics");
        self.bought = form.text("source") == "bought";
        self.brand = form.text("brand");
        self.make_days = form.text("make_days");
        self.errors = errors;
        self
    }
}

struct AccessoryInput {
    accessory: NewAccessory,
    partner: Option<AccessoryId>,
}

fn read_form(form: &FormData, editing: Option<AccessoryId>) -> Result<AccessoryInput, Vec<String>> {
    let mut errors = Vec::new();

    let name = form.text("name");
    if name.is_empty() {
        errors.push("Give it a name.".to_string());
    }
    let kind = parse_choice(&KINDS, &form.text("kind"));
    if kind.is_none() {
        errors.push("Choose bow, scarf or costume.".to_string());
    }
    let status = parse_choice(&STATUSES, &form.text("status"));
    if status.is_none() {
        errors.push("Choose a status.".to_string());
    }

    let source = if form.text("source") == "bought" {
        let brand = form.text("brand");
        Source::Bought {
            brand: (!brand.is_empty()).then_some(brand),
        }
    } else {
        Source::Handmade
    };

    let make_days = match form.text("make_days").as_str() {
        "" => None,
        text => match text.parse::<u32>() {
            Ok(days) if days > 0 => Some(days),
            _ => {
                errors.push("Days to make must be a whole number of 1 or more.".to_string());
                None
            }
        },
    };

    let colors = read_colors(form).unwrap_or_else(|color_errors| {
        errors.extend(color_errors);
        Vec::new()
    });

    let mut fabrics = Vec::new();
    for (role, field, label) in ROLES {
        match form.optional_id(field) {
            Ok(Some(id)) => fabrics.push(FabricUse {
                fabric_id: FabricId(id),
                role,
            }),
            Ok(None) => {}
            Err(NotANumber) => errors.push(format!("{label} isn't a valid choice.")),
        }
    }

    let partner = match form.optional_id("partner") {
        Ok(p) => p.map(AccessoryId),
        Err(NotANumber) => {
            errors.push("Matching partner isn't a valid choice.".to_string());
            None
        }
    };
    if partner.is_some() && partner == editing {
        errors.push("An accessory can't be its own partner.".to_string());
    }

    match (kind, status) {
        (Some(kind), Some(status)) if errors.is_empty() => Ok(AccessoryInput {
            accessory: NewAccessory {
                kind,
                name,
                description: form.text("description"),
                colors,
                colors_from_fabrics: form.checked("colors_from_fabrics"),
                fabrics,
                source,
                status,
                make_days,
            },
            partner,
        }),
        _ => Err(errors),
    }
}

/// Make the saved partner match the form: pair, re-pair or unpair.
async fn apply_partner(
    state: &AppState,
    id: AccessoryId,
    current: Option<AccessoryId>,
    wanted: Option<AccessoryId>,
) -> Result<(), AppError> {
    match wanted {
        Some(p) if wanted != current => state.db.pair_accessories(id, p).await?,
        None if current.is_some() => state.db.unpair_accessory(id).await?,
        _ => {}
    }
    Ok(())
}

/// The edit form for `accessory`, including what would block deleting it.
async fn edit_page(state: &AppState, accessory: &Accessory) -> Result<AccessoryFormPage, AppError> {
    let usage = load_usage(state).await?;
    let fabrics = state.db.list_fabrics().await?;
    let mut page = AccessoryFormPage::new(Some(accessory), &usage.accessories, &fabrics);
    page.blocked = blockers::for_accessory(accessory.id, &usage);
    page.photos =
        PhotoSection::load(state, PhotoOwner::Accessory(accessory.id), &accessory.name).await?;
    Ok(page)
}

fn form_response(status: StatusCode, page: AccessoryFormPage) -> Result<Response, AppError> {
    Ok((status, render(page)?).into_response())
}

pub async fn new_form(State(state): State<AppState>) -> Result<Response, AppError> {
    let accessories = state.db.list_accessories().await?;
    let fabrics = state.db.list_fabrics().await?;
    form_response(
        StatusCode::OK,
        AccessoryFormPage::new(None, &accessories, &fabrics),
    )
}

pub async fn create(
    State(state): State<AppState>,
    Form(pairs): Form<Vec<(String, String)>>,
) -> Result<Response, AppError> {
    let form = FormData::new(pairs);
    let input = match read_form(&form, None) {
        Ok(input) => input,
        Err(errors) => {
            let accessories = state.db.list_accessories().await?;
            let fabrics = state.db.list_fabrics().await?;
            let page = AccessoryFormPage::new(None, &accessories, &fabrics).refill(&form, errors);
            return form_response(StatusCode::UNPROCESSABLE_ENTITY, page);
        }
    };
    let id = state.db.create_accessory(&input.accessory).await?;
    apply_partner(&state, id, None, input.partner).await?;
    Ok(Redirect::to("/accessories").into_response())
}

pub async fn edit_form(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Response, AppError> {
    let accessory = state.db.get_accessory(AccessoryId(id)).await?;
    form_response(StatusCode::OK, edit_page(&state, &accessory).await?)
}

pub async fn update(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Form(pairs): Form<Vec<(String, String)>>,
) -> Result<Response, AppError> {
    let form = FormData::new(pairs);
    let accessory = state.db.get_accessory(AccessoryId(id)).await?;
    let input = match read_form(&form, Some(accessory.id)) {
        Ok(input) => input,
        Err(errors) => {
            let page = edit_page(&state, &accessory).await?.refill(&form, errors);
            return form_response(StatusCode::UNPROCESSABLE_ENTITY, page);
        }
    };
    state
        .db
        .update_accessory(accessory.id, &input.accessory)
        .await?;
    apply_partner(&state, accessory.id, accessory.pair_id, input.partner).await?;
    Ok(Redirect::to("/accessories").into_response())
}

pub async fn delete(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Response, AppError> {
    // Note the photos first: deleting the accessory deletes their records.
    let photo_files = state
        .db
        .list_photos(PhotoOwner::Accessory(AccessoryId(id)))
        .await?;
    match state.db.delete_accessory(AccessoryId(id)).await {
        Ok(()) => {
            photos::remove_files(&state, &photo_files).await;
            Ok(Redirect::to("/accessories").into_response())
        }
        Err(DbError::InUse { .. }) => {
            let accessory = state.db.get_accessory(AccessoryId(id)).await?;
            let mut page = edit_page(&state, &accessory).await?;
            page.errors = vec![format!(
                "{} is still in use, so it can't be deleted yet.",
                accessory.name
            )];
            form_response(StatusCode::CONFLICT, page)
        }
        Err(e) => Err(e.into()),
    }
}
