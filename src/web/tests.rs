//! Web tests: build the whole app on an in-memory database and send it
//! requests directly (no real server or network), then check the HTML.
//! "Today" is pinned to Oct 5, 2026 so the output never depends on the date.

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use chrono::NaiveDate;
use tower::ServiceExt;

use super::{AppState, router};
use crate::db::{NewEvent, Repository};
use crate::models::{EventKind, PostPlanId};
use crate::{demo, holidays};

fn d(y: i32, m: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, day).unwrap()
}

/// The app with the built-in holidays and the Halloween example, as it looks
/// on `today`.
async fn setup(today: NaiveDate) -> (Router, Repository, PostPlanId) {
    let db = Repository::in_memory().await.unwrap();
    let rules = holidays::parse(holidays::BUILT_IN).unwrap();
    db.sync_holidays(&rules, 2026..=2027).await.unwrap();
    let post = demo::seed_halloween(&db, 2026).await.unwrap();

    let mut state = AppState::new(db.clone());
    state.today_override = Some(today);
    (router(state), db, post)
}

async fn get(app: &Router, uri: &str) -> (StatusCode, String) {
    let request = Request::get(uri).body(Body::empty()).unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (status, String::from_utf8(bytes.to_vec()).unwrap())
}

#[tokio::test]
async fn home_page_shows_this_month_with_holidays_and_tasks() {
    let (app, _, post) = setup(d(2026, 10, 5)).await;
    let (status, html) = get(&app, "/").await;

    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("October 2026"));
    assert!(html.contains("Mochi"), "Mochi's birthday is on Oct 15");
    assert!(html.contains("🎉 Halloween"));
    assert!(html.contains("Make Pumpkin bow"));
    assert!(html.contains(&format!("href=\"/posts/{}\"", post.0)));
    // Nothing is due on Oct 5; the first task (ordering the scarf) is Oct 20.
    assert!(html.contains("Nothing due today"));
    assert!(html.contains("starts in 15d (Oct 20)"));
}

#[tokio::test]
async fn calendar_pages_move_between_months() {
    let (app, _, _) = setup(d(2026, 10, 5)).await;
    let (status, html) = get(&app, "/calendar/2026/11").await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("November 2026"));
    assert!(html.contains("Thanksgiving"));
    assert!(html.contains("href=\"/calendar/2026/10\""));
    assert!(html.contains("href=\"/calendar/2026/12\""));
}

#[tokio::test]
async fn overdue_work_is_flagged_on_the_calendar() {
    // Oct 29: the bows were due Oct 27, so they're 2 days late.
    let (app, _, _) = setup(d(2026, 10, 29)).await;
    let (_, html) = get(&app, "/").await;
    assert!(html.contains("To do today"));
    assert!(html.contains("2d late"));
    assert!(html.contains("entry task-make overdue"));
}

#[tokio::test]
async fn post_page_shows_the_timeline() {
    let (app, _, post) = setup(d(2026, 10, 5)).await;
    let (status, html) = get(&app, &format!("/posts/{}", post.0)).await;

    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("Halloween post"));
    assert!(html.contains("On track: 15 days before prep starts"));
    assert!(html.contains("Pumpkin bow, Ghost scarf"), "Mochi's look");
    assert!(html.contains("Buy fabric: Orange velvet"));
    assert!(html.contains("hay bales and mini pumpkins"));
}

#[tokio::test]
async fn posts_list_moves_finished_posts_to_the_past() {
    let (app, _, _) = setup(d(2026, 10, 5)).await;
    let (_, html) = get(&app, "/posts").await;
    assert!(html.contains("On track"));
    assert!(!html.contains("Past posts"));

    let (app, _, _) = setup(d(2026, 11, 5)).await;
    let (_, html) = get(&app, "/posts").await;
    assert!(html.contains("Past posts"));
    assert!(html.contains("Published"));
}

#[tokio::test]
async fn unknown_things_are_404s() {
    let (app, _, _) = setup(d(2026, 10, 5)).await;
    for uri in ["/posts/999", "/calendar/2026/13", "/no/such/page"] {
        let (status, html) = get(&app, uri).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{uri}");
        assert!(
            html.contains("Not found"),
            "{uri} should show the error page"
        );
    }
    // Not a number at all: Axum rejects it before our code runs.
    let (status, _) = get(&app, "/posts/abc").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn names_are_escaped_not_run_as_html() {
    let (app, db, _) = setup(d(2026, 10, 5)).await;
    db.create_event(&NewEvent {
        name: "<script>alert('hi')</script>".into(),
        description: String::new(),
        kind: EventKind::Outing,
        start: d(2026, 10, 10),
        end: None,
        keywords: vec![],
    })
    .await
    .unwrap();

    let (_, html) = get(&app, "/").await;
    assert!(!html.contains("<script>alert"));
    // Askama writes < and > as the numeric codes &#60; and &#62;.
    assert!(html.contains("&#60;script&#62;"));
}

#[tokio::test]
async fn stylesheets_are_served_as_css() {
    let (app, _, _) = setup(d(2026, 10, 5)).await;
    for uri in ["/static/pico.min.css", "/static/style.css"] {
        let response = app
            .clone()
            .oneshot(Request::get(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "text/css; charset=utf-8"
        );
    }
}
