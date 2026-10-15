//! Post pages: the list of planned posts, one post's prep timeline, and the
//! form for planning a post (which event, when, and who wears what).

use std::collections::HashMap;

use askama::Template;
use axum::Form;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Redirect, Response};
use chrono::NaiveDate;

use super::forms::{Choice, FormData, KINDS, NotADate, PETS, STATUSES, label_of};
use super::ideas::{self, Ideas};
use super::photos::{self, PhotoSection};
use super::{AppError, AppState, LoadedPost, load_posts, render};
use crate::db::NewPostPlan;
use crate::models::{
    Accessory, AccessoryId, Event, EventId, Fabric, Look, Pet, PhotoOwner, PostPlan, PostPlanId,
};
use crate::planner::{TaskStatus, Timeline};

/// How a post is doing, as words plus a CSS class for the color.
struct Health {
    text: String,
    class: &'static str,
}

fn health(post: &LoadedPost, today: NaiveDate) -> Health {
    let timeline = match &post.timeline {
        Ok(t) => t,
        Err(e) => {
            return Health {
                text: format!("Can't plan: {e}"),
                class: "bad",
            };
        }
    };
    if timeline.post_date < today {
        return Health {
            text: "Published".into(),
            class: "done",
        };
    }
    match timeline.slack_days(today) {
        s if s > 0 => Health {
            text: format!("On track: {s} days before prep starts"),
            class: "good",
        },
        0 => Health {
            text: "Prep starts today".into(),
            class: "warn",
        },
        // Negative slack is normal once prep is underway; it only means
        // trouble if some task is actually overdue.
        _ if has_overdue_task(timeline, today) => Health {
            text: "Behind schedule".into(),
            class: "bad",
        },
        _ => Health {
            text: "Prep in progress".into(),
            class: "warn",
        },
    }
}

fn has_overdue_task(timeline: &Timeline, today: NaiveDate) -> bool {
    timeline
        .tasks
        .iter()
        .any(|t| matches!(t.status(today), TaskStatus::Overdue { .. }))
}

fn date_long(date: NaiveDate) -> String {
    date.format("%a %b %-d, %Y").to_string()
}

// ---------------------------------------------------------------------------
// /posts
// ---------------------------------------------------------------------------

struct PostRow {
    href: String,
    event_name: String,
    post_date: String,
    health: Health,
}

#[derive(Template)]
#[template(path = "posts.html")]
struct PostsPage {
    upcoming: Vec<PostRow>,
    past: Vec<PostRow>,
}

pub async fn list(State(state): State<AppState>) -> Result<Html<String>, AppError> {
    let today = state.today();
    let (posts, _accessories) = load_posts(&state).await?;

    let (mut upcoming, mut past) = (Vec::new(), Vec::new());
    for post in &posts {
        let row = PostRow {
            href: format!("/posts/{}", post.plan.id.0),
            event_name: post.event.name.clone(),
            post_date: date_long(post.plan.post_date),
            health: health(post, today),
        };
        if post.plan.post_date < today {
            past.push(row);
        } else {
            upcoming.push(row);
        }
    }
    // Most recent first for history.
    past.reverse();
    render(PostsPage { upcoming, past })
}

// ---------------------------------------------------------------------------
// /posts/{id}
// ---------------------------------------------------------------------------

struct LookRow {
    pet: String,
    items: String,
}

struct TaskRow {
    when: String,
    title: String,
    status: String,
    class: &'static str,
}

#[derive(Template)]
#[template(path = "post.html")]
struct PostPage {
    event_name: String,
    event_href: String,
    edit_href: String,
    delete_action: String,
    post_date: String,
    shoot_date: Option<String>,
    health: Health,
    looks: Vec<LookRow>,
    backdrop: String,
    notes: String,
    tasks: Vec<TaskRow>,
    photos: PhotoSection,
}

/// "Pumpkin bow, Ghost scarf" for each pet in the post.
fn look_rows(post: &LoadedPost, accessories: &[Accessory]) -> Vec<LookRow> {
    [Pet::Mochi, Pet::Momo]
        .into_iter()
        .filter_map(|pet| {
            let items: Vec<String> = post
                .plan
                .looks
                .iter()
                .filter(|look| look.pet == pet)
                .map(|look| {
                    accessories
                        .iter()
                        .find(|a| a.id == look.accessory_id)
                        .map(|a| a.name.clone())
                        .unwrap_or_else(|| format!("missing accessory #{}", look.accessory_id.0))
                })
                .collect();
            (!items.is_empty()).then(|| LookRow {
                pet: pet.to_string(),
                items: items.join(", "),
            })
        })
        .collect()
}

pub async fn show(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Html<String>, AppError> {
    let today = state.today();
    let (posts, accessories) = load_posts(&state).await?;
    let post = posts
        .iter()
        .find(|p| p.plan.id.0 == id)
        .ok_or(AppError::NotFound)?;
    let finished = post.plan.post_date < today;

    let tasks = match &post.timeline {
        Ok(timeline) => timeline
            .tasks
            .iter()
            .map(|task| {
                let when = if task.start == task.due {
                    task.due.format("%a %b %-d").to_string()
                } else {
                    format!(
                        "{} – {}",
                        task.start.format("%a %b %-d"),
                        task.due.format("%a %b %-d")
                    )
                };
                let (status, class) = match task.status(today) {
                    _ if finished => ("Done".to_string(), "done"),
                    TaskStatus::Upcoming { starts_in_days } => {
                        (format!("in {starts_in_days}d"), "upcoming")
                    }
                    TaskStatus::Active => ("Now".to_string(), "active"),
                    TaskStatus::Overdue { days_late } => (format!("{days_late}d late"), "overdue"),
                };
                TaskRow {
                    when,
                    title: task.title.clone(),
                    status,
                    class,
                }
            })
            .collect(),
        Err(_) => Vec::new(),
    };

    let photos = PhotoSection::load(
        &state,
        PhotoOwner::PostPlan(post.plan.id),
        &format!("the {} post", post.event.name),
    )
    .await?;

    render(PostPage {
        event_name: post.event.name.clone(),
        event_href: format!("/events/{}", post.event.id.0),
        edit_href: format!("/posts/{id}/edit"),
        delete_action: format!("/posts/{id}/delete"),
        post_date: date_long(post.plan.post_date),
        shoot_date: post.timeline.as_ref().ok().map(|t| date_long(t.shoot_date)),
        health: health(post, today),
        looks: look_rows(post, &accessories),
        backdrop: post.plan.backdrop.clone(),
        notes: post.plan.notes.clone(),
        tasks,
        photos,
    })
}

// ---------------------------------------------------------------------------
// The plan-a-post form
// ---------------------------------------------------------------------------

/// One pet's checkboxes: everything they could wear.
struct PetLooks {
    label: &'static str,
    field: &'static str,
    choices: Vec<Choice>,
}

#[derive(Template)]
#[template(path = "post_form.html")]
struct PostFormPage {
    heading: String,
    action: String,
    cancel_href: String,
    event_choices: Vec<Choice>,
    post_date: String,
    pets: Vec<PetLooks>,
    backdrop: String,
    notes: String,
    /// Suggestions for the chosen event, shown above the checkboxes.
    ideas: Option<Ideas>,
    errors: Vec<String>,
}

/// Events a post can be for: anything not yet over, plus the post's current
/// event even if it's in the past (so editing an old post still works).
fn event_choices(events: &[Event], today: NaiveDate, selected: Option<EventId>) -> Vec<Choice> {
    let mut list = vec![Choice::new("", "— choose an event —", selected.is_none())];
    for e in events
        .iter()
        .filter(|e| e.end_date() >= today || Some(e.id) == selected)
    {
        let label = format!("{} · {}", e.start.format("%a %b %-d, %Y"), e.name);
        list.push(Choice::new(e.id.0, label, Some(e.id) == selected));
    }
    list
}

/// "Pumpkin bow (bow, idea)" checkboxes for each pet.
fn pet_looks(accessories: &[Accessory], looks: &[Look]) -> Vec<PetLooks> {
    PETS.iter()
        .map(|(pet, field, label)| PetLooks {
            label,
            field,
            choices: accessories
                .iter()
                .map(|a| {
                    let text = format!(
                        "{} ({}, {})",
                        a.name,
                        label_of(&KINDS, a.kind).to_lowercase(),
                        label_of(&STATUSES, a.status).to_lowercase()
                    );
                    let worn = looks
                        .iter()
                        .any(|l| l.pet == *pet && l.accessory_id == a.id);
                    Choice::new(a.id.0, text, worn)
                })
                .collect(),
        })
        .collect()
}

impl PostFormPage {
    fn new(
        editing: Option<&PostPlan>,
        preselected_event: Option<&Event>,
        events: &[Event],
        accessories: &[Accessory],
        today: NaiveDate,
    ) -> Self {
        let event_id = editing
            .map(|p| p.event_id)
            .or(preselected_event.map(|e| e.id));
        // A new post defaults to the day of its event.
        let post_date = editing
            .map(|p| p.post_date)
            .or(preselected_event.map(|e| e.start));
        Self {
            heading: match (editing, preselected_event) {
                (Some(_), _) => "Edit post".into(),
                (None, Some(e)) => format!("Plan a post for {}", e.name),
                (None, None) => "Plan a post".into(),
            },
            action: editing.map_or("/posts".into(), |p| format!("/posts/{}", p.id.0)),
            cancel_href: editing.map_or("/posts".into(), |p| format!("/posts/{}", p.id.0)),
            event_choices: event_choices(events, today, event_id),
            post_date: post_date
                .map(|d| d.format("%Y-%m-%d").to_string())
                .unwrap_or_default(),
            pets: pet_looks(accessories, editing.map_or(&[], |p| &p.looks)),
            backdrop: editing.map(|p| p.backdrop.clone()).unwrap_or_default(),
            notes: editing.map(|p| p.notes.clone()).unwrap_or_default(),
            ideas: None,
            errors: Vec::new(),
        }
    }

    fn refill(mut self, form: &FormData, errors: Vec<String>) -> Self {
        let event = form.text("event");
        for choice in &mut self.event_choices {
            choice.selected = choice.value == event;
        }
        for pet in &mut self.pets {
            let ticked: Vec<&str> = form.all(pet.field).collect();
            for choice in &mut pet.choices {
                choice.selected = ticked.contains(&choice.value.as_str());
            }
        }
        self.post_date = form.text("post_date");
        self.backdrop = form.text("backdrop");
        self.notes = form.text("notes");
        self.errors = errors;
        self
    }
}

fn read_form(
    form: &FormData,
    events: &[Event],
    accessories: &[Accessory],
) -> Result<NewPostPlan, Vec<String>> {
    let mut errors = Vec::new();

    let event = match form.optional_id("event") {
        Ok(Some(id)) => events.iter().find(|e| e.id.0 == id),
        _ => None,
    };
    if event.is_none() {
        errors.push("Choose which event the post is for.".to_string());
    }

    // Left empty, the post goes up on the day of the event.
    let post_date = match form.optional_date("post_date") {
        Ok(date) => date.or(event.map(|e| e.start)),
        Err(NotADate) => {
            errors.push("The post date isn't a real date.".to_string());
            None
        }
    };

    let mut looks = Vec::new();
    for (pet, field, label) in PETS {
        for value in form.all(field) {
            match value
                .parse::<i64>()
                .ok()
                .filter(|id| accessories.iter().any(|a| a.id.0 == *id))
            {
                Some(id) => looks.push(Look {
                    pet,
                    accessory_id: AccessoryId(id),
                }),
                None => errors.push(format!("One of {label}'s items isn't in the inventory.")),
            }
        }
    }

    match (event, post_date) {
        (Some(event), Some(post_date)) if errors.is_empty() => Ok(NewPostPlan {
            event_id: event.id,
            post_date,
            looks,
            backdrop: form.text("backdrop"),
            notes: form.text("notes"),
        }),
        _ => Err(errors),
    }
}

/// Ideas for the event the form is showing, if one is chosen. Without
/// JavaScript the box can't follow the dropdown live; it reflects the event
/// the page was opened (or last submitted) with.
fn form_ideas(
    state: &AppState,
    event: Option<&Event>,
    post_date: Option<NaiveDate>,
    accessories: &[Accessory],
    fabrics: &[Fabric],
) -> Option<Ideas> {
    event.map(|e| {
        ideas::for_event(
            e,
            post_date.unwrap_or(e.start),
            state.today(),
            accessories,
            fabrics,
            &state.lead_times,
        )
    })
}

/// The event and date a submitted form asked for, even if the form had
/// mistakes, so the ideas box can still be shown.
fn submitted_event<'a>(
    form: &FormData,
    events: &'a [Event],
) -> (Option<&'a Event>, Option<NaiveDate>) {
    let event = form
        .optional_id("event")
        .ok()
        .flatten()
        .and_then(|id| events.iter().find(|e| e.id.0 == id));
    (event, form.optional_date("post_date").ok().flatten())
}

fn form_response(status: StatusCode, page: PostFormPage) -> Result<Response, AppError> {
    Ok((status, render(page)?).into_response())
}

/// `/posts/new`, optionally `?event=12` from an event's "Plan a post" button.
pub async fn new_form(
    State(state): State<AppState>,
    Query(query): Query<HashMap<String, String>>,
) -> Result<Response, AppError> {
    let events = state.db.list_events().await?;
    let accessories = state.db.list_accessories().await?;
    let preselected = query
        .get("event")
        .and_then(|v| v.parse::<i64>().ok())
        .and_then(|id| events.iter().find(|e| e.id.0 == id));
    let fabrics = state.db.list_fabrics().await?;
    let mut page = PostFormPage::new(None, preselected, &events, &accessories, state.today());
    page.ideas = form_ideas(&state, preselected, None, &accessories, &fabrics);
    form_response(StatusCode::OK, page)
}

pub async fn create(
    State(state): State<AppState>,
    Form(pairs): Form<Vec<(String, String)>>,
) -> Result<Response, AppError> {
    let form = FormData::new(pairs);
    let events = state.db.list_events().await?;
    let accessories = state.db.list_accessories().await?;
    match read_form(&form, &events, &accessories) {
        Ok(new) => {
            let id = state.db.create_post_plan(&new).await?;
            // Straight to the new post's timeline: that's the payoff.
            Ok(Redirect::to(&format!("/posts/{}", id.0)).into_response())
        }
        Err(errors) => {
            let fabrics = state.db.list_fabrics().await?;
            let (event, date) = submitted_event(&form, &events);
            let mut page = PostFormPage::new(None, None, &events, &accessories, state.today())
                .refill(&form, errors);
            page.ideas = form_ideas(&state, event, date, &accessories, &fabrics);
            form_response(StatusCode::UNPROCESSABLE_ENTITY, page)
        }
    }
}

pub async fn edit_form(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Response, AppError> {
    let plan = state.db.get_post_plan(PostPlanId(id)).await?;
    let events = state.db.list_events().await?;
    let accessories = state.db.list_accessories().await?;
    let fabrics = state.db.list_fabrics().await?;
    let event = events.iter().find(|e| e.id == plan.event_id);
    let mut page = PostFormPage::new(Some(&plan), None, &events, &accessories, state.today());
    page.ideas = form_ideas(&state, event, Some(plan.post_date), &accessories, &fabrics);
    form_response(StatusCode::OK, page)
}

pub async fn update(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Form(pairs): Form<Vec<(String, String)>>,
) -> Result<Response, AppError> {
    let form = FormData::new(pairs);
    let plan = state.db.get_post_plan(PostPlanId(id)).await?;
    let events = state.db.list_events().await?;
    let accessories = state.db.list_accessories().await?;
    match read_form(&form, &events, &accessories) {
        Ok(new) => {
            state.db.update_post_plan(plan.id, &new).await?;
            Ok(Redirect::to(&format!("/posts/{id}")).into_response())
        }
        Err(errors) => {
            let fabrics = state.db.list_fabrics().await?;
            let (event, date) = submitted_event(&form, &events);
            let mut page =
                PostFormPage::new(Some(&plan), None, &events, &accessories, state.today())
                    .refill(&form, errors);
            page.ideas = form_ideas(&state, event, date, &accessories, &fabrics);
            form_response(StatusCode::UNPROCESSABLE_ENTITY, page)
        }
    }
}

pub async fn delete(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Redirect, AppError> {
    // Note the photos first: deleting the plan deletes their records.
    let photo_files = state
        .db
        .list_photos(PhotoOwner::PostPlan(PostPlanId(id)))
        .await?;
    state.db.delete_post_plan(PostPlanId(id)).await?;
    photos::remove_files(&state, &photo_files).await;
    Ok(Redirect::to("/posts"))
}
