//! The calendar page: a month grid with holidays and prep tasks, and a box
//! above it saying what needs doing today.

use askama::Template;
use axum::extract::{Path, State};
use axum::response::Html;
use chrono::Datelike;

use super::month::{self, Agenda, MonthView, PlannedPost};
use super::{AppError, AppState, load_posts, render};
use crate::planner::PlanError;

#[derive(Template)]
#[template(path = "calendar.html")]
struct CalendarPage {
    view: MonthView,
    agenda: Agenda,
    /// Posts that couldn't be planned, e.g. "Halloween: accessory #4 is missing".
    problems: Vec<String>,
}

/// `/`: the current month.
pub async fn this_month(State(state): State<AppState>) -> Result<Html<String>, AppError> {
    let today = state.today();
    render_month(&state, today.year(), today.month()).await
}

/// `/calendar/{year}/{month}`
pub async fn month(
    State(state): State<AppState>,
    Path((year, month)): Path<(i32, u32)>,
) -> Result<Html<String>, AppError> {
    render_month(&state, year, month).await
}

async fn render_month(state: &AppState, year: i32, month: u32) -> Result<Html<String>, AppError> {
    let (grid_start, grid_end) = month::grid_range(year, month).ok_or(AppError::NotFound)?;
    let today = state.today();

    let events = state.db.list_events_between(grid_start, grid_end).await?;
    let (posts, _accessories) = load_posts(state).await?;

    let mut planned = Vec::new();
    let mut problems = Vec::new();
    for post in &posts {
        match &post.timeline {
            Ok(timeline) => planned.push(PlannedPost {
                id: post.plan.id,
                post_name: &post.name,
                timeline,
            }),
            // Ideas have no dates, so there's nothing to put on a calendar.
            Err(PlanError::NotScheduled) => {}
            Err(e) => problems.push(format!("{}: {e}", post.name)),
        }
    }

    let view =
        month::month_view(year, month, today, &events, &planned).ok_or(AppError::NotFound)?;
    let agenda = month::agenda(today, &planned);
    render(CalendarPage {
        view,
        agenda,
        problems,
    })
}
