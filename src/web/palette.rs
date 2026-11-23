//! The Palette page (M15a): every color the stash uses, once, in rainbow
//! order, with what uses it. Fabrics and accessories point at palette colors,
//! so renaming or reshading one here changes it everywhere.
//!
//! - `GET /palette`: the list, and a form to add a color.
//! - `POST /palette`: add a color.
//! - `POST /palette/{id}`: rename / reshade.
//! - `POST /palette/{id}/delete`: delete a color nothing uses.
//! - `GET /palette/{id}/merge?into=NAME`: what merging it into another
//!   color would change; `POST` the same: save a copy of the database, then
//!   merge.
//!
//! Plain HTML forms; no JavaScript needed.

use std::collections::HashMap;

use askama::Template;
use axum::Form;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use chrono::Local;

use super::blockers::Link;
use super::forms::FormData;
use super::{AppError, AppState, render};
use crate::db::DbError;
use crate::models::{Accessory, Color, Fabric, PaletteColor, PaletteColorId};
use crate::palette;
use crate::shades::Owner;

/// One palette color as listed.
struct Row {
    id: i64,
    color: Color,
    /// For the shade picker: the shade, or white when there's none.
    picker: String,
    uses: usize,
    users: Vec<Link>,
}

#[derive(Template)]
#[template(path = "palette.html")]
struct PalettePage {
    rows: Vec<Row>,
    /// Every color's name, for the "merge into" boxes.
    names: Vec<String>,
    notice: Option<String>,
    errors: Vec<String>,
    /// The add form, as typed (kept after a mistake).
    new_name: String,
    new_hex: String,
    new_no_shade: bool,
}

/// Links to the items using each palette color.
fn uses_by_color(
    uses: &[(PaletteColorId, Owner)],
    fabrics: &[Fabric],
    accessories: &[Accessory],
) -> HashMap<PaletteColorId, Vec<Link>> {
    let mut by_color: HashMap<PaletteColorId, Vec<Link>> = HashMap::new();
    for (color, owner) in uses {
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
        if let Some(link) = link {
            by_color.entry(*color).or_default().push(link);
        }
    }
    by_color
}

/// Rainbow order (like the lists' "sort by color"), name-only colors last.
fn rainbow(colors: &mut [PaletteColor]) {
    colors.sort_by(|a, b| {
        let key = |c: &PaletteColor| {
            c.hex
                .as_deref()
                .and_then(palette::rgb_from_hex)
                .map_or((true, (true, 0, 0)), |rgb| {
                    (false, palette::rainbow_order(rgb))
                })
        };
        key(a)
            .cmp(&key(b))
            .then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
}

async fn page(state: &AppState, notice: Option<String>) -> Result<PalettePage, AppError> {
    let mut colors = state.db.list_palette().await?;
    let uses = state.db.palette_uses().await?;
    let fabrics = state.db.list_fabrics().await?;
    let accessories = state.db.list_accessories().await?;
    let mut users = uses_by_color(&uses, &fabrics, &accessories);
    let names = colors.iter().map(|c| c.name.clone()).collect();
    rainbow(&mut colors);
    Ok(PalettePage {
        rows: colors
            .iter()
            .map(|c| {
                let users = users.remove(&c.id).unwrap_or_default();
                Row {
                    id: c.id.0,
                    picker: c.hex.clone().unwrap_or_else(|| "#ffffff".into()),
                    color: c.color(),
                    uses: users.len(),
                    users,
                }
            })
            .collect(),
        names,
        notice,
        errors: Vec::new(),
        new_name: String::new(),
        new_hex: "#ffffff".into(),
        new_no_shade: true,
    })
}

pub async fn list(
    State(state): State<AppState>,
    Query(query): Query<Vec<(String, String)>>,
) -> Result<Response, AppError> {
    let form = FormData::new(query);
    let notice = Some(form.text("done"))
        .filter(|d| !d.is_empty())
        .map(|done| match form.text("backup") {
            b if b.is_empty() => format!("{done}."),
            b => format!("{done}. A copy of the database from before was saved as backups/{b}."),
        });
    Ok(render(page(&state, notice).await?)?.into_response())
}

/// A palette color's name and shade from a form: `name`, `hex` from the
/// picker, and `no_shade` (ticked: no shade). Names are lowercase, like the
/// color forms make them.
fn read_color(form: &FormData) -> Result<(String, Option<String>), String> {
    let name = form.text("name").trim().to_lowercase();
    if name.is_empty() {
        return Err("Give the color a name.".into());
    }
    if form.checked("no_shade") {
        return Ok((name, None));
    }
    match Color::parse_hex(&form.text("hex")) {
        Some(hex) => Ok((name, Some(hex))),
        None => Err("That shade isn't a color (#rrggbb).".into()),
    }
}

/// Show the page again with `errors` (422), keeping the add form as typed.
async fn with_errors(
    state: &AppState,
    form: &FormData,
    errors: Vec<String>,
    adding: bool,
) -> Result<Response, AppError> {
    let mut page = page(state, None).await?;
    page.errors = errors;
    if adding {
        page.new_name = form.text("name");
        page.new_no_shade = form.checked("no_shade");
        if let Some(hex) = Color::parse_hex(&form.text("hex")) {
            page.new_hex = hex;
        }
    }
    Ok((StatusCode::UNPROCESSABLE_ENTITY, render(page)?).into_response())
}

pub async fn create(
    State(state): State<AppState>,
    Form(pairs): Form<Vec<(String, String)>>,
) -> Result<Response, AppError> {
    let form = FormData::new(pairs);
    let (name, hex) = match read_color(&form) {
        Ok(color) => color,
        Err(problem) => return with_errors(&state, &form, vec![problem], true).await,
    };
    match state.db.create_palette_color(&name, hex.as_deref()).await {
        Ok(_) => {
            Ok(Redirect::to(&format!("/palette?done=Added+{}", encode(&name))).into_response())
        }
        Err(DbError::Duplicate { name, .. }) => {
            let problem = format!("There's already a color called “{name}”.");
            with_errors(&state, &form, vec![problem], true).await
        }
        Err(e) => Err(e.into()),
    }
}

pub async fn update(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Form(pairs): Form<Vec<(String, String)>>,
) -> Result<Response, AppError> {
    let form = FormData::new(pairs);
    let (name, hex) = match read_color(&form) {
        Ok(color) => color,
        Err(problem) => return with_errors(&state, &form, vec![problem], false).await,
    };
    match state
        .db
        .update_palette_color(PaletteColorId(id), &name, hex.as_deref())
        .await
    {
        Ok(()) => {
            Ok(Redirect::to(&format!("/palette?done=Saved+{}", encode(&name))).into_response())
        }
        Err(DbError::Duplicate { name, .. }) => {
            let problem = format!(
                "There's already a color called “{name}”. To make them one color, merge them instead."
            );
            with_errors(&state, &form, vec![problem], false).await
        }
        Err(e) => Err(e.into()),
    }
}

pub async fn delete(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Response, AppError> {
    match state.db.delete_palette_color(PaletteColorId(id)).await {
        Ok(()) => Ok(Redirect::to("/palette?done=Deleted").into_response()),
        Err(DbError::InUse { .. }) => {
            let form = FormData::new(Vec::new());
            let problem = "That color is still used, so it can't be deleted. Merge it into another color instead.".to_string();
            with_errors(&state, &form, vec![problem], false).await
        }
        Err(e) => Err(e.into()),
    }
}

/// Percent-encode text for an address (the notice after a change).
fn encode(text: &str) -> String {
    text.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' => (b as char).to_string(),
            b' ' => "+".into(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Merging one color into another
// ---------------------------------------------------------------------------

/// An item a merge changes: its colors now and after.
struct ChangeRow {
    link: Link,
    before: Vec<Color>,
    /// Each color after, and whether it's the merged one (highlighted).
    after: Vec<(Color, bool)>,
}

#[derive(Template)]
#[template(path = "palette_merge.html")]
struct MergePage {
    from: Color,
    into: Color,
    id: i64,
    into_name: String,
    rows: Vec<ChangeRow>,
    /// A mistake ("no color called …"), shown instead of the changes.
    problem: Option<String>,
}

/// The two palette colors a merge is about: `id` and the one named `into`.
async fn merge_pair(
    state: &AppState,
    id: i64,
    into_name: &str,
) -> Result<Result<(PaletteColor, PaletteColor), String>, AppError> {
    let colors = state.db.list_palette().await?;
    let from = colors
        .iter()
        .find(|c| c.id.0 == id)
        .cloned()
        .ok_or(AppError::NotFound)?;
    let into = colors
        .iter()
        .find(|c| c.name.eq_ignore_ascii_case(into_name.trim()))
        .cloned();
    Ok(match into {
        None if into_name.trim().is_empty() => Err("Type the color to merge it into.".into()),
        None => Err(format!("There's no color called “{}”.", into_name.trim())),
        Some(into) if into.id == from.id => Err("That's the same color.".into()),
        Some(into) => Ok((from, into)),
    })
}

/// The items using `from`, with their colors before and after it becomes
/// `into` (an item that already has `into` keeps it once).
fn change_rows(
    from: &PaletteColor,
    into: &PaletteColor,
    fabrics: &[Fabric],
    accessories: &[Accessory],
) -> Vec<ChangeRow> {
    let (from_c, into_c) = (from.color(), into.color());
    let lists = fabrics
        .iter()
        .map(|f| (format!("/fabrics/{}", f.id.0), &f.name, &f.colors))
        .chain(
            accessories
                .iter()
                .map(|a| (format!("/accessories/{}", a.id.0), &a.name, &a.colors)),
        );
    lists
        .filter(|(_, _, colors)| colors.contains(&from_c))
        .map(|(href, name, colors)| {
            let mut after: Vec<(Color, bool)> = Vec::new();
            for c in colors {
                let (c, merged) = if *c == from_c {
                    (into_c.clone(), true)
                } else {
                    (c.clone(), false)
                };
                if !after.iter().any(|(seen, _)| *seen == c) {
                    after.push((c, merged));
                }
            }
            ChangeRow {
                link: Link {
                    label: name.clone(),
                    href,
                },
                before: colors.clone(),
                after,
            }
        })
        .collect()
}

pub async fn merge_preview(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Query(query): Query<Vec<(String, String)>>,
) -> Result<Response, AppError> {
    let form = FormData::new(query);
    let into_name = form.text("into");
    let (from, into) = match merge_pair(&state, id, &into_name).await? {
        Ok(pair) => pair,
        Err(problem) => {
            let colors = state.db.list_palette().await?;
            let from = colors
                .iter()
                .find(|c| c.id.0 == id)
                .ok_or(AppError::NotFound)?;
            return Ok(render(MergePage {
                from: from.color(),
                into: Color::from(""),
                id,
                into_name,
                rows: Vec::new(),
                problem: Some(problem),
            })?
            .into_response());
        }
    };
    let fabrics = state.db.list_fabrics().await?;
    let accessories = state.db.list_accessories().await?;
    Ok(render(MergePage {
        rows: change_rows(&from, &into, &fabrics, &accessories),
        from: from.color(),
        into: into.color(),
        id,
        into_name: into.name.clone(),
        problem: None,
    })?
    .into_response())
}

/// Save a copy of the database in the backups folder; its file name.
async fn backup(state: &AppState) -> Result<String, AppError> {
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
    Ok(file)
}

pub async fn merge(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Form(pairs): Form<Vec<(String, String)>>,
) -> Result<Response, AppError> {
    let form = FormData::new(pairs);
    // The preview explained any mistake; a form sent anyway does nothing.
    let Ok((from, into)) = merge_pair(&state, id, &form.text("into")).await? else {
        return Ok(Redirect::to("/palette").into_response());
    };
    let file = backup(&state).await?;
    state.db.merge_palette_colors(&[from.id], into.id).await?;
    Ok(Redirect::to(&format!(
        "/palette?done=Merged+{}+into+{}&backup={file}",
        encode(&from.name),
        encode(&into.name)
    ))
    .into_response())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::FabricId;

    fn color(id: i64, name: &str, hex: Option<&str>) -> PaletteColor {
        PaletteColor {
            id: PaletteColorId(id),
            name: name.into(),
            hex: hex.map(String::from),
        }
    }

    #[test]
    fn the_palette_is_in_rainbow_order_name_only_colors_last() {
        let mut colors = vec![
            color(1, "sparkly", None),
            color(2, "navy", Some("#1f2531")),
            color(3, "red", Some("#cc0000")),
        ];
        rainbow(&mut colors);
        let names: Vec<&str> = colors.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["red", "navy", "sparkly"]);
    }

    #[test]
    fn a_merge_preview_shows_each_item_once_with_the_merged_color() {
        let pink = color(1, "pink (2)", Some("#f35f69"));
        let coral = color(2, "coral", Some("#f36f6f"));
        let fabric = |id, colors: Vec<Color>| Fabric {
            id: FabricId(id),
            name: format!("Fabric {id}"),
            description: String::new(),
            colors,
            keywords: vec![],
            pairings: vec![],
            in_stock: true,
        };
        let fabrics = [
            fabric(1, vec![pink.color(), Color::from("white")]),
            fabric(2, vec![coral.color(), pink.color()]),
            fabric(3, vec![Color::from("white")]),
        ];
        let rows = change_rows(&pink, &coral, &fabrics, &[]);
        assert_eq!(rows.len(), 2, "only the items using it");
        assert_eq!(
            rows[0].after,
            [(coral.color(), true), (Color::from("white"), false)]
        );
        assert_eq!(
            rows[1].after,
            [(coral.color(), false)],
            "already had coral: once"
        );
    }
}
