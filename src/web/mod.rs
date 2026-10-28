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
//!   this computer alone. Forms are still protected from other websites (see
//!   [`reject_cross_site`]), because a web page you visit can make your own
//!   browser send requests to localhost.

mod accessories;
pub mod blockers;
mod browse;
mod calendar;
mod colors;
mod events;
mod fabrics;
pub mod forms;
pub mod ideas;
pub mod month;
mod photos;
mod posts;
mod series;

use std::collections::HashMap;

use askama::Template;
use axum::Router;
use axum::extract::{DefaultBodyLimit, Request};
use axum::http::{HeaderMap, Method, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use chrono::{Local, NaiveDate};

use crate::db::{DbError, Repository};
use crate::models::{Accessory, Event, EventId, PostPlan, Series, SeriesId};
use crate::photos::PhotoStore;
use crate::photos::heic::HeicConverter;
use crate::planner::{
    Inventory, LeadTimes, PlanError, Readiness, SeriesPost, Timeline, build_series_timeline,
    build_timeline, combined_plan, readiness,
};

/// Everything a handler needs. Axum hands each handler a clone; the
/// repository's connection pool is shared between clones.
#[derive(Debug, Clone)]
pub struct AppState {
    pub db: Repository,
    /// The folder uploaded photos are kept in.
    pub photos: PhotoStore,
    /// How HEIC uploads are converted. `new` starts with none; `main` picks
    /// the system's (so tests behave the same on every computer).
    pub heic: HeicConverter,
    pub lead_times: LeadTimes,
    /// Pretend today is this date. Tests use it so their output doesn't
    /// depend on when they run; the real app leaves it `None`.
    pub today_override: Option<NaiveDate>,
}

impl AppState {
    pub fn new(db: Repository, photos: PhotoStore) -> Self {
        Self {
            db,
            photos,
            heic: HeicConverter::Unavailable,
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
    // Uploads get a bigger size limit than the 2 MB axum allows by default;
    // every other route keeps the small limit.
    let uploads = Router::new()
        .route("/fabrics/{id}/photos", post(photos::upload_for_fabric))
        .route(
            "/accessories/{id}/photos",
            post(photos::upload_for_accessory),
        )
        .route("/posts/{id}/photos", post(photos::upload_for_post))
        .layer(DefaultBodyLimit::max(photos::MAX_UPLOAD_BYTES));

    Router::new()
        .merge(uploads)
        .route("/colors/name", get(colors::name_for_hex))
        .route("/photos/{id}", get(photos::serve))
        .route("/photos/{id}/thumb", get(photos::serve_thumbnail))
        .route("/photos/{id}/delete", post(photos::delete))
        .route("/", get(calendar::this_month))
        .route("/calendar/{year}/{month}", get(calendar::month))
        .route("/posts", get(posts::list).post(posts::create))
        .route("/posts/new", get(posts::new_form))
        .route("/posts/{id}", get(posts::show).post(posts::update))
        .route("/posts/{id}/edit", get(posts::edit_form))
        .route("/posts/{id}/schedule", post(posts::schedule))
        .route("/posts/{id}/series", post(posts::another_from_shoot))
        .route("/posts/{id}/delete", post(posts::delete))
        .route("/series", post(series::create))
        .route("/series/new", get(series::new_form))
        .route("/series/{id}", get(series::show).post(series::update))
        .route("/series/{id}/edit", get(series::edit_form))
        .route("/series/{id}/delete", post(series::delete))
        .route("/events", get(events::list).post(events::create))
        .route("/events/new", get(events::new_form))
        .route("/events/{id}", get(events::show).post(events::update))
        .route("/events/{id}/edit", get(events::edit_form))
        .route("/events/{id}/delete", post(events::delete))
        // Fabric and accessory forms can carry photos, so they get the
        // upload size limit too.
        .route(
            "/fabrics",
            get(fabrics::list)
                .post(fabrics::create)
                .layer(DefaultBodyLimit::max(photos::MAX_UPLOAD_BYTES)),
        )
        .route("/fabrics/new", get(fabrics::new_form))
        .route(
            "/fabrics/{id}",
            get(fabrics::edit_form)
                .post(fabrics::update)
                .layer(DefaultBodyLimit::max(photos::MAX_UPLOAD_BYTES)),
        )
        .route("/fabrics/{id}/stock", post(fabrics::toggle_stock))
        .route("/fabrics/{id}/delete", post(fabrics::delete))
        .route(
            "/accessories",
            get(accessories::list)
                .post(accessories::create)
                .layer(DefaultBodyLimit::max(photos::MAX_UPLOAD_BYTES)),
        )
        .route("/accessories/new", get(accessories::new_form))
        .route(
            "/accessories/{id}",
            get(accessories::edit_form)
                .post(accessories::update)
                .layer(DefaultBodyLimit::max(photos::MAX_UPLOAD_BYTES)),
        )
        .route("/accessories/{id}/status", post(accessories::set_status))
        .route("/accessories/{id}/delete", post(accessories::delete))
        .route("/static/pico.min.css", get(pico_css))
        .route("/static/style.css", get(style_css))
        .route("/static/colors.js", get(colors_js))
        .route("/static/photos.js", get(photos_js))
        .fallback(not_found)
        .layer(middleware::from_fn(reject_cross_site))
        .with_state(state)
}

/// Refuse form submissions that come from another website.
///
/// This defends against "cross-site request forgery": a page on some other
/// site could contain a hidden form that posts to http://127.0.0.1:3000 and
/// deletes your fabrics, and your browser would happily send it. Browsers
/// label every request with where it came from (`Sec-Fetch-Site`, and
/// `Origin` on form posts), so we only accept changes from our own pages.
/// Reading pages (GET) is always allowed.
async fn reject_cross_site(request: Request, next: Next) -> Response {
    if request.method() != Method::GET
        && request.method() != Method::HEAD
        && is_cross_site(request.headers())
    {
        return (
            StatusCode::FORBIDDEN,
            "Forms can only be submitted from Munchiko Calendar's own pages.",
        )
            .into_response();
    }
    next.run(request).await
}

fn is_cross_site(headers: &HeaderMap) -> bool {
    let text = |name| {
        headers
            .get(name)
            .and_then(|v: &header::HeaderValue| v.to_str().ok())
    };
    // "same-origin" is our own page; "none" means typed in or bookmarked.
    if let Some(site) = text(header::HeaderName::from_static("sec-fetch-site")) {
        return site != "same-origin" && site != "none";
    }
    // Older browsers: compare the Origin header ("http://127.0.0.1:3000")
    // with the address the request was sent to ("127.0.0.1:3000").
    match (text(header::ORIGIN), text(header::HOST)) {
        (Some(origin), Some(host)) => origin.split_once("://").map(|(_, rest)| rest) != Some(host),
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// What a handler can fail with. Turning it into a response (below) is the
/// one place that decides which status code and page the browser gets.
#[derive(Debug)]
pub enum AppError {
    NotFound,
    /// The request doesn't make sense (e.g. an unknown status value), which
    /// only happens if someone crafts it by hand.
    BadRequest,
    /// Trying to change a holiday that comes from holidays.toml.
    ReadOnly,
    /// An upload that isn't a usable photo; `back` is the page to return to.
    PhotoRejected {
        status: StatusCode,
        message: String,
        back: String,
    },
    Internal(String),
}

impl From<DbError> for AppError {
    fn from(e: DbError) -> Self {
        match e {
            DbError::NotFound { .. } => AppError::NotFound,
            DbError::ReadOnly { .. } => AppError::ReadOnly,
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
    message: String,
    back_href: String,
    back_label: &'static str,
}

impl ErrorPage {
    /// An error page whose link goes back to the calendar.
    fn new(heading: &'static str, message: &str) -> Self {
        Self {
            heading,
            message: message.to_string(),
            back_href: "/".into(),
            back_label: "Back to the calendar",
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, page) = match self {
            AppError::NotFound => (
                StatusCode::NOT_FOUND,
                ErrorPage::new("Not found", "There's nothing at this address."),
            ),
            AppError::BadRequest => (
                StatusCode::BAD_REQUEST,
                ErrorPage::new(
                    "That didn't work",
                    "The request had a value the app doesn't understand.",
                ),
            ),
            AppError::ReadOnly => (
                StatusCode::FORBIDDEN,
                ErrorPage::new(
                    "Holidays are edited in a file",
                    "This holiday comes from data/holidays.toml. Change it there and restart the app.",
                ),
            ),
            AppError::PhotoRejected {
                status,
                message,
                back,
            } => (
                status,
                ErrorPage {
                    heading: "That photo wasn't added",
                    message,
                    back_href: back,
                    back_label: "Go back",
                },
            ),
            AppError::Internal(detail) => {
                // The details go to the server log, not to the browser.
                eprintln!("internal error: {detail}");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    ErrorPage::new(
                        "Something went wrong",
                        "The error has been logged in the terminal running the app.",
                    ),
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
// Stylesheets and the one script
// ---------------------------------------------------------------------------

const PICO_CSS: &str = include_str!("../../static/pico.min.css");
const STYLE_CSS: &str = include_str!("../../static/style.css");
const COLORS_JS: &str = include_str!("../../static/colors.js");
const PHOTOS_JS: &str = include_str!("../../static/photos.js");

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

/// Photo previews and instant uploads; the forms work without it.
async fn photos_js() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
        PHOTOS_JS,
    )
}

/// Swatch pickers for the color rows; the forms work without it.
async fn colors_js() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
        COLORS_JS,
    )
}

// ---------------------------------------------------------------------------
// Shared loading
// ---------------------------------------------------------------------------

/// Everything that can block a delete, for the "can't delete yet" box.
async fn load_usage(state: &AppState) -> Result<blockers::Usage, AppError> {
    Ok(blockers::Usage {
        accessories: state.db.list_accessories().await?,
        plans: state.db.list_post_plans().await?,
        events: state.db.list_events().await?,
    })
}

/// The series a post belongs to, as its pages need it.
#[derive(Debug, Clone)]
struct SeriesInfo {
    series: Series,
    /// How many posts are in it.
    posts: usize,
    /// The first scheduled post's date: the shared shoot is planned for it.
    first_post: Option<NaiveDate>,
}

/// A post plan with its event and its computed schedule.
struct LoadedPost {
    plan: PostPlan,
    /// The event it's for, if any.
    event: Option<Event>,
    /// What to call it: its title, else its event's name.
    name: String,
    /// An error here means the plan refers to something that's gone missing;
    /// pages show the problem instead of failing completely. Ideas (no date)
    /// always get `Err(PlanError::NotScheduled)`.
    timeline: Result<Timeline, PlanError>,
    /// For ideas only: the prep they still need and how long it takes.
    readiness: Option<Result<Readiness, PlanError>>,
    /// In a series, the timeline above is the *series'* shared one.
    series: Option<SeriesInfo>,
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
    let lead = &state.lead_times;
    let plans = state.db.list_post_plans().await?;

    // Each post's event and name.
    let mut named = Vec::new();
    for plan in plans {
        // The schema guarantees a linked event exists (foreign key), so a
        // miss here would be a bug rather than something the user did.
        let event = match plan.event_id {
            Some(id) => Some(events.get(&id).cloned().ok_or_else(|| {
                AppError::Internal(format!("post {} has a missing event", plan.id.0))
            })?),
            None => None,
        };
        let name = plan.name(event.as_ref());
        named.push((plan, event, name));
    }

    // Each series is planned once, for all of its posts together.
    let mut shared: HashMap<SeriesId, SharedPlan> = HashMap::new();
    for series in state.db.list_series().await? {
        let members: Vec<SeriesPost> = named
            .iter()
            .filter(|(p, _, _)| p.series_id == Some(series.id))
            .map(|(plan, _, name)| SeriesPost { plan, name })
            .collect();
        if members.is_empty() {
            continue;
        }
        let plans: Vec<&PostPlan> = members.iter().map(|m| m.plan).collect();
        let combined = combined_plan(&series, &plans);
        let timeline = build_series_timeline(&series, &members, &inventory, lead);
        let ready = readiness(&combined, &inventory, lead);
        let info = SeriesInfo {
            posts: members.len(),
            first_post: combined.post_date,
            series: series.clone(),
        };
        shared.insert(
            series.id,
            SharedPlan {
                info,
                timeline,
                readiness: ready,
            },
        );
    }

    let mut posts = Vec::new();
    for (plan, event, name) in named {
        let (timeline, readiness, series) = match plan.series_id.and_then(|id| shared.get(&id)) {
            Some(shared) => (
                // A dated post in a series follows the shared schedule.
                match plan.post_date {
                    Some(_) => shared.timeline.clone(),
                    None => Err(PlanError::NotScheduled),
                },
                plan.is_idea().then(|| shared.readiness.clone()),
                Some(shared.info.clone()),
            ),
            None => (
                // Ideas get `Err(NotScheduled)`: they have no dates to plan yet.
                build_timeline(&plan, &inventory, lead),
                plan.is_idea().then(|| readiness(&plan, &inventory, lead)),
                None,
            ),
        };
        posts.push(LoadedPost {
            plan,
            event,
            name,
            timeline,
            readiness,
            series,
        });
    }
    Ok((posts, accessories))
}

/// A series, planned once for all of its posts.
struct SharedPlan {
    info: SeriesInfo,
    timeline: Result<Timeline, PlanError>,
    /// For posts in it that have no date yet.
    readiness: Result<Readiness, PlanError>,
}

#[cfg(test)]
mod tests;
