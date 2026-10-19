//! Post pages: the list of planned posts and ideas, one post's prep timeline
//! (or an idea's readiness), and the form for planning a post (what it's for,
//! when, and who wears what).
//!
//! An *idea* is a post without a date: the looks and backdrop can be planned,
//! and the page shows what's still to do and the soonest it could go up.
//! "Schedule it" gives it a date, and it becomes a normal post with a timeline.

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
    Series, SeriesId,
};
use crate::planner::{TaskKind, TaskStatus, Timeline};

/// How a post is doing, as words plus a CSS class for the color.
struct Health {
    text: String,
    class: &'static str,
}

fn health(post: &LoadedPost, today: NaiveDate) -> Health {
    if post.plan.is_idea() {
        let status = idea_status(post, today);
        return Health {
            text: match status.earliest {
                Some(date) => format!("Idea · could post from {}", date.format("%b %-d")),
                None => status.summary,
            },
            class: if status.earliest.is_some() {
                "upcoming"
            } else {
                "bad"
            },
        };
    }
    let timeline = match &post.timeline {
        Ok(t) => t,
        Err(e) => {
            return Health {
                text: format!("Can't plan: {e}"),
                class: "bad",
            };
        }
    };
    // The post's own date, not the timeline's: in a series the shared
    // timeline runs until the *last* post.
    if post.plan.post_date.is_some_and(|d| d < today) {
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

/// Where an idea stands: what's left to do and the soonest it could post.
struct IdeaStatus {
    /// "Everything to wear is ready" or "Still to do: Make Yellow Wheat (bow) for Momo".
    summary: String,
    /// `None` if it can't be planned (something it uses has gone missing).
    earliest: Option<NaiveDate>,
    /// Nothing to make, buy or order: only the backdrop and the shoot.
    ready: bool,
}

fn idea_status(post: &LoadedPost, today: NaiveDate) -> IdeaStatus {
    // In a series whose shoot is already planned, an undated post's photos
    // come from that shoot: it only needs a date, from the first post on.
    if let Some(first) = post.series.as_ref().and_then(|s| s.first_post) {
        return IdeaStatus {
            summary: "Photographed at the series' shoot; it just needs a date".into(),
            earliest: Some(first.max(today)),
            ready: true,
        };
    }
    match &post.readiness {
        Some(Ok(readiness)) => {
            let todo: Vec<&str> = readiness
                .prep
                .iter()
                .filter(|p| p.kind != TaskKind::BuildBackdrop)
                .map(|p| p.title.as_str())
                .collect();
            IdeaStatus {
                summary: if todo.is_empty() {
                    "Everything to wear is ready".into()
                } else {
                    format!("Still to do: {}", todo.join("; "))
                },
                earliest: Some(readiness.earliest_post_date(today)),
                ready: readiness.everything_to_wear_is_ready(),
            }
        }
        Some(Err(e)) => IdeaStatus {
            summary: format!("Can't plan: {e}"),
            earliest: None,
            ready: false,
        },
        // Only ideas have a readiness; a scheduled post never gets here.
        None => IdeaStatus {
            summary: String::new(),
            earliest: None,
            ready: false,
        },
    }
}

// ---------------------------------------------------------------------------
// /posts
// ---------------------------------------------------------------------------

/// "Earth Day series", for posts that are part of one.
fn series_label(post: &LoadedPost) -> Option<String> {
    post.series
        .as_ref()
        .map(|s| format!("{} series", s.series.title))
}

struct PostRow {
    href: String,
    name: String,
    /// "Earth Day series", if it's part of one.
    series: Option<String>,
    post_date: String,
    health: Health,
}

struct IdeaRow {
    href: String,
    name: String,
    series: Option<String>,
    /// The event it's linked to, if any.
    event_name: Option<String>,
    summary: String,
    /// "Nov 3", or empty if it can't be planned.
    earliest: String,
    ready: bool,
}

#[derive(Template)]
#[template(path = "posts.html")]
struct PostsPage {
    upcoming: Vec<PostRow>,
    past: Vec<PostRow>,
    ideas: Vec<IdeaRow>,
}

pub async fn list(State(state): State<AppState>) -> Result<Html<String>, AppError> {
    let today = state.today();
    let (posts, _accessories) = load_posts(&state).await?;

    let (mut upcoming, mut past) = (Vec::new(), Vec::new());
    let mut ideas: Vec<(IdeaStatus, IdeaRow)> = Vec::new();
    for post in &posts {
        let href = format!("/posts/{}", post.plan.id.0);
        let Some(post_date) = post.plan.post_date else {
            let status = idea_status(post, today);
            let row = IdeaRow {
                href,
                name: post.name.clone(),
                series: series_label(post),
                event_name: post.event.as_ref().map(|e| e.name.clone()),
                summary: status.summary.clone(),
                earliest: status
                    .earliest
                    .map(|d| d.format("%b %-d").to_string())
                    .unwrap_or_default(),
                ready: status.ready,
            };
            ideas.push((status, row));
            continue;
        };
        let row = PostRow {
            href,
            name: post.name.clone(),
            series: series_label(post),
            post_date: date_long(post_date),
            health: health(post, today),
        };
        if post_date < today {
            past.push(row);
        } else {
            upcoming.push(row);
        }
    }
    // Most recent first for history.
    past.reverse();
    // Ideas that are ready to go first, then the ones that could post soonest.
    ideas.sort_by(|(a, ra), (b, rb)| {
        b.ready
            .cmp(&a.ready)
            .then(a.earliest.is_none().cmp(&b.earliest.is_none()))
            .then(a.earliest.cmp(&b.earliest))
            .then(ra.name.cmp(&rb.name))
    });
    let ideas = ideas.into_iter().map(|(_, row)| row).collect();
    render(PostsPage {
        upcoming,
        past,
        ideas,
    })
}

// ---------------------------------------------------------------------------
// /posts/{id}
// ---------------------------------------------------------------------------

struct LookRow {
    pet: String,
    items: String,
}

/// One row of a prep timeline table (post pages and series pages).
pub(super) struct TaskRow {
    pub(super) when: String,
    pub(super) title: String,
    pub(super) status: String,
    pub(super) class: &'static str,
}

/// The rows of a timeline table. Once the timeline is over (for a series,
/// after its *last* post) every task shows as done.
pub(super) fn task_rows(timeline: &Timeline, today: NaiveDate) -> Vec<TaskRow> {
    let finished = timeline.post_date < today;
    timeline
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
        .collect()
}

/// A link to the event a post is for.
struct EventLink {
    name: String,
    href: String,
}

/// One line of an idea's prep list.
struct PrepRow {
    title: String,
    days: i64,
}

/// What an idea's page shows instead of a timeline.
struct IdeaView {
    summary: String,
    ready: bool,
    /// "Tue Nov 3, 2026", or empty if it can't be planned.
    earliest: String,
    prep: Vec<PrepRow>,
    schedule_action: String,
    /// The schedule form's starting value: the earliest possible date.
    suggested_date: String,
}

/// "Part of the Earth Day series (3 posts)" on a post's page.
struct SeriesLink {
    title: String,
    href: String,
    posts: usize,
}

#[derive(Template)]
#[template(path = "post.html")]
struct PostPage {
    name: String,
    series: Option<SeriesLink>,
    /// "+ Another post from this shoot".
    another_action: String,
    /// The post has its own title (otherwise `name` is the event's name).
    has_title: bool,
    event: Option<EventLink>,
    edit_href: String,
    delete_action: String,
    /// `None` for an idea.
    post_date: Option<String>,
    shoot_date: Option<String>,
    idea: Option<IdeaView>,
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

    let tasks = match &post.timeline {
        Ok(timeline) => task_rows(timeline, today),
        Err(_) => Vec::new(),
    };

    let photos = PhotoSection::load(
        &state,
        PhotoOwner::PostPlan(post.plan.id),
        &format!("the {} post", post.name),
    )
    .await?;

    let idea = post.plan.is_idea().then(|| {
        let status = idea_status(post, today);
        let prep = match &post.readiness {
            Some(Ok(readiness)) => readiness
                .prep
                .iter()
                .map(|p| PrepRow {
                    title: p.title.clone(),
                    days: p.days,
                })
                .collect(),
            _ => Vec::new(),
        };
        IdeaView {
            summary: status.summary,
            ready: status.ready,
            earliest: status.earliest.map(date_long).unwrap_or_default(),
            prep,
            schedule_action: format!("/posts/{id}/schedule"),
            suggested_date: status
                .earliest
                .map(|d| d.format("%Y-%m-%d").to_string())
                .unwrap_or_default(),
        }
    });

    // In a series, the backdrop is the series' shared one.
    let backdrop = match &post.series {
        Some(info) => info.series.backdrop.clone(),
        None => post.plan.backdrop.clone(),
    };

    render(PostPage {
        name: post.name.clone(),
        series: post.series.as_ref().map(|info| SeriesLink {
            title: info.series.title.clone(),
            href: format!("/series/{}", info.series.id.0),
            posts: info.posts,
        }),
        another_action: format!("/posts/{id}/series"),
        has_title: !post.plan.title.trim().is_empty(),
        event: post.event.as_ref().map(|e| EventLink {
            name: e.name.clone(),
            href: format!("/events/{}", e.id.0),
        }),
        edit_href: format!("/posts/{id}/edit"),
        delete_action: format!("/posts/{id}/delete"),
        post_date: post.plan.post_date.map(date_long),
        shoot_date: post.timeline.as_ref().ok().map(|t| date_long(t.shoot_date)),
        idea,
        health: health(post, today),
        looks: look_rows(post, &accessories),
        backdrop,
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
    title: String,
    event_choices: Vec<Choice>,
    series_choices: Vec<Choice>,
    /// A series is chosen, so its backdrop is used rather than this post's.
    in_series: bool,
    /// "No date yet" is chosen: save it as an idea.
    is_idea: bool,
    post_date: String,
    pets: Vec<PetLooks>,
    backdrop: String,
    notes: String,
    /// Suggestions for the chosen event, shown above the checkboxes.
    ideas: Option<Ideas>,
    errors: Vec<String>,
}

/// Everything the form offers to choose from, loaded once per request.
struct FormOptions {
    events: Vec<Event>,
    accessories: Vec<Accessory>,
    fabrics: Vec<Fabric>,
    series: Vec<Series>,
    today: NaiveDate,
}

impl FormOptions {
    async fn load(state: &AppState) -> Result<Self, AppError> {
        Ok(Self {
            events: state.db.list_events().await?,
            accessories: state.db.list_accessories().await?,
            fabrics: state.db.list_fabrics().await?,
            series: state.db.list_series().await?,
            today: state.today(),
        })
    }

    fn event(&self, id: Option<EventId>) -> Option<&Event> {
        id.and_then(|id| self.events.iter().find(|e| e.id == id))
    }
}

/// How a brand-new post form starts.
#[derive(Default)]
struct NewPost<'a> {
    /// From an event's "Plan a post" button.
    event: Option<&'a Event>,
    /// From "+ Another post from this shoot" or a series page.
    series: Option<&'a Series>,
    /// From the "+ New idea" button.
    as_idea: bool,
}

/// Events a post can be for: none, then upcoming events (soonest first), then
/// past ones (most recent first). Past events are offered too, because an
/// idea can come from an outing that already happened.
fn event_choices(events: &[Event], today: NaiveDate, selected: Option<EventId>) -> Vec<Choice> {
    let mut list = vec![Choice::new("", "— no event —", selected.is_none())];
    let choice = |e: &Event, prefix: &str| {
        let label = format!("{prefix}{} · {}", e.start.format("%a %b %-d, %Y"), e.name);
        Choice::new(e.id.0, label, Some(e.id) == selected)
    };
    list.extend(
        events
            .iter()
            .filter(|e| e.end_date() >= today)
            .map(|e| choice(e, "")),
    );
    list.extend(
        events
            .iter()
            .rev()
            .filter(|e| e.end_date() < today)
            .map(|e| choice(e, "Past: ")),
    );
    list
}

fn series_choices(series: &[Series], selected: Option<SeriesId>) -> Vec<Choice> {
    let mut list = vec![Choice::new("", "— not in a series —", selected.is_none())];
    for s in series {
        list.push(Choice::new(s.id.0, s.title.clone(), Some(s.id) == selected));
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
    /// The form for `editing`, or for a new post started as `start` says.
    fn new(editing: Option<&PostPlan>, start: NewPost, options: &FormOptions) -> Self {
        let (event_id, series_id, post_date, is_idea) = match editing {
            Some(p) => (p.event_id, p.series_id, p.post_date, p.is_idea()),
            // A new post defaults to the day of its event.
            None => (
                start.event.map(|e| e.id),
                start.series.map(|s| s.id),
                start.event.filter(|_| !start.as_idea).map(|e| e.start),
                start.as_idea,
            ),
        };
        Self {
            heading: match (editing, &start) {
                (Some(p), _) if p.is_idea() => "Edit post idea".into(),
                (Some(_), _) => "Edit post".into(),
                (
                    None,
                    NewPost {
                        series: Some(s), ..
                    },
                ) => {
                    format!("Another post for the {} series", s.title)
                }
                (None, NewPost { event: Some(e), .. }) => format!("Plan a post for {}", e.name),
                (None, NewPost { as_idea: true, .. }) => "New post idea".into(),
                (None, _) => "Plan a post".into(),
            },
            action: editing.map_or("/posts".into(), |p| format!("/posts/{}", p.id.0)),
            cancel_href: editing.map_or("/posts".into(), |p| format!("/posts/{}", p.id.0)),
            title: editing.map(|p| p.title.clone()).unwrap_or_default(),
            event_choices: event_choices(&options.events, options.today, event_id),
            series_choices: series_choices(&options.series, series_id),
            in_series: series_id.is_some(),
            is_idea,
            post_date: post_date
                .map(|d| d.format("%Y-%m-%d").to_string())
                .unwrap_or_default(),
            pets: pet_looks(&options.accessories, editing.map_or(&[], |p| &p.looks)),
            backdrop: editing.map(|p| p.backdrop.clone()).unwrap_or_default(),
            notes: editing.map(|p| p.notes.clone()).unwrap_or_default(),
            ideas: None,
            errors: Vec::new(),
        }
    }

    fn refill(mut self, form: &FormData, errors: Vec<String>) -> Self {
        let select = |list: &mut Vec<Choice>, value: String| {
            for choice in list.iter_mut() {
                choice.selected = choice.value == value;
            }
        };
        select(&mut self.event_choices, form.text("event"));
        select(&mut self.series_choices, form.text("series"));
        for pet in &mut self.pets {
            let ticked: Vec<&str> = form.all(pet.field).collect();
            for choice in &mut pet.choices {
                choice.selected = ticked.contains(&choice.value.as_str());
            }
        }
        self.title = form.text("title");
        self.in_series = !form.text("series").is_empty();
        self.is_idea = form.text("when") == "idea";
        self.post_date = form.text("post_date");
        self.backdrop = form.text("backdrop");
        self.notes = form.text("notes");
        self.errors = errors;
        self
    }

    /// Add the ideas box for `event` (see `form_ideas`).
    fn with_ideas(
        mut self,
        state: &AppState,
        event: Option<&Event>,
        post_date: Option<NaiveDate>,
        options: &FormOptions,
    ) -> Self {
        self.ideas = form_ideas(state, event, post_date, options);
        self
    }
}

fn read_form(form: &FormData, options: &FormOptions) -> Result<NewPostPlan, Vec<String>> {
    let mut errors = Vec::new();

    let title = form.text("title");
    // The event is optional, but if one is sent it has to exist.
    let event = match form.optional_id("event") {
        Ok(None) => None,
        Ok(Some(id)) => {
            let found = options.events.iter().find(|e| e.id.0 == id);
            if found.is_none() {
                errors.push("That event doesn't exist any more.".to_string());
            }
            found
        }
        Err(_) => {
            errors.push("That event isn't a valid choice.".to_string());
            None
        }
    };
    if title.is_empty() && event.is_none() {
        errors.push("Give the post a title, or choose an event.".to_string());
    }

    // The series is optional too.
    let series_id = match form.optional_id("series") {
        Ok(None) => None,
        Ok(Some(id)) => match options.series.iter().find(|s| s.id.0 == id) {
            Some(s) => Some(s.id),
            None => {
                errors.push("That series doesn't exist any more.".to_string());
                None
            }
        },
        Err(_) => {
            errors.push("That series isn't a valid choice.".to_string());
            None
        }
    };

    // An idea has no date. Otherwise an empty date means the event's day.
    let post_date = if form.text("when") == "idea" {
        None
    } else {
        match form.optional_date("post_date") {
            Ok(Some(date)) => Some(date),
            Ok(None) => match event {
                Some(e) => Some(e.start),
                None => {
                    errors.push(
                        "Choose a post date, or save it as an idea with no date yet.".to_string(),
                    );
                    None
                }
            },
            Err(NotADate) => {
                errors.push("The post date isn't a real date.".to_string());
                None
            }
        }
    };

    let mut looks = Vec::new();
    for (pet, field, label) in PETS {
        for value in form.all(field) {
            match value
                .parse::<i64>()
                .ok()
                .filter(|id| options.accessories.iter().any(|a| a.id.0 == *id))
            {
                Some(id) => looks.push(Look {
                    pet,
                    accessory_id: AccessoryId(id),
                }),
                None => errors.push(format!("One of {label}'s items isn't in the inventory.")),
            }
        }
    }

    if !errors.is_empty() {
        return Err(errors);
    }
    Ok(NewPostPlan {
        title,
        event_id: event.map(|e| e.id),
        post_date,
        series_id,
        looks,
        backdrop: form.text("backdrop"),
        notes: form.text("notes"),
    })
}

/// Ideas for the event the form is showing, if one is chosen. Without
/// JavaScript the box can't follow the dropdown live; it reflects the event
/// the page was opened (or last submitted) with.
fn form_ideas(
    state: &AppState,
    event: Option<&Event>,
    post_date: Option<NaiveDate>,
    options: &FormOptions,
) -> Option<Ideas> {
    event.map(|e| {
        ideas::for_event(
            e,
            post_date.unwrap_or(e.start),
            options.today,
            &options.accessories,
            &options.fabrics,
            &state.lead_times,
        )
    })
}

/// The event and date a submitted form asked for, even if the form had
/// mistakes, so the ideas box can still be shown.
fn submitted_event<'a>(
    form: &FormData,
    options: &'a FormOptions,
) -> (Option<&'a Event>, Option<NaiveDate>) {
    let event = options.event(form.optional_id("event").ok().flatten().map(EventId));
    (event, form.optional_date("post_date").ok().flatten())
}

fn form_response(status: StatusCode, page: PostFormPage) -> Result<Response, AppError> {
    Ok((status, render(page)?).into_response())
}

/// `/posts/new`, optionally `?event=12` from an event's "Plan a post" button,
/// `?idea=1` from the "+ New idea" button, or `?series=3` from "+ Another post
/// from this shoot" (it starts with the series' event, and as an idea if the
/// series has no dates yet).
pub async fn new_form(
    State(state): State<AppState>,
    Query(query): Query<HashMap<String, String>>,
) -> Result<Response, AppError> {
    let options = FormOptions::load(&state).await?;
    let id_from = |key: &str| query.get(key).and_then(|v| v.parse::<i64>().ok());
    let series = id_from("series").and_then(|id| options.series.iter().find(|s| s.id.0 == id));

    let mut start = NewPost {
        event: options.event(id_from("event").map(EventId)),
        series,
        as_idea: query.contains_key("idea"),
    };
    if let Some(series) = series {
        let plans = state.db.list_post_plans().await?;
        let members: Vec<&PostPlan> = plans
            .iter()
            .filter(|p| p.series_id == Some(series.id))
            .collect();
        if start.event.is_none() {
            start.event = options.event(members.first().and_then(|p| p.event_id));
        }
        start.as_idea |= !members.is_empty() && members.iter().all(|p| p.is_idea());
    }
    let event = start.event;
    let page = PostFormPage::new(None, start, &options).with_ideas(&state, event, None, &options);
    form_response(StatusCode::OK, page)
}

pub async fn create(
    State(state): State<AppState>,
    Form(pairs): Form<Vec<(String, String)>>,
) -> Result<Response, AppError> {
    let form = FormData::new(pairs);
    let options = FormOptions::load(&state).await?;
    match read_form(&form, &options) {
        Ok(new) => {
            let id = state.db.create_post_plan(&new).await?;
            // Straight to the new post's timeline: that's the payoff.
            Ok(Redirect::to(&format!("/posts/{}", id.0)).into_response())
        }
        Err(errors) => {
            let (event, date) = submitted_event(&form, &options);
            let page = PostFormPage::new(None, NewPost::default(), &options)
                .refill(&form, errors)
                .with_ideas(&state, event, date, &options);
            form_response(StatusCode::UNPROCESSABLE_ENTITY, page)
        }
    }
}

pub async fn edit_form(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Response, AppError> {
    let plan = state.db.get_post_plan(PostPlanId(id)).await?;
    let options = FormOptions::load(&state).await?;
    let event = options.event(plan.event_id);
    let page = PostFormPage::new(Some(&plan), NewPost::default(), &options).with_ideas(
        &state,
        event,
        plan.post_date,
        &options,
    );
    form_response(StatusCode::OK, page)
}

pub async fn update(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Form(pairs): Form<Vec<(String, String)>>,
) -> Result<Response, AppError> {
    let form = FormData::new(pairs);
    let plan = state.db.get_post_plan(PostPlanId(id)).await?;
    let options = FormOptions::load(&state).await?;
    match read_form(&form, &options) {
        Ok(new) => {
            state.db.update_post_plan(plan.id, &new).await?;
            Ok(Redirect::to(&format!("/posts/{id}")).into_response())
        }
        Err(errors) => {
            let (event, date) = submitted_event(&form, &options);
            let page = PostFormPage::new(Some(&plan), NewPost::default(), &options)
                .refill(&form, errors)
                .with_ideas(&state, event, date, &options);
            form_response(StatusCode::UNPROCESSABLE_ENTITY, page)
        }
    }
}

/// "+ Another post from this shoot": put the post in a series (making one,
/// named after the post, if it isn't in one yet), then open the form for the
/// next post in it.
pub async fn another_from_shoot(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Redirect, AppError> {
    let plan = state.db.get_post_plan(PostPlanId(id)).await?;
    let series = match plan.series_id {
        Some(series) => series,
        None => {
            let event = match plan.event_id {
                Some(id) => Some(state.db.get_event(id).await?),
                None => None,
            };
            state
                .db
                .series_from_post(plan.id, &plan.name(event.as_ref()))
                .await?
        }
    };
    Ok(Redirect::to(&format!("/posts/new?series={}", series.0)))
}

/// "Schedule it" on an idea's page: give it a date, and it becomes a normal
/// post with a timeline. (Editing the post can also take a date away again.)
pub async fn schedule(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Form(pairs): Form<Vec<(String, String)>>,
) -> Result<Response, AppError> {
    let form = FormData::new(pairs);
    let plan = state.db.get_post_plan(PostPlanId(id)).await?;
    // The date input is required in the page; only a hand-made request
    // could send something else.
    let date = form
        .optional_date("post_date")
        .ok()
        .flatten()
        .ok_or(AppError::BadRequest)?;
    let scheduled = NewPostPlan {
        title: plan.title,
        event_id: plan.event_id,
        post_date: Some(date),
        series_id: plan.series_id,
        looks: plan.looks,
        backdrop: plan.backdrop,
        notes: plan.notes,
    };
    state.db.update_post_plan(plan.id, &scheduled).await?;
    Ok(Redirect::to(&format!("/posts/{id}")).into_response())
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
