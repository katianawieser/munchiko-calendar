//! Post pages: the list of planned posts, and one post's prep timeline.

use askama::Template;
use axum::extract::{Path, State};
use axum::response::Html;
use chrono::NaiveDate;

use super::{AppError, AppState, LoadedPost, load_posts, render};
use crate::models::{Accessory, Pet};
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
    post_date: String,
    shoot_date: Option<String>,
    health: Health,
    looks: Vec<LookRow>,
    backdrop: String,
    notes: String,
    tasks: Vec<TaskRow>,
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

    render(PostPage {
        event_name: post.event.name.clone(),
        post_date: date_long(post.plan.post_date),
        shoot_date: post.timeline.as_ref().ok().map(|t| date_long(t.shoot_date)),
        health: health(post, today),
        looks: look_rows(post, &accessories),
        backdrop: post.plan.backdrop.clone(),
        notes: post.plan.notes.clone(),
        tasks,
    })
}
