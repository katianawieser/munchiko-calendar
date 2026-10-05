//! The web app (Milestone 4): an Axum server that renders HTML pages.
//!
//! Design notes:
//! - Pages are rendered on the server with Askama templates (`templates/`).
//!   Askama compiles each template into Rust, so a typo in a template is a
//!   build error, not a broken page.
//! - Handlers stay thin: load data from the [`Repository`], run the planner,
//!   and hand the template ready-made strings. Layout logic that deserves
//!   tests (the month grid) lives in [`month`], which has no web code.
//! - The stylesheets are compiled into the binary with `include_str!`, so the
//!   app is a single file that needs no internet connection.
//! - No login: the server only listens on 127.0.0.1, so it's reachable from
//!   this computer alone.

mod calendar;
pub mod month;
mod posts;

use std::collections::HashMap;

use askama::Template;
use axum::Router;
use axum::http::{StatusCode, header};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use chrono::{Local, NaiveDate};

use crate::db::{DbError, Repository};
use crate::models::{Accessory, Event, EventId, PostPlan};
use crate::planner::{Inventory, LeadTimes, PlanError, Timeline, build_timeline};

/// Everything a handler needs. Axum hands each handler a clone; the
/// repository's connection pool is shared between clones.
#[derive(Debug, Clone)]
pub struct AppState {
    pub db: Repository,
    pub lead_times: LeadTimes,
    /// Pretend today is this date. Tests use it so their output doesn't
    /// depend on when they run; the real app leaves it `None`.
    pub today_override: Option<NaiveDate>,
}

impl AppState {
    pub fn new(db: Repository) -> Self {
        Self {
            db,
            lead_times: LeadTimes::default(),
            today_override: None,
        }
    }

    fn today(&self) -> NaiveDate {
        self.today_override
            .unwrap_or_else(|| Local::now().date_naive())
    }
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(calendar::this_month))
        .route("/calendar/{year}/{month}", get(calendar::month))
        .route("/posts", get(posts::list))
        .route("/posts/{id}", get(posts::show))
        .route("/static/pico.min.css", get(pico_css))
        .route("/static/style.css", get(style_css))
        .fallback(not_found)
        .with_state(state)
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// What a handler can fail with. Turning it into a response (below) is the
/// one place that decides which status code and page the browser gets.
#[derive(Debug)]
pub enum AppError {
    NotFound,
    Internal(String),
}

impl From<DbError> for AppError {
    fn from(e: DbError) -> Self {
        match e {
            DbError::NotFound { .. } => AppError::NotFound,
            other => AppError::Internal(other.to_string()),
        }
    }
}

impl From<askama::Error> for AppError {
    fn from(e: askama::Error) -> Self {
        AppError::Internal(format!("template error: {e}"))
    }
}

#[derive(Template)]
#[template(path = "error.html")]
struct ErrorPage {
    heading: &'static str,
    message: &'static str,
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, page) = match self {
            AppError::NotFound => (
                StatusCode::NOT_FOUND,
                ErrorPage {
                    heading: "Not found",
                    message: "There's nothing at this address.",
                },
            ),
            AppError::Internal(detail) => {
                // The details go to the server log, not to the browser.
                eprintln!("internal error: {detail}");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    ErrorPage {
                        heading: "Something went wrong",
                        message: "The error has been logged in the terminal running the app.",
                    },
                )
            }
        };
        match page.render() {
            Ok(html) => (status, Html(html)).into_response(),
            Err(_) => (status, page.heading).into_response(),
        }
    }
}

async fn not_found() -> AppError {
    AppError::NotFound
}

/// Render a template into an HTML response.
fn render(page: impl Template) -> Result<Html<String>, AppError> {
    Ok(Html(page.render()?))
}

// ---------------------------------------------------------------------------
// Stylesheets
// ---------------------------------------------------------------------------

const PICO_CSS: &str = include_str!("../../static/pico.min.css");
const STYLE_CSS: &str = include_str!("../../static/style.css");

async fn pico_css() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
        PICO_CSS,
    )
}

async fn style_css() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
        STYLE_CSS,
    )
}

// ---------------------------------------------------------------------------
// Shared loading
// ---------------------------------------------------------------------------

/// A post plan with its event and its computed schedule.
struct LoadedPost {
    plan: PostPlan,
    event: Event,
    /// An error here means the plan refers to something that's gone missing;
    /// pages show the problem instead of failing completely.
    timeline: Result<Timeline, PlanError>,
}

/// Every post plan (soonest first), plus the accessories so pages can show
/// what each pet is wearing. Uses three queries in total, however many posts
/// there are.
async fn load_posts(state: &AppState) -> Result<(Vec<LoadedPost>, Vec<Accessory>), AppError> {
    let fabrics = state.db.list_fabrics().await?;
    let accessories = state.db.list_accessories().await?;
    let events: HashMap<EventId, Event> = state
        .db
        .list_events()
        .await?
        .into_iter()
        .map(|e| (e.id, e))
        .collect();

    let inventory = Inventory {
        accessories: &accessories,
        fabrics: &fabrics,
    };
    let mut posts = Vec::new();
    for plan in state.db.list_post_plans().await? {
        // The schema guarantees the event exists (foreign key), so a miss
        // here would be a bug rather than something the user did.
        let event = events
            .get(&plan.event_id)
            .cloned()
            .ok_or_else(|| AppError::Internal(format!("post {} has no event", plan.id.0)))?;
        let timeline = build_timeline(&plan, &inventory, &state.lead_times);
        posts.push(LoadedPost {
            plan,
            event,
            timeline,
        });
    }
    Ok((posts, accessories))
}

#[cfg(test)]
mod tests;
