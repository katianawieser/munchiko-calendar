//! Event pages: the list of holidays, notable days and outings; one event's
//! page, with a "Plan a post" button; and the add/edit form for events you
//! enter yourself. Holidays from `holidays.toml` are read-only here: the
//! file is their source of truth, and the next sync would undo any edit.

use std::collections::HashMap;

use askama::Template;
use axum::Form;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use chrono::NaiveDate;

use super::blockers::{self, Blockers};
use super::forms::{
    Choice, EVENT_KINDS, FormData, NotADate, choices, join_list, label_of, parse_choice, split_list,
};
use super::ideas::{self, Ideas};
use super::{AppError, AppState, load_usage, render};
use crate::db::{DbError, NewEvent};
use crate::models::{Event, EventId, EventKind, PostPlan};

/// "Sat Oct 31, 2026" or "Sun Jul 26 – Sat Aug 1, 2026".
fn describe_when(event: &Event) -> String {
    match event.end {
        Some(end) if end != event.start => format!(
            "{} – {}",
            event.start.format("%a %b %-d"),
            end.format("%a %b %-d, %Y")
        ),
        _ => event.start.format("%a %b %-d, %Y").to_string(),
    }
}

/// CSS class for an event kind; matches the calendar colors.
fn kind_class(kind: EventKind) -> &'static str {
    match kind {
        EventKind::Holiday => "holiday",
        EventKind::NotableDay => "notable",
        EventKind::Seasonal => "seasonal",
        EventKind::Outing => "outing",
    }
}

/// A link to a post planned for an event.
struct PostLink {
    href: String,
    label: String,
}

fn post_links(plans: &[&PostPlan]) -> Vec<PostLink> {
    plans
        .iter()
        .map(|p| PostLink {
            href: format!("/posts/{}", p.id.0),
            label: format!("Post {}", p.post_date.format("%b %-d")),
        })
        .collect()
}

// ---------------------------------------------------------------------------
// /events
// ---------------------------------------------------------------------------

struct EventRow {
    href: String,
    when: String,
    name: String,
    kind: &'static str,
    kind_class: &'static str,
    posts: Vec<PostLink>,
    plan_href: String,
}

#[derive(Template)]
#[template(path = "events.html")]
struct EventsPage {
    showing_past: bool,
    kind_filter: Vec<Choice>,
    rows: Vec<EventRow>,
}

pub async fn list(
    State(state): State<AppState>,
    Query(query): Query<HashMap<String, String>>,
) -> Result<Response, AppError> {
    let today = state.today();
    let showing_past = query.get("show").is_some_and(|v| v == "past");
    let kind = query
        .get("kind")
        .and_then(|v| parse_choice(&EVENT_KINDS, v));

    let plans = state.db.list_post_plans().await?;
    let mut events: Vec<Event> = state
        .db
        .list_events()
        .await?
        .into_iter()
        .filter(|e| (e.end_date() < today) == showing_past)
        .filter(|e| kind.is_none_or(|k| e.kind == k))
        .collect();
    if showing_past {
        events.reverse(); // most recent first
    }

    let rows = events
        .iter()
        .map(|e| {
            let planned: Vec<&PostPlan> = plans.iter().filter(|p| p.event_id == e.id).collect();
            EventRow {
                href: format!("/events/{}", e.id.0),
                when: describe_when(e),
                name: e.name.clone(),
                kind: label_of(&EVENT_KINDS, e.kind),
                kind_class: kind_class(e.kind),
                posts: post_links(&planned),
                plan_href: format!("/posts/new?event={}", e.id.0),
            }
        })
        .collect();

    let mut kind_filter = vec![Choice::new("", "All kinds", kind.is_none())];
    for (k, value, label) in EVENT_KINDS {
        kind_filter.push(Choice::new(value, label, kind == Some(k)));
    }
    Ok(render(EventsPage {
        showing_past,
        kind_filter,
        rows,
    })?
    .into_response())
}

// ---------------------------------------------------------------------------
// /events/{id}
// ---------------------------------------------------------------------------

#[derive(Template)]
#[template(path = "event.html")]
struct EventPage {
    name: String,
    when: String,
    kind: &'static str,
    kind_class: &'static str,
    description: String,
    keywords: String,
    from_holidays_file: bool,
    edit_href: String,
    plan_href: String,
    posts: Vec<PostLink>,
    ideas: Option<Ideas>,
}

pub async fn show(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Response, AppError> {
    let event = state.db.get_event(EventId(id)).await?;
    let plans = state.db.list_post_plans().await?;
    let planned: Vec<&PostPlan> = plans.iter().filter(|p| p.event_id == event.id).collect();
    let accessories = state.db.list_accessories().await?;
    let fabrics = state.db.list_fabrics().await?;
    // Ideas for a post on the day itself, the usual case.
    let ideas = ideas::for_event(
        &event,
        event.start,
        state.today(),
        &accessories,
        &fabrics,
        &state.lead_times,
    );
    Ok(render(EventPage {
        name: event.name.clone(),
        when: describe_when(&event),
        kind: label_of(&EVENT_KINDS, event.kind),
        kind_class: kind_class(event.kind),
        description: event.description.clone(),
        keywords: join_list(&event.keywords),
        from_holidays_file: event.from_holidays_file,
        edit_href: format!("/events/{}/edit", event.id.0),
        plan_href: format!("/posts/new?event={}", event.id.0),
        posts: post_links(&planned),
        ideas: Some(ideas),
    })?
    .into_response())
}

// ---------------------------------------------------------------------------
// The add / edit form (your own events only)
// ---------------------------------------------------------------------------

#[derive(Template)]
#[template(path = "event_form.html")]
struct EventFormPage {
    heading: String,
    action: String,
    cancel_href: String,
    delete_action: Option<String>,
    name: String,
    kind_choices: Vec<Choice>,
    start: String,
    end: String,
    description: String,
    keywords: String,
    blocked: Blockers,
    errors: Vec<String>,
}

fn date_value(date: Option<NaiveDate>) -> String {
    date.map(|d| d.format("%Y-%m-%d").to_string())
        .unwrap_or_default()
}

impl EventFormPage {
    fn new(editing: Option<&Event>) -> Self {
        match editing {
            Some(e) => Self {
                heading: format!("Edit {}", e.name),
                action: format!("/events/{}", e.id.0),
                cancel_href: format!("/events/{}", e.id.0),
                delete_action: Some(format!("/events/{}/delete", e.id.0)),
                name: e.name.clone(),
                kind_choices: choices(&EVENT_KINDS, e.kind),
                start: date_value(Some(e.start)),
                end: date_value(e.end),
                description: e.description.clone(),
                keywords: join_list(&e.keywords),
                blocked: Blockers::default(),
                errors: Vec::new(),
            },
            None => Self {
                heading: "New event".into(),
                action: "/events".into(),
                cancel_href: "/events".into(),
                delete_action: None,
                name: String::new(),
                kind_choices: choices(&EVENT_KINDS, EventKind::Outing),
                start: String::new(),
                end: String::new(),
                description: String::new(),
                keywords: String::new(),
                blocked: Blockers::default(),
                errors: Vec::new(),
            },
        }
    }

    fn refill(mut self, form: &FormData, errors: Vec<String>) -> Self {
        let kind = form.text("kind");
        for choice in &mut self.kind_choices {
            choice.selected = choice.value == kind;
        }
        self.name = form.text("name");
        self.start = form.text("start");
        self.end = form.text("end");
        self.description = form.text("description");
        self.keywords = form.text("keywords");
        self.errors = errors;
        self
    }
}

fn read_form(form: &FormData) -> Result<NewEvent, Vec<String>> {
    let mut errors = Vec::new();
    let name = form.text("name");
    if name.is_empty() {
        errors.push("Give the event a name.".to_string());
    }
    let kind = parse_choice(&EVENT_KINDS, &form.text("kind"));
    if kind.is_none() {
        errors.push("Choose what kind of event it is.".to_string());
    }
    let start = match form.optional_date("start") {
        Ok(Some(date)) => Some(date),
        Ok(None) => {
            errors.push("Choose the date it starts.".to_string());
            None
        }
        Err(NotADate) => {
            errors.push("The start date isn't a real date.".to_string());
            None
        }
    };
    let end = match form.optional_date("end") {
        Ok(end) => end,
        Err(NotADate) => {
            errors.push("The end date isn't a real date.".to_string());
            None
        }
    };
    if let (Some(start), Some(end)) = (start, end) {
        if end < start {
            errors.push("The end date can't be before the start date.".to_string());
        }
    }

    match (kind, start) {
        (Some(kind), Some(start)) if errors.is_empty() => Ok(NewEvent {
            name,
            description: form.text("description"),
            kind,
            start,
            // Ending on the day it starts is just a one-day event.
            end: end.filter(|e| *e != start),
            keywords: split_list(&form.text("keywords")),
        }),
        _ => Err(errors),
    }
}

fn form_response(status: StatusCode, page: EventFormPage) -> Result<Response, AppError> {
    Ok((status, render(page)?).into_response())
}

/// Load an event for editing; holidays can't be edited here.
async fn editable_event(state: &AppState, id: i64) -> Result<Event, AppError> {
    let event = state.db.get_event(EventId(id)).await?;
    if event.from_holidays_file {
        return Err(AppError::ReadOnly);
    }
    Ok(event)
}

/// The edit form for `event`, including what would block deleting it.
async fn edit_page(state: &AppState, event: &Event) -> Result<EventFormPage, AppError> {
    let mut page = EventFormPage::new(Some(event));
    page.blocked = blockers::for_event(event.id, &load_usage(state).await?);
    Ok(page)
}

pub async fn new_form() -> Result<Response, AppError> {
    form_response(StatusCode::OK, EventFormPage::new(None))
}

pub async fn create(
    State(state): State<AppState>,
    Form(pairs): Form<Vec<(String, String)>>,
) -> Result<Response, AppError> {
    let form = FormData::new(pairs);
    match read_form(&form) {
        Ok(new) => {
            let id = state.db.create_event(&new).await?;
            Ok(Redirect::to(&format!("/events/{}", id.0)).into_response())
        }
        Err(errors) => form_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            EventFormPage::new(None).refill(&form, errors),
        ),
    }
}

pub async fn edit_form(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Response, AppError> {
    let event = editable_event(&state, id).await?;
    form_response(StatusCode::OK, edit_page(&state, &event).await?)
}

pub async fn update(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Form(pairs): Form<Vec<(String, String)>>,
) -> Result<Response, AppError> {
    let form = FormData::new(pairs);
    let event = editable_event(&state, id).await?;
    match read_form(&form) {
        Ok(new) => {
            state.db.update_event(event.id, &new).await?;
            Ok(Redirect::to(&format!("/events/{id}")).into_response())
        }
        Err(errors) => form_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            edit_page(&state, &event).await?.refill(&form, errors),
        ),
    }
}

pub async fn delete(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Response, AppError> {
    let event = editable_event(&state, id).await?;
    match state.db.delete_event(event.id).await {
        Ok(()) => Ok(Redirect::to("/events").into_response()),
        Err(DbError::InUse { .. }) => {
            let mut page = edit_page(&state, &event).await?;
            page.errors = vec![format!(
                "{} is still in use, so it can't be deleted yet.",
                event.name
            )];
            form_response(StatusCode::CONFLICT, page)
        }
        Err(e) => Err(e.into()),
    }
}
