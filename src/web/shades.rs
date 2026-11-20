//! The Shades page: near-duplicate shades in the stash, and merging them
//! (M13c). The grouping and the merge itself are worked out in
//! [`crate::shades`]; this module shows them and saves the result.
//!
//! - `GET /shades`: groups with more than one shade. `view=alike` (look-alikes,
//!   with `closeness=tight|medium|loose`) or `view=name` (one name, several
//!   shades).
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
use crate::shades::{self, Closeness, MergePlan, Owner, Shade, ShadeGroup};

const CLOSENESS: [(Closeness, &str, &str); 3] = [
    (Closeness::Tight, "tight", "Tight"),
    (Closeness::Medium, "medium", "Medium"),
    (Closeness::Loose, "loose", "Loose"),
];

/// Which list the page shows, from the address.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Choice {
    by_name: bool,
    closeness: Closeness,
}

impl Choice {
    fn read(form: &FormData) -> Self {
        Self {
            by_name: form.text("view") == "name",
            closeness: CLOSENESS
                .iter()
                .find(|(_, value, _)| *value == form.text("closeness"))
                .map_or(Closeness::Medium, |(c, _, _)| *c),
        }
    }

    fn closeness_value(self) -> &'static str {
        CLOSENESS
            .iter()
            .find(|(c, _, _)| *c == self.closeness)
            .unwrap()
            .1
    }

    /// The address of the list with these choices.
    fn href(self) -> String {
        if self.by_name {
            "/shades?view=name".into()
        } else {
            format!("/shades?view=alike&closeness={}", self.closeness_value())
        }
    }
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
    /// Other names it's used under ("also: charcoal").
    other_names: String,
    uses: usize,
    users: Vec<Link>,
    /// The radio starts on the most used shade.
    keep: bool,
}

struct GroupView {
    /// "black" for a same-name group; empty for look-alikes.
    name: String,
    uses: usize,
    shades: Vec<ShadeView>,
}

/// A view switch link (Look-alikes / Same name, Tight / Medium / Loose).
struct Tab {
    label: &'static str,
    href: String,
    current: bool,
}

#[derive(Template)]
#[template(path = "shades.html")]
struct ShadesPage {
    by_name: bool,
    view: &'static str,
    closeness: &'static str,
    total_shades: usize,
    total_groups: usize,
    groups: Vec<GroupView>,
    view_tabs: Vec<Tab>,
    closeness_tabs: Vec<Tab>,
    /// After a merge: "Merged: 9 colors on 7 items. Backup: ...".
    notice: Option<String>,
}

fn group_view(group: &ShadeGroup, fabrics: &[Fabric], accessories: &[Accessory]) -> GroupView {
    GroupView {
        name: group.name.clone().unwrap_or_default(),
        uses: group.use_count(),
        shades: group
            .shades
            .iter()
            .enumerate()
            .map(|(i, shade)| ShadeView {
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
                keep: i == 0,
            })
            .collect(),
    }
}

pub async fn list(
    State(state): State<AppState>,
    Query(query): Query<Vec<(String, String)>>,
) -> Result<Response, AppError> {
    let form = FormData::new(query);
    let choice = Choice::read(&form);
    let fabrics = state.db.list_fabrics().await?;
    let accessories = state.db.list_accessories().await?;
    let all = shades::collect(&fabrics, &accessories);
    let groups = if choice.by_name {
        shades::name_groups(&all)
    } else {
        shades::look_alike_groups(&all, choice.closeness)
    };
    let total_groups = groups.len();
    // Only groups with something to merge; the biggest first.
    let mut groups: Vec<&ShadeGroup> = groups.iter().filter(|g| g.shades.len() > 1).collect();
    groups.sort_by_key(|g| std::cmp::Reverse(g.shades.len()));

    let notice = (!form.text("merged").is_empty()).then(|| {
        format!(
            "Merged: {} changed. A copy of the database from before was saved as backups/{}.",
            form.text("merged"),
            form.text("backup")
        )
    });
    let tab = |label, by_name, closeness, current| Tab {
        label,
        href: Choice { by_name, closeness }.href(),
        current,
    };
    let view_tabs = vec![
        tab("Look-alikes", false, choice.closeness, !choice.by_name),
        tab("Same name", true, choice.closeness, choice.by_name),
    ];
    let closeness_tabs = CLOSENESS
        .iter()
        .map(|(c, _, label)| tab(label, false, *c, *c == choice.closeness))
        .collect();
    Ok(render(ShadesPage {
        by_name: choice.by_name,
        view: if choice.by_name { "name" } else { "alike" },
        closeness: choice.closeness_value(),
        total_shades: all.len(),
        total_groups,
        groups: groups
            .iter()
            .map(|g| group_view(g, &fabrics, &accessories))
            .collect(),
        view_tabs,
        closeness_tabs,
        notice,
    })?
    .into_response())
}

// ---------------------------------------------------------------------------
// The merge: preview, then apply
// ---------------------------------------------------------------------------

/// The merge the form asks for: the ticked shades (`from`), the one to keep
/// (`keep`) and, for a same-name group, the name (`name`). The kept shade's
/// name comes from the current data, not the page.
fn read_plan(form: &FormData, all: &[Shade]) -> Result<MergePlan, AppError> {
    let keep = Color::parse_hex(&form.text("keep")).ok_or(AppError::BadRequest)?;
    // The kept shade is part of the merge even if its box was unticked.
    let from: Vec<String> = form.all("from").filter_map(Color::parse_hex).collect();
    let only_name = Some(form.text("name")).filter(|n| !n.is_empty());
    let kept = all
        .iter()
        .find(|s| s.hex == keep)
        .ok_or(AppError::NotFound)?;
    let name = only_name.clone().unwrap_or_else(|| kept.name().to_string());
    Ok(MergePlan {
        from: from.into_iter().filter(|h| *h != keep).collect(),
        only_name,
        to: Color::with_hex(name, keep),
    })
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
    keep: Color,
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
    let plan = read_plan(&form, &all)?;
    let rows = change_rows(&plan, &fabrics, &accessories);
    Ok(render(MergePage {
        keep: plan.to.clone(),
        summary: format!(
            "{} on {}",
            plural(colors_changed(&rows), "color"),
            plural(rows.len(), "item")
        ),
        rows,
        fields: query,
        back: Choice::read(&form).href(),
    })?
    .into_response())
}

pub async fn apply(
    State(state): State<AppState>,
    Form(pairs): Form<Vec<(String, String)>>,
) -> Result<Response, AppError> {
    let form = FormData::new(pairs);
    let back = Choice::read(&form).href();
    let fabrics = state.db.list_fabrics().await?;
    let accessories = state.db.list_accessories().await?;
    let all = shades::collect(&fabrics, &accessories);
    let plan = read_plan(&form, &all)?;
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
