//! Series pages: several posts from one setup and one photo shoot.
//!
//! A series page shows the shared setup (backdrop), its posts in date order
//! and the one prep timeline they share. A series can be started here
//! ("+ New series", then add posts) or from any post ("+ Another post from
//! this shoot", in `posts.rs`).

use askama::Template;
use axum::Form;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};

use super::forms::FormData;
use super::posts::{TaskRow, task_rows};
use super::{AppError, AppState, load_posts, render};
use crate::db::NewSeries;
use crate::models::{Pet, Series, SeriesId};

// ---------------------------------------------------------------------------
// /series/{id}
// ---------------------------------------------------------------------------

/// One post of the series, as a table row.
struct MemberRow {
    href: String,
    name: String,
    /// "Tue Apr 20, 2027" or "no date yet".
    when: String,
    /// "Momo", "Mochi", "Mochi & Momo", or "" if no looks chosen yet.
    who: String,
}

#[derive(Template)]
#[template(path = "series.html")]
struct SeriesPage {
    title: String,
    backdrop: String,
    notes: String,
    posts: Vec<MemberRow>,
    /// The shared prep timeline, once a post has a date.
    tasks: Vec<TaskRow>,
    /// When no post has a date yet: what's still to do, and the soonest the
    /// first post could go up.
    readiness: Option<String>,
    /// Why there's no timeline, if planning failed (e.g. a missing accessory).
    problem: Option<String>,
    add_href: String,
    edit_href: String,
    delete_action: String,
}

fn who(looks: &[crate::models::Look]) -> String {
    let in_it = |pet| looks.iter().any(|l| l.pet == pet);
    match (in_it(Pet::Mochi), in_it(Pet::Momo)) {
        (true, true) => "Mochi & Momo".into(),
        (true, false) => "Mochi".into(),
        (false, true) => "Momo".into(),
        (false, false) => String::new(),
    }
}

pub async fn show(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Response, AppError> {
    let series = state.db.get_series(SeriesId(id)).await?;
    let today = state.today();
    let (posts, _) = load_posts(&state).await?;
    let members: Vec<_> = posts
        .iter()
        .filter(|p| p.plan.series_id == Some(series.id))
        .collect();

    let rows = members
        .iter()
        .map(|p| MemberRow {
            href: format!("/posts/{}", p.plan.id.0),
            name: p.name.clone(),
            when: p.plan.post_date.map_or("no date yet".into(), |d| {
                d.format("%a %b %-d, %Y").to_string()
            }),
            who: who(&p.plan.looks),
        })
        .collect();

    // Every dated member carries the same shared timeline; any one will do.
    let (mut tasks, mut readiness, mut problem) = (Vec::new(), None, None);
    match members.iter().find(|p| p.plan.post_date.is_some()) {
        Some(dated) => match &dated.timeline {
            Ok(timeline) => tasks = task_rows(timeline, today),
            Err(e) => problem = Some(format!("Can't plan: {e}")),
        },
        // No dates yet: every member carries the series' readiness.
        None => match members.first().and_then(|p| p.readiness.as_ref()) {
            Some(Ok(r)) => {
                readiness = Some(format!(
                    "{} If prep starts today, the first post could go up on {}.",
                    if r.everything_to_wear_is_ready() {
                        "Everything to wear is ready."
                    } else {
                        "Some looks still need prep."
                    },
                    r.earliest_post_date(today).format("%a %b %-d, %Y")
                ));
            }
            Some(Err(e)) => problem = Some(format!("Can't plan: {e}")),
            None => {}
        },
    }

    Ok(render(SeriesPage {
        title: series.title,
        backdrop: series.backdrop,
        notes: series.notes,
        posts: rows,
        tasks,
        readiness,
        problem,
        add_href: format!("/posts/new?series={id}"),
        edit_href: format!("/series/{id}/edit"),
        delete_action: format!("/series/{id}/delete"),
    })?
    .into_response())
}

// ---------------------------------------------------------------------------
// The new / edit form
// ---------------------------------------------------------------------------

#[derive(Template)]
#[template(path = "series_form.html")]
struct SeriesFormPage {
    heading: String,
    action: String,
    cancel_href: String,
    title: String,
    backdrop: String,
    notes: String,
    errors: Vec<String>,
}

impl SeriesFormPage {
    fn new(editing: Option<&Series>) -> Self {
        Self {
            heading: editing.map_or("New series".into(), |s| format!("Edit {}", s.title)),
            action: editing.map_or("/series".into(), |s| format!("/series/{}", s.id.0)),
            cancel_href: editing.map_or("/posts".into(), |s| format!("/series/{}", s.id.0)),
            title: editing.map(|s| s.title.clone()).unwrap_or_default(),
            backdrop: editing.map(|s| s.backdrop.clone()).unwrap_or_default(),
            notes: editing.map(|s| s.notes.clone()).unwrap_or_default(),
            errors: Vec::new(),
        }
    }

    fn refill(mut self, form: &FormData, errors: Vec<String>) -> Self {
        self.title = form.text("title");
        self.backdrop = form.text("backdrop");
        self.notes = form.text("notes");
        self.errors = errors;
        self
    }
}

fn read_form(form: &FormData) -> Result<NewSeries, Vec<String>> {
    let title = form.text("title");
    if title.is_empty() {
        return Err(vec!["Give the series a name.".to_string()]);
    }
    Ok(NewSeries {
        title,
        backdrop: form.text("backdrop"),
        notes: form.text("notes"),
    })
}

fn form_response(status: StatusCode, page: SeriesFormPage) -> Result<Response, AppError> {
    Ok((status, render(page)?).into_response())
}

pub async fn new_form() -> Result<Response, AppError> {
    form_response(StatusCode::OK, SeriesFormPage::new(None))
}

pub async fn create(
    State(state): State<AppState>,
    Form(pairs): Form<Vec<(String, String)>>,
) -> Result<Response, AppError> {
    let form = FormData::new(pairs);
    match read_form(&form) {
        Ok(new) => {
            let id = state.db.create_series(&new).await?;
            Ok(Redirect::to(&format!("/series/{}", id.0)).into_response())
        }
        Err(errors) => form_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            SeriesFormPage::new(None).refill(&form, errors),
        ),
    }
}

pub async fn edit_form(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Response, AppError> {
    let series = state.db.get_series(SeriesId(id)).await?;
    form_response(StatusCode::OK, SeriesFormPage::new(Some(&series)))
}

pub async fn update(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Form(pairs): Form<Vec<(String, String)>>,
) -> Result<Response, AppError> {
    let form = FormData::new(pairs);
    let series = state.db.get_series(SeriesId(id)).await?;
    match read_form(&form) {
        Ok(new) => {
            state.db.update_series(series.id, &new).await?;
            Ok(Redirect::to(&format!("/series/{id}")).into_response())
        }
        Err(errors) => form_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            SeriesFormPage::new(Some(&series)).refill(&form, errors),
        ),
    }
}

/// Delete the series but keep its posts (see `Repository::delete_series`).
pub async fn delete(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Redirect, AppError> {
    state.db.delete_series(SeriesId(id)).await?;
    Ok(Redirect::to("/posts"))
}
