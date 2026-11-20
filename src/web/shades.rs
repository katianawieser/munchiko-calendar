//! The Shades page: near-duplicate shades in the stash, and merging them
//! (M13c). The grouping and the merge itself are worked out in
//! [`crate::shades`]; this module shows them and saves the result.
//!
//! - `GET /shades`: one card per color family ([`shades::families`]), so each
//!   shade appears once; `closeness=tight|medium|loose` says how alike a lone
//!   shade of another name must be to be offered as "similar".
//! - `GET /shades/merge`: what a merge would change, item by item.
//! - `POST /shades/merge`: save a copy of the database, then apply it.
//!
//! The merge is always recomputed from the current data on the server; the
//! page only says which shades and which one to keep.

use askama::Template;
use axum::Form;
use axum::extract::{Query, State};
use axum::response::{IntoResponse, Redirect, Response};
use chrono::Local;

use super::blockers::Link;
use super::forms::FormData;
use super::{AppError, AppState, render};
use crate::models::{Accessory, Color, Fabric};
use crate::shades::{self, Closeness, Family, MergePlan, Owner, Shade};

const CLOSENESS: [(Closeness, &str, &str); 3] = [
    (Closeness::Tight, "tight", "Tight"),
    (Closeness::Medium, "medium", "Medium"),
    (Closeness::Loose, "loose", "Loose"),
];

/// How alike "similar" shades must be, from the address (Medium if unset).
fn read_closeness(form: &FormData) -> Closeness {
    CLOSENESS
        .iter()
        .find(|(_, value, _)| *value == form.text("closeness"))
        .map_or(Closeness::Medium, |(c, _, _)| *c)
}

fn closeness_value(closeness: Closeness) -> &'static str {
    CLOSENESS
        .iter()
        .find(|(c, _, _)| *c == closeness)
        .unwrap()
        .1
}

/// The list's address with this closeness.
fn list_href(closeness: Closeness) -> String {
    format!("/shades?closeness={}", closeness_value(closeness))
}

/// Links to the fabrics and accessories that use a shade.
fn user_links(
    uses: &[(Owner, String)],
    fabrics: &[Fabric],
    accessories: &[Accessory],
) -> Vec<Link> {
    let mut links: Vec<Link> = Vec::new();
    for (owner, _) in uses {
        let link = match owner {
            Owner::Fabric(id) => fabrics.iter().find(|f| f.id == *id).map(|f| Link {
                label: f.name.clone(),
                href: format!("/fabrics/{}", id.0),
            }),
            Owner::Accessory(id) => accessories.iter().find(|a| a.id == *id).map(|a| Link {
                label: a.name.clone(),
                href: format!("/accessories/{}", id.0),
            }),
        };
        if let Some(link) = link.filter(|l| !links.iter().any(|seen| seen.href == l.href)) {
            links.push(link);
        }
    }
    links
}

// ---------------------------------------------------------------------------
// GET /shades
// ---------------------------------------------------------------------------

struct ShadeView {
    hex: String,
    name: String,
    /// Other names it's used under ("also called charcoal").
    other_names: String,
    uses: usize,
    users: Vec<Link>,
    /// Ticked for merging: the family's own shades, not the similar ones.
    merge: bool,
    /// The "keep" radio starts on the family's most used shade.
    keep: bool,
}

struct FamilyView {
    /// Numbers the card's name suggestions (`<datalist id="names-3">`).
    index: usize,
    name: String,
    shades: Vec<ShadeView>,
    similar: Vec<ShadeView>,
    /// Names to suggest in the card's Name box.
    name_ideas: Vec<String>,
}

/// A Tight / Medium / Loose link.
struct Tab {
    label: &'static str,
    href: String,
    current: bool,
}

#[derive(Template)]
#[template(path = "shades.html")]
struct ShadesPage {
    closeness: &'static str,
    total_shades: usize,
    families: Vec<FamilyView>,
    closeness_tabs: Vec<Tab>,
    /// After a merge: "Merged: 9 colors on 7 items changed. ...".
    notice: Option<String>,
}

fn shade_view(
    shade: &Shade,
    merge: bool,
    keep: bool,
    fabrics: &[Fabric],
    accessories: &[Accessory],
) -> ShadeView {
    ShadeView {
        hex: shade.hex.clone(),
        name: shade.name().to_string(),
        other_names: shade
            .names
            .iter()
            .skip(1)
            .cloned()
            .collect::<Vec<_>>()
            .join(", "),
        uses: shade.uses.len(),
        users: user_links(&shade.uses, fabrics, accessories),
        merge,
        keep,
    }
}

fn family_view(
    index: usize,
    family: &Family,
    stash: &[Shade],
    fabrics: &[Fabric],
    accessories: &[Accessory],
) -> FamilyView {
    let members: Vec<&Shade> = family.shades.iter().chain(&family.similar).collect();
    FamilyView {
        index,
        name_ideas: shades::name_ideas(&members, stash),
        name: family.name.clone(),
        shades: family
            .shades
            .iter()
            .enumerate()
            .map(|(i, s)| shade_view(s, true, i == 0, fabrics, accessories))
            .collect(),
        similar: family
            .similar
            .iter()
            .map(|s| shade_view(s, false, false, fabrics, accessories))
            .collect(),
    }
}

pub async fn list(
    State(state): State<AppState>,
    Query(query): Query<Vec<(String, String)>>,
) -> Result<Response, AppError> {
    let form = FormData::new(query);
    let closeness = read_closeness(&form);
    let fabrics = state.db.list_fabrics().await?;
    let accessories = state.db.list_accessories().await?;
    let all = shades::collect(&fabrics, &accessories);
    let families = shades::families(&all, closeness);

    let notice = (!form.text("merged").is_empty()).then(|| {
        format!(
            "Done: {} changed. A copy of the database from before was saved as backups/{}.",
            form.text("merged"),
            form.text("backup")
        )
    });
    let closeness_tabs = CLOSENESS
        .iter()
        .map(|(c, _, label)| Tab {
            label,
            href: list_href(*c),
            current: *c == closeness,
        })
        .collect();
    Ok(render(ShadesPage {
        closeness: closeness_value(closeness),
        total_shades: all.len(),
        families: families
            .iter()
            .enumerate()
            .map(|(i, f)| family_view(i, f, &all, &fabrics, &accessories))
            .collect(),
        closeness_tabs,
        notice,
    })?
    .into_response())
}

// ---------------------------------------------------------------------------
// The merge: preview, then apply
// ---------------------------------------------------------------------------

/// The change the form asks for. `action=merge` (the default): the ticked
/// shades (`from`) and the kept one (`keep`, which counts as ticked) become
/// the kept shade, under `new_name` or else the kept shade's own name (read
/// from the current data, not the page). `action=rename`: the ticked shades
/// keep their shades and take `new_name`, which is then required.
///
/// `Ok(Err(message))` is a mistake to explain on the page; `Err` is a
/// request no page of the app would send.
fn read_plan(form: &FormData, all: &[Shade]) -> Result<Result<MergePlan, String>, AppError> {
    let mut from: Vec<String> = form.all("from").filter_map(Color::parse_hex).collect();
    // Names are lowercase everywhere, like the color forms make them.
    let typed = form.text("new_name").trim().to_lowercase();
    if form.text("action") == "rename" {
        if from.is_empty() {
            return Ok(Err("Tick the shades to rename.".into()));
        }
        if typed.is_empty() {
            return Ok(Err("Type the new name for the ticked shades.".into()));
        }
        return Ok(Ok(MergePlan {
            from,
            name: typed,
            shade: None,
        }));
    }
    let keep = Color::parse_hex(&form.text("keep")).ok_or(AppError::BadRequest)?;
    let kept = all
        .iter()
        .find(|s| s.hex == keep)
        .ok_or(AppError::NotFound)?;
    if !from.contains(&keep) {
        from.push(keep.clone());
    }
    let name = if typed.is_empty() {
        kept.name().to_string()
    } else {
        typed
    };
    Ok(Ok(MergePlan {
        from,
        name,
        shade: Some(keep),
    }))
}

/// One item a merge changes, before and after.
struct ChangeRow {
    link: Link,
    before: Vec<Color>,
    /// Each color after the merge, and whether it's new (highlighted).
    after: Vec<(Color, bool)>,
}

#[derive(Template)]
#[template(path = "shade_merge.html")]
struct MergePage {
    /// The result: the kept shade and name, or (a rename) just the name.
    result: Color,
    rename: bool,
    /// A mistake in the form, shown instead of the changes.
    problem: Option<String>,
    rows: Vec<ChangeRow>,
    /// "2 colors on 2 items".
    summary: String,
    /// The form's fields, to send again with the confirm button.
    fields: Vec<(String, String)>,
    back: String,
}

/// The items `plan` changes, with links and before/after colors.
fn change_rows(plan: &MergePlan, fabrics: &[Fabric], accessories: &[Accessory]) -> Vec<ChangeRow> {
    plan.changes(fabrics, accessories)
        .into_iter()
        .filter_map(|(owner, after)| {
            let (link, before) = match owner {
                Owner::Fabric(id) => {
                    let f = fabrics.iter().find(|f| f.id == id)?;
                    (
                        Link {
                            label: f.name.clone(),
                            href: format!("/fabrics/{}", id.0),
                        },
                        f.colors.clone(),
                    )
                }
                Owner::Accessory(id) => {
                    let a = accessories.iter().find(|a| a.id == id)?;
                    (
                        Link {
                            label: a.name.clone(),
                            href: format!("/accessories/{}", id.0),
                        },
                        a.colors.clone(),
                    )
                }
            };
            let after = after
                .into_iter()
                .map(|c| {
                    let new = !before.contains(&c);
                    (c, new)
                })
                .collect();
            Some(ChangeRow {
                link,
                before,
                after,
            })
        })
        .collect()
}

/// "1 color", "2 colors".
fn plural(n: usize, word: &str) -> String {
    if n == 1 {
        format!("1 {word}")
    } else {
        format!("{n} {word}s")
    }
}

/// How many colors differ between `before` and `after` (changed or dropped).
fn colors_changed(rows: &[ChangeRow]) -> usize {
    rows.iter()
        .map(|r| {
            r.before
                .iter()
                .filter(|c| !r.after.iter().any(|(a, _)| a == *c))
                .count()
        })
        .sum()
}

pub async fn preview(
    State(state): State<AppState>,
    Query(query): Query<Vec<(String, String)>>,
) -> Result<Response, AppError> {
    let form = FormData::new(query.clone());
    let fabrics = state.db.list_fabrics().await?;
    let accessories = state.db.list_accessories().await?;
    let all = shades::collect(&fabrics, &accessories);
    let back = list_href(read_closeness(&form));
    let plan = match read_plan(&form, &all)? {
        Ok(plan) => plan,
        Err(problem) => {
            return Ok(render(MergePage {
                result: Color::from(""),
                rename: false,
                problem: Some(problem),
                rows: Vec::new(),
                summary: String::new(),
                fields: Vec::new(),
                back,
            })?
            .into_response());
        }
    };
    let rows = change_rows(&plan, &fabrics, &accessories);
    Ok(render(MergePage {
        result: Color {
            name: plan.name.clone(),
            hex: plan.shade.clone(),
        },
        rename: plan.shade.is_none(),
        problem: None,
        summary: format!(
            "{} on {}",
            plural(colors_changed(&rows), "color"),
            plural(rows.len(), "item")
        ),
        rows,
        fields: query,
        back,
    })?
    .into_response())
}

pub async fn apply(
    State(state): State<AppState>,
    Form(pairs): Form<Vec<(String, String)>>,
) -> Result<Response, AppError> {
    let form = FormData::new(pairs);
    let back = list_href(read_closeness(&form));
    let fabrics = state.db.list_fabrics().await?;
    let accessories = state.db.list_accessories().await?;
    let all = shades::collect(&fabrics, &accessories);
    // The preview already explained any mistake; a form sent anyway does nothing.
    let Ok(plan) = read_plan(&form, &all)? else {
        return Ok(Redirect::to(&back).into_response());
    };
    let changes = plan.changes(&fabrics, &accessories);
    if changes.is_empty() {
        return Ok(Redirect::to(&back).into_response());
    }
    let rows = change_rows(&plan, &fabrics, &accessories);
    let summary = format!(
        "{} on {}",
        plural(colors_changed(&rows), "color"),
        plural(changes.len(), "item")
    );

    // A copy of the database first, so a merge can always be undone by hand.
    let folder = state.backups.as_ref().ok_or_else(|| {
        AppError::Internal("no backups folder is set, so merging is switched off".into())
    })?;
    tokio::fs::create_dir_all(folder)
        .await
        .map_err(|e| AppError::Internal(format!("making {}: {e}", folder.display())))?;
    let file = format!(
        "munchiko-before-merge-{}.db",
        Local::now().format("%Y-%m-%d-%H%M%S%3f")
    );
    state.db.backup_to(&folder.join(&file)).await?;

    state.db.set_colors(&changes).await?;
    let separator = if back.contains('?') { '&' } else { '?' };
    Ok(Redirect::to(&format!(
        "{back}{separator}merged={}&backup={}",
        summary.replace(' ', "+"),
        file
    ))
    .into_response())
}
