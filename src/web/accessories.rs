//! Accessory pages: the bow / scarf / costume inventory, quick status
//! buttons, and the add/edit form. Same Post/Redirect/Get pattern as the
//! fabric pages (see `fabrics.rs`).

use std::collections::HashMap;

use askama::Template;
use axum::Form;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};

use super::blockers::{self, Blockers, Link};
use super::browse::{Browse, Controls, Filter, query_string};
use super::colors;
use super::forms::{
    Choice, ColorRow, FormData, KINDS, ROLES, STATUSES, choices, color_rows, color_rows_from_form,
    join_list, label_of, local_back, parse_choice, read_colors, split_list,
};
use super::outfits;
use super::photos::{self, FormWithPhotos, IdeaGroup, PhotoSection};
use super::{AppError, AppState, load_usage, render};
use crate::db::{DbError, NewAccessory, NewFabric};
use crate::models::{
    Accessory, AccessoryId, AccessoryKind, AccessoryStatus, Color, Fabric, FabricId, FabricRole,
    FabricUse, PhotoOwner, Source, without_repeats,
};
use crate::suggest::{self, PartnerQuery};

fn describe_source(source: &Source) -> String {
    match source {
        Source::Handmade => "Handmade".into(),
        Source::Bought { brand: Some(brand) } => format!("Bought ({brand})"),
        Source::Bought { brand: None } => "Bought".into(),
    }
}

/// One of an accessory's fabrics in the list: its photo if it has one (the
/// name is the tooltip), else its name.
#[derive(Debug, Clone, PartialEq)]
struct FabricLink {
    /// "Orange velvet (main)", for the tooltip and the photo's alt text.
    title: String,
    name: String,
    href: String,
    thumb: Option<String>,
}

/// An accessory's fabrics in role order (main first), with thumbnails from
/// `thumbnails` (fabric photo owner → thumbnail address).
fn fabric_links(
    accessory: &Accessory,
    fabrics: &[Fabric],
    thumbnails: &HashMap<PhotoOwner, String>,
) -> Vec<FabricLink> {
    accessory
        .fabrics
        .iter()
        .filter_map(|u| {
            let fabric = fabrics.iter().find(|f| f.id == u.fabric_id)?;
            let role = label_of(&ROLES, u.role)
                .trim_end_matches(" fabric")
                .to_lowercase();
            Some(FabricLink {
                title: format!("{} ({role})", fabric.name),
                name: fabric.name.clone(),
                href: format!("/fabrics/{}", fabric.id.0),
                thumb: thumbnails.get(&PhotoOwner::Fabric(fabric.id)).cloned(),
            })
        })
        .collect()
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
    /// Shown as "×2" after the name when more than 1.
    quantity: u32,
    kind: &'static str,
    status: &'static str,
    status_class: &'static str,
    next_step: Option<NextStep>,
    fabrics: Vec<FabricLink>,
    partner: String,
    source: String,
    colors: Vec<Color>,
    /// Its own tags and its fabrics' keywords.
    tags: String,
}

#[derive(Template)]
#[template(path = "accessories.html")]
struct AccessoriesPage {
    rows: Vec<AccessoryRow>,
    grid: bool,
    /// "small", "medium" or "large" (the grid's CSS class).
    card_size: &'static str,
    controls: Controls,
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
    headers: HeaderMap,
) -> Result<Response, AppError> {
    let browse = Browse::read(&query, &headers);
    let kind = query.get("kind").and_then(|v| parse_choice(&KINDS, v));
    let status = query.get("status").and_then(|v| parse_choice(&STATUSES, v));

    let accessories = state.db.list_accessories().await?;
    let fabrics = state.db.list_fabrics().await?;
    let thumbnails = photos::thumbnails(&state).await?;

    let mut rows: Vec<AccessoryRow> = accessories
        .iter()
        .filter(|a| kind.is_none_or(|k| a.kind == k))
        .filter(|a| status.is_none_or(|s| a.status == s))
        .map(|a| (a, a.effective_colors(&fabrics), a.all_tags(&fabrics)))
        .filter(|(_, colors, tags)| browse.keeps(colors, tags))
        .map(|(a, colors, tags)| {
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
                quantity: a.quantity,
                kind: label_of(&KINDS, a.kind),
                status: label_of(&STATUSES, a.status),
                status_class: match a.status {
                    AccessoryStatus::Idea => "warn",
                    AccessoryStatus::InProgress => "active",
                    AccessoryStatus::Ready => "good",
                },
                next_step,
                fabrics: fabric_links(a, &fabrics, &thumbnails),
                partner: a
                    .pair_id
                    .and_then(|p| accessories.iter().find(|x| x.id == p))
                    .map(|p| p.name.clone())
                    .unwrap_or_default(),
                source: describe_source(&a.source),
                colors,
                tags: join_list(&tags),
            }
        })
        .collect();
    browse.sort(&mut rows, |row| &row.colors);

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
    let filters = vec![
        Filter {
            name: "kind",
            label: "Kind",
            choices: filter_choices(&KINDS, "All kinds", kind),
        },
        Filter {
            name: "status",
            label: "Status",
            choices: filter_choices(&STATUSES, "Any status", status),
        },
    ];
    // Every tag in use, not only on the accessories shown, so choosing one
    // doesn't empty the dropdown.
    let all_tags: Vec<String> = accessories
        .iter()
        .flat_map(|a| a.all_tags(&fabrics))
        .collect();
    let controls = browse.controls("/accessories", filters, &params, &all_tags);
    params.extend(browse.params());
    let back = format!("/accessories{}", query_string(&params));

    let mut response = render(AccessoriesPage {
        rows,
        grid: browse.is_grid(),
        card_size: browse.size_name(),
        controls,
        back,
    })?
    .into_response();
    for (name, value) in browse.cookies() {
        response.headers_mut().append(name, value);
    }
    Ok(response)
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

    // Back to the list, the calendar or a post: wherever the button was.
    let back = local_back(&form.text("back"), "/accessories");
    Ok(Redirect::to(&back).into_response())
}

// ---------------------------------------------------------------------------
// The add / edit form
// ---------------------------------------------------------------------------

/// One `<select>` per fabric role.
/// One fabric box (main / middle / accent): type a fabric's name, with the
/// stash's names suggested as you type (`<datalist id="fabric-names">`).
struct FabricBox {
    field: &'static str,
    label: &'static str,
    /// The fabric's name, as saved or as typed.
    value: String,
    /// "Need to buy it", for a name that isn't a fabric yet: `buy_main_fabric`.
    buy_field: String,
    buy: bool,
}

/// What's typed in a fabric box.
#[derive(Debug, Clone, PartialEq)]
enum FabricPick {
    None,
    Existing(FabricId),
    /// Not a fabric yet: it's created on save.
    New(String),
}

/// A fabric box's text: empty is no fabric; a fabric's name (ignoring
/// capitals and spaces around it) is that fabric; anything else is a new one.
fn pick_fabric(text: &str, fabrics: &[Fabric]) -> FabricPick {
    let text = text.trim();
    if text.is_empty() {
        return FabricPick::None;
    }
    match fabrics
        .iter()
        .find(|f| f.name.trim().eq_ignore_ascii_case(text))
    {
        Some(f) => FabricPick::Existing(f.id),
        None => FabricPick::New(text.to_string()),
    }
}

/// The matching partner box: an accessory's name (ignoring capitals), or
/// empty for none. `Err` explains a name that isn't one of them.
fn pick_partner(text: &str, accessories: &[Accessory]) -> Result<Option<AccessoryId>, String> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    accessories
        .iter()
        .find(|a| a.name.trim().eq_ignore_ascii_case(text))
        .map(|a| Some(a.id))
        .ok_or_else(|| format!("There's no accessory called “{text}” to pair with."))
}

/// A fabric typed into an accessory's form that isn't in the stash yet.
#[derive(Debug, Clone, PartialEq)]
struct NewFabricName {
    role: FabricRole,
    name: String,
    /// False if "Need to buy it" was ticked.
    in_stock: bool,
}

/// The fabric to create for `new`, with the accessory's own colors.
fn new_fabric(new: &NewFabricName, accessory: &NewAccessory) -> NewFabric {
    NewFabric {
        name: new.name.clone(),
        description: String::new(),
        colors: accessory.colors.clone(),
        keywords: Vec::new(),
        in_stock: new.in_stock,
    }
}

fn buy_field(field: &str) -> String {
    format!("buy_{field}")
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
    /// Comma-separated, like the fabric form's keywords.
    tags: String,
    colors: Vec<ColorRow>,
    colors_from_fabrics: bool,
    /// The chosen fabrics' colors, shown under the switch.
    fabric_colors: Vec<Color>,
    /// Every fabric's colors, for colors.js to show the right ones live as
    /// fabrics are chosen.
    fabric_chips: Vec<FabricChips>,
    bought: bool,
    brand: String,
    make_days: String,
    /// "How many", as typed (so a bad value can be shown again).
    quantity: String,
    fabric_boxes: Vec<FabricBox>,
    /// Every fabric, for the fabric boxes' suggestions.
    fabric_names: Vec<NameOption>,
    /// The partner's name, as saved or as typed.
    partner: String,
    /// Every other accessory, for the partner box's suggestions.
    partner_names: Vec<NameOption>,
    /// Likely partners (shared fabric, paired fabric, name), while there's none.
    partner_ideas: Vec<PartnerIdeaView>,
    /// The accessory's id when editing, for the page script's idea requests.
    editing_id: String,
    /// Outfits it's a piece of (edit page only).
    outfits: Vec<Link>,
    blocked: Blockers,
    photos: PhotoSection,
    /// Main colors of its photos, offered beside the color rows.
    color_ideas: Vec<IdeaGroup>,
    /// The palette, for the color name boxes and "Pick from your palette".
    palette: Vec<colors::PaletteOption>,
    errors: Vec<String>,
}

/// One fabric's colors, rendered as chips the page's script can copy.
/// `name` is lowercase, matching what's typed in a fabric box.
struct FabricChips {
    name: String,
    colors: Vec<Color>,
}

/// A likely partner, as shown under the partner box.
struct PartnerIdeaView {
    name: String,
    /// "same fabric: Velvet · also a bow"
    reasons: String,
    /// "already in a set with Bat bow", if it is.
    taken: String,
}

/// Partner ideas for `query`, ready to show.
fn partner_idea_views(
    query: &PartnerQuery<'_>,
    accessories: &[Accessory],
    fabrics: &[Fabric],
) -> Vec<PartnerIdeaView> {
    let name_of = |id: AccessoryId| {
        accessories
            .iter()
            .find(|a| a.id == id)
            .map(|a| a.name.clone())
            .unwrap_or_default()
    };
    suggest::partner_ideas(query, accessories, fabrics)
        .into_iter()
        .map(|idea| PartnerIdeaView {
            name: name_of(idea.accessory),
            reasons: idea.reasons.join(" · "),
            taken: idea
                .paired_with
                .map(|p| format!("already in a set with {}", name_of(p)))
                .unwrap_or_default(),
        })
        .collect()
}

/// Partner ideas for what's typed in an accessory form (the fabric boxes'
/// names that are fabrics already, the name and the kind).
fn ideas_for_form(
    form: &FormData,
    editing: Option<AccessoryId>,
    accessories: &[Accessory],
    fabrics: &[Fabric],
) -> Vec<PartnerIdeaView> {
    let fabric_ids: Vec<FabricId> = ROLES
        .iter()
        .filter_map(
            |(_, field, _)| match pick_fabric(&form.text(field), fabrics) {
                FabricPick::Existing(id) => Some(id),
                _ => None,
            },
        )
        .collect();
    let name = form.text("name");
    let query = PartnerQuery {
        name: &name,
        kind: parse_choice(&KINDS, &form.text("kind")).unwrap_or(AccessoryKind::Bow),
        fabrics: &fabric_ids,
        editing,
    };
    partner_idea_views(&query, accessories, fabrics)
}

/// The partner ideas alone, for the page script.
#[derive(Template)]
#[template(path = "partner_ideas.html")]
struct PartnerIdeasPart {
    partner_ideas: Vec<PartnerIdeaView>,
}

/// `GET /accessories/partner-ideas?name=…&kind=…&main_fabric=…&editing=ID`:
/// partner ideas for what's typed in the form, as a piece of HTML.
pub async fn partner_ideas(
    State(state): State<AppState>,
    Query(query): Query<Vec<(String, String)>>,
) -> Result<Response, AppError> {
    let form = FormData::new(query);
    let editing = form.text("editing").parse().ok().map(AccessoryId);
    let accessories = state.db.list_accessories().await?;
    let fabrics = state.db.list_fabrics().await?;
    let ideas = ideas_for_form(&form, editing, &accessories, &fabrics);
    Ok(render(PartnerIdeasPart {
        partner_ideas: ideas,
    })?
    .into_response())
}

/// A suggestion in a name box's list, with a note the page script shows
/// once it's chosen ("on the shopping list", "bow").
struct NameOption {
    name: String,
    note: String,
}

impl AccessoryFormPage {
    /// After a mistake: partner ideas for what was typed (if no partner was).
    fn with_ideas_from(
        mut self,
        form: &FormData,
        editing: Option<AccessoryId>,
        accessories: &[Accessory],
        fabrics: &[Fabric],
    ) -> Self {
        if form.text("partner").trim().is_empty() {
            self.partner_ideas = ideas_for_form(form, editing, accessories, fabrics);
        }
        self
    }

    fn new(editing: Option<&Accessory>, accessories: &[Accessory], fabrics: &[Fabric]) -> Self {
        let editing_id = editing.map(|a| a.id);
        let name_of = |id: FabricId| {
            fabrics
                .iter()
                .find(|f| f.id == id)
                .map(|f| f.name.clone())
                .unwrap_or_default()
        };
        let fabric_boxes = ROLES
            .iter()
            .map(|(role, field, label)| FabricBox {
                field,
                label,
                // The form has one fabric per role; show the first one saved.
                value: editing
                    .and_then(|a| a.fabrics.iter().find(|u| u.role == *role))
                    .map(|u| name_of(u.fabric_id))
                    .unwrap_or_default(),
                buy_field: buy_field(field),
                buy: false,
            })
            .collect();
        let fabric_names = fabrics
            .iter()
            .map(|f| NameOption {
                name: f.name.clone(),
                note: if f.in_stock {
                    "in your stash".into()
                } else {
                    "on the shopping list".into()
                },
            })
            .collect();

        let partner = editing
            .and_then(|a| a.pair_id)
            .and_then(|id| accessories.iter().find(|a| a.id == id))
            .map(|a| a.name.clone())
            .unwrap_or_default();
        // Ideas only while there's no partner yet.
        let partner_ideas = match editing {
            Some(a) if a.pair_id.is_none() => {
                let fabric_ids: Vec<FabricId> = a.fabrics.iter().map(|u| u.fabric_id).collect();
                let query = PartnerQuery {
                    name: &a.name,
                    kind: a.kind,
                    fabrics: &fabric_ids,
                    editing: Some(a.id),
                };
                partner_idea_views(&query, accessories, fabrics)
            }
            _ => Vec::new(),
        };
        let partner_names = accessories
            .iter()
            .filter(|a| Some(a.id) != editing_id)
            .map(|a| NameOption {
                name: a.name.clone(),
                note: label_of(&KINDS, a.kind).to_lowercase(),
            })
            .collect();

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
            tags: editing.map(|a| join_list(&a.tags)).unwrap_or_default(),
            colors: color_rows(editing.map_or(&[], |a| &a.colors)),
            // New accessories start with the box ticked: less typing.
            colors_from_fabrics: editing.is_none_or(|a| a.colors_from_fabrics),
            bought,
            brand,
            make_days: editing
                .and_then(|a| a.make_days)
                .map(|d| d.to_string())
                .unwrap_or_default(),
            quantity: editing.map_or(1, |a| a.quantity).to_string(),
            fabric_colors: Vec::new(),
            fabric_chips: fabrics
                .iter()
                .filter(|f| !f.colors.is_empty())
                .map(|f| FabricChips {
                    name: f.name.trim().to_lowercase(),
                    colors: f.colors.clone(),
                })
                .collect(),
            fabric_boxes,
            fabric_names,
            partner,
            partner_names,
            partner_ideas,
            editing_id: editing_id.map(|id| id.0.to_string()).unwrap_or_default(),
            outfits: Vec::new(),
            blocked: Blockers::default(),
            photos: PhotoSection::unsaved(),
            color_ideas: Vec::new(),
            palette: Vec::new(),
            errors: Vec::new(),
        }
        .with_fabric_colors()
    }

    /// Fill in `fabric_colors` from the fabrics named in the boxes, main
    /// fabric first, the same way `Accessory::effective_colors` combines them.
    fn with_fabric_colors(mut self) -> Self {
        let chosen = self.fabric_boxes.iter().filter_map(|b| {
            let name = b.value.trim().to_lowercase();
            self.fabric_chips.iter().find(|f| f.name == name)
        });
        self.fabric_colors = without_repeats(chosen.flat_map(|f| &f.colors));
        self
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
        self.partner = form.text("partner");
        self.partner_ideas = Vec::new(); // see `with_ideas_from`
        for b in &mut self.fabric_boxes {
            b.value = form.text(b.field);
            b.buy = form.checked(&b.buy_field);
        }
        self.name = form.text("name");
        self.description = form.text("description");
        self.tags = form.text("tags");
        self.colors = color_rows_from_form(form);
        self.colors_from_fabrics = form.checked("colors_from_fabrics");
        self = self.with_fabric_colors();
        self.bought = form.text("source") == "bought";
        self.brand = form.text("brand");
        self.make_days = form.text("make_days");
        self.quantity = form.text("quantity");
        self.errors = errors;
        self
    }
}

struct AccessoryInput {
    accessory: NewAccessory,
    partner: Option<AccessoryId>,
    /// Fabric names typed that aren't fabrics yet, to create on save.
    new_fabrics: Vec<NewFabricName>,
}

/// `stash` and `accessories` are every fabric and accessory, for the name
/// boxes.
fn read_form(
    form: &FormData,
    editing: Option<AccessoryId>,
    stash: &[Fabric],
    accessories: &[Accessory],
) -> Result<AccessoryInput, Vec<String>> {
    let mut errors = Vec::new();

    let name = form.text("name");
    if name.is_empty() {
        errors.push("Give it a name.".to_string());
    }
    let kind = parse_choice(&KINDS, &form.text("kind"));
    if kind.is_none() {
        errors.push("Choose what kind of accessory it is.".to_string());
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

    // Empty means 1: one of it, as before quantities existed.
    let quantity = match form.text("quantity").as_str() {
        "" => 1,
        text => match text.parse::<u32>() {
            Ok(n) if n > 0 => n,
            _ => {
                errors.push("How many must be a whole number of 1 or more.".to_string());
                1
            }
        },
    };

    let colors = read_colors(form).unwrap_or_else(|color_errors| {
        errors.extend(color_errors);
        Vec::new()
    });

    let mut fabrics = Vec::new();
    let mut new_fabrics = Vec::new();
    for (role, field, _) in ROLES {
        match pick_fabric(&form.text(field), stash) {
            FabricPick::None => {}
            FabricPick::Existing(fabric_id) => fabrics.push(FabricUse { fabric_id, role }),
            FabricPick::New(name) => new_fabrics.push(NewFabricName {
                role,
                name,
                in_stock: !form.checked(&buy_field(field)),
            }),
        }
    }

    let others: Vec<Accessory> = accessories
        .iter()
        .filter(|a| Some(a.id) != editing)
        .cloned()
        .collect();
    let partner = match pick_partner(&form.text("partner"), &others) {
        Ok(p) => p,
        Err(problem) => {
            // Its own name is the one mistake worth naming specifically.
            let own = editing.is_some()
                && accessories.iter().any(|a| {
                    Some(a.id) == editing
                        && a.name.eq_ignore_ascii_case(form.text("partner").trim())
                });
            errors.push(if own {
                "An accessory can't be its own partner.".to_string()
            } else {
                problem
            });
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
                tags: split_list(&form.text("tags")),
                fabrics,
                source,
                status,
                make_days,
                quantity,
            },
            partner,
            new_fabrics,
        }),
        _ => Err(errors),
    }
}

/// Turn typed names that aren't fabrics yet into fabrics for the accessory: an existing
/// fabric with the same name (ignoring capitals) is used as it is, rather
/// than making a duplicate; otherwise a new one is created, in the stash
/// unless "Need to buy it" was ticked, starting with the accessory's own
/// colors (a bow is usually made from fabric of its colors; they can be fixed
/// on the fabric's page). The same new name in two roles makes one fabric.
/// Afterwards the fabrics are put back in role order (main first), which is
/// the order their colors are listed in.
async fn fabrics_for_new_names(
    state: &AppState,
    input: &mut AccessoryInput,
) -> Result<(), AppError> {
    if input.new_fabrics.is_empty() {
        return Ok(());
    }
    let mut known: Vec<(String, FabricId)> = state
        .db
        .list_fabrics()
        .await?
        .into_iter()
        .map(|f| (f.name.to_lowercase(), f.id))
        .collect();
    for new in &input.new_fabrics {
        let key = new.name.to_lowercase();
        let fabric_id = match known.iter().find(|(n, _)| *n == key) {
            Some((_, id)) => *id,
            None => {
                let id = state
                    .db
                    .create_fabric(&new_fabric(new, &input.accessory))
                    .await?;
                known.push((key, id));
                id
            }
        };
        input.accessory.fabrics.push(FabricUse {
            fabric_id,
            role: new.role,
        });
    }
    let order = |role: FabricRole| ROLES.iter().position(|(r, _, _)| *r == role);
    input.accessory.fabrics.sort_by_key(|u| order(u.role));
    Ok(())
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
    page.outfits = outfits::outfits_with(accessory.id, &state.db.list_outfits().await?);
    let owner = PhotoOwner::Accessory(accessory.id);
    let photos = state.db.list_photos(owner).await?;
    page.photos = PhotoSection::gallery(owner, &accessory.name, &photos);
    // Compare with all its colors, including the ones from its fabrics.
    let colors = accessory.effective_colors(&fabrics);
    page.color_ideas = photos::color_ideas(state, &photos, &colors).await;
    Ok(page)
}

/// Show the form with `status`. Every form page goes through here, so this
/// is where the palette (for the color rows) is added.
async fn form_response(
    state: &AppState,
    status: StatusCode,
    mut page: AccessoryFormPage,
) -> Result<Response, AppError> {
    page.palette = colors::palette_options(&state.db.list_palette().await?);
    Ok((status, render(page)?).into_response())
}

pub async fn new_form(State(state): State<AppState>) -> Result<Response, AppError> {
    let accessories = state.db.list_accessories().await?;
    let fabrics = state.db.list_fabrics().await?;
    form_response(
        &state,
        StatusCode::OK,
        AccessoryFormPage::new(None, &accessories, &fabrics),
    )
    .await
}

pub async fn create(
    State(state): State<AppState>,
    FormWithPhotos { form, photos }: FormWithPhotos,
) -> Result<Response, AppError> {
    let stash = state.db.list_fabrics().await?;
    let accessories = state.db.list_accessories().await?;
    let chosen = !photos.is_empty();
    let checked = photos::with_photos(
        read_form(&form, None, &stash, &accessories),
        photos::prepare_all(&state, photos).await,
        chosen,
    );
    let (input, files) = match checked {
        Ok(ok) => ok,
        Err(errors) => {
            let page = AccessoryFormPage::new(None, &accessories, &stash)
                .refill(&form, errors)
                .with_ideas_from(&form, None, &accessories, &stash);
            return form_response(&state, StatusCode::UNPROCESSABLE_ENTITY, page).await;
        }
    };
    let mut input = input;
    fabrics_for_new_names(&state, &mut input).await?;
    let id = state.db.create_accessory(&input.accessory).await?;
    apply_partner(&state, id, None, input.partner).await?;
    photos::save_all(&state, PhotoOwner::Accessory(id), files).await?;
    Ok(Redirect::to("/accessories").into_response())
}

pub async fn edit_form(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Response, AppError> {
    let accessory = state.db.get_accessory(AccessoryId(id)).await?;
    form_response(&state, StatusCode::OK, edit_page(&state, &accessory).await?).await
}

pub async fn update(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    FormWithPhotos { form, photos }: FormWithPhotos,
) -> Result<Response, AppError> {
    let accessory = state.db.get_accessory(AccessoryId(id)).await?;
    let stash = state.db.list_fabrics().await?;
    let accessories = state.db.list_accessories().await?;
    let chosen = !photos.is_empty();
    let checked = photos::with_photos(
        read_form(&form, Some(accessory.id), &stash, &accessories),
        photos::prepare_all(&state, photos).await,
        chosen,
    );
    let (input, files) = match checked {
        Ok(ok) => ok,
        Err(errors) => {
            let page = edit_page(&state, &accessory)
                .await?
                .refill(&form, errors)
                .with_ideas_from(&form, Some(accessory.id), &accessories, &stash);
            return form_response(&state, StatusCode::UNPROCESSABLE_ENTITY, page).await;
        }
    };
    let mut input = input;
    fabrics_for_new_names(&state, &mut input).await?;
    state
        .db
        .update_accessory(accessory.id, &input.accessory)
        .await?;
    apply_partner(&state, accessory.id, accessory.pair_id, input.partner).await?;
    photos::save_all(&state, PhotoOwner::Accessory(accessory.id), files).await?;
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
            form_response(&state, StatusCode::CONFLICT, page).await
        }
        Err(e) => Err(e.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fabric(id: i64, name: &str) -> Fabric {
        Fabric {
            id: FabricId(id),
            name: name.into(),
            description: String::new(),
            colors: vec![],
            keywords: vec![],
            pairings: vec![],
            in_stock: true,
        }
    }

    #[test]
    fn a_fabric_box_is_none_a_fabric_or_a_new_one() {
        let stash = [fabric(1, "Orange velvet"), fabric(2, "Black satin")];
        assert_eq!(pick_fabric("", &stash), FabricPick::None);
        assert_eq!(pick_fabric("   ", &stash), FabricPick::None);
        assert_eq!(
            pick_fabric("orange VELVET ", &stash),
            FabricPick::Existing(FabricId(1))
        );
        assert_eq!(
            pick_fabric(" Polka dots ", &stash),
            FabricPick::New("Polka dots".into()),
            "trimmed, capitals kept"
        );
    }

    #[test]
    fn fabrics_show_their_photo_or_their_name_main_first() {
        let stash = [fabric(1, "Orange velvet"), fabric(2, "Black satin")];
        let bow = Accessory {
            fabrics: vec![
                FabricUse {
                    fabric_id: FabricId(2),
                    role: FabricRole::Middle,
                },
                FabricUse {
                    fabric_id: FabricId(1),
                    role: FabricRole::Main,
                },
            ],
            ..ghost_scarf()
        };
        let thumbs = HashMap::from([(
            PhotoOwner::Fabric(FabricId(2)),
            "/photos/9/thumb".to_string(),
        )]);
        let links = fabric_links(&bow, &stash, &thumbs);
        assert_eq!(links[0].title, "Black satin (middle)", "in the saved order");
        assert_eq!(links[0].thumb.as_deref(), Some("/photos/9/thumb"));
        assert_eq!(links[1].thumb, None, "no photo: shown by name");
        assert_eq!(links[1].href, "/fabrics/1");
    }

    fn ghost_scarf() -> Accessory {
        Accessory {
            id: AccessoryId(3),
            kind: AccessoryKind::Scarf,
            name: "Ghost scarf".into(),
            description: String::new(),
            colors: vec![],
            colors_from_fabrics: false,
            tags: vec![],
            fabrics: vec![],
            source: Source::Handmade,
            status: AccessoryStatus::Ready,
            pair_id: None,
            make_days: None,
            quantity: 1,
        }
    }

    #[test]
    fn a_partner_must_be_an_accessory() {
        let bow = Accessory {
            id: AccessoryId(3),
            kind: AccessoryKind::Scarf,
            name: "Ghost scarf".into(),
            description: String::new(),
            colors: vec![],
            colors_from_fabrics: false,
            tags: vec![],
            fabrics: vec![],
            source: Source::Handmade,
            status: AccessoryStatus::Ready,
            pair_id: None,
            make_days: None,
            quantity: 1,
        };
        assert_eq!(pick_partner("", std::slice::from_ref(&bow)), Ok(None));
        assert_eq!(
            pick_partner("ghost SCARF", std::slice::from_ref(&bow)),
            Ok(Some(AccessoryId(3)))
        );
        assert!(pick_partner("Ghost scraf", &[bow]).is_err());
    }
}
