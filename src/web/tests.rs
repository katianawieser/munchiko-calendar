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

// ---------------------------------------------------------------------------
// Forms (M4b). The Halloween example has fabrics 1 = Orange velvet and
// 2 = Black satin, and accessories 1 = Pumpkin bow, 2 = Bat bow (a matching
// set) and 3 = Ghost scarf, all used by the Halloween post.
// ---------------------------------------------------------------------------

/// What came back from a form submission.
struct Reply {
    status: StatusCode,
    /// Where a redirect points, if it was one.
    location: Option<String>,
    html: String,
}

async fn post_with(app: &Router, uri: &str, body: &str, headers: &[(&str, &str)]) -> Reply {
    let mut request =
        Request::post(uri).header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let response = app
        .clone()
        .oneshot(request.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let location = response
        .headers()
        .get(header::LOCATION)
        .map(|v| v.to_str().unwrap().to_string());
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    Reply {
        status,
        location,
        html: String::from_utf8(bytes.to_vec()).unwrap(),
    }
}

/// A form posted from one of our own pages.
async fn post(app: &Router, uri: &str, body: &str) -> Reply {
    post_with(app, uri, body, &[("sec-fetch-site", "same-origin")]).await
}

use crate::models::{AccessoryId, AccessoryStatus, FabricId, FabricRole, Source};

#[tokio::test]
async fn adding_a_fabric_redirects_to_the_list() {
    let (app, db, _) = setup(d(2026, 10, 5)).await;
    let reply = post(
        &app,
        "/fabrics",
        "name=Gold+lame&description=&colors=Gold%2C+Yellow&keywords=New+Year&in_stock=on&pairs=2",
    )
    .await;
    assert_eq!(reply.status, StatusCode::SEE_OTHER);
    assert_eq!(reply.location.as_deref(), Some("/fabrics"));

    let gold = db.get_fabric(FabricId(3)).await.unwrap();
    assert_eq!(gold.colors, vec!["gold", "yellow"]);
    assert_eq!(gold.pairings, vec![FabricId(2)]);
    assert!(
        db.get_fabric(FabricId(2))
            .await
            .unwrap()
            .pairings
            .contains(&gold.id),
        "two-way"
    );

    let (_, html) = get(&app, "/fabrics").await;
    assert!(html.contains("Gold lame"));
    assert!(html.contains("Shopping list"));
}

#[tokio::test]
async fn a_fabric_form_mistake_keeps_what_was_typed() {
    let (app, db, _) = setup(d(2026, 10, 5)).await;
    let reply = post(&app, "/fabrics", "name=+&colors=teal&pairs=1").await;

    assert_eq!(reply.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(reply.html.contains("Give the fabric a name."));
    assert!(
        reply.html.contains("value=\"teal\""),
        "typed colors are kept"
    );
    assert!(
        reply.html.contains("value=\"1\" checked"),
        "ticked pairing is kept"
    );
    assert_eq!(db.list_fabrics().await.unwrap().len(), 2, "nothing saved");
}

#[tokio::test]
async fn editing_a_fabric_saves_details_and_pairings() {
    let (app, db, _) = setup(d(2026, 10, 5)).await;
    let (status, html) = get(&app, "/fabrics/1").await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("Edit Orange velvet"));
    assert!(html.contains("Used by: Pumpkin bow"));

    // Untick the pairing with black satin and rename.
    let reply = post(
        &app,
        "/fabrics/1",
        "name=Pumpkin+velvet&colors=orange&keywords=",
    )
    .await;
    assert_eq!(reply.status, StatusCode::SEE_OTHER);
    let velvet = db.get_fabric(FabricId(1)).await.unwrap();
    assert_eq!(velvet.name, "Pumpkin velvet");
    assert!(velvet.pairings.is_empty());
    assert!(!velvet.in_stock, "unticked checkbox means not in stock");
    assert!(
        db.get_fabric(FabricId(2))
            .await
            .unwrap()
            .pairings
            .is_empty()
    );
}

#[tokio::test]
async fn stock_button_moves_fabric_between_lists() {
    let (app, db, _) = setup(d(2026, 10, 5)).await;
    assert!(!db.get_fabric(FabricId(1)).await.unwrap().in_stock);
    let reply = post(&app, "/fabrics/1/stock", "").await;
    assert_eq!(reply.location.as_deref(), Some("/fabrics"));
    assert!(db.get_fabric(FabricId(1)).await.unwrap().in_stock);
}

#[tokio::test]
async fn deleting_a_fabric_in_use_explains_why_not() {
    let (app, db, _) = setup(d(2026, 10, 5)).await;
    let reply = post(&app, "/fabrics/2/delete", "").await;
    assert_eq!(reply.status, StatusCode::CONFLICT);
    assert!(
        reply
            .html
            .contains("Can&#39;t delete Black satin: it&#39;s used by Bat bow, Pumpkin bow")
    );

    let spare = db
        .create_fabric(&crate::db::NewFabric {
            name: "Spare tulle".into(),
            description: String::new(),
            colors: vec![],
            keywords: vec![],
            in_stock: true,
        })
        .await
        .unwrap();
    let reply = post(&app, &format!("/fabrics/{}/delete", spare.0), "").await;
    assert_eq!(reply.status, StatusCode::SEE_OTHER);
    assert!(db.get_fabric(spare).await.is_err());
}

#[tokio::test]
async fn adding_an_accessory_with_fabrics_and_a_partner() {
    let (app, db, _) = setup(d(2026, 10, 5)).await;
    let reply = post(
        &app,
        "/accessories",
        "kind=bow&status=idea&name=Candy+corn+bow&description=&colors=orange%2C+white\
         &main_fabric=1&middle_fabric=&accent_fabric=2&source=bought&brand=Etsy\
         &make_days=&partner=3",
    )
    .await;
    assert_eq!(reply.status, StatusCode::SEE_OTHER);

    let candy = db.get_accessory(AccessoryId(4)).await.unwrap();
    let roles: Vec<(i64, FabricRole)> = candy
        .fabrics
        .iter()
        .map(|u| (u.fabric_id.0, u.role))
        .collect();
    assert_eq!(roles, vec![(1, FabricRole::Main), (2, FabricRole::Accent)]);
    assert_eq!(
        candy.source,
        Source::Bought {
            brand: Some("Etsy".into())
        }
    );
    assert_eq!(candy.pair_id, Some(AccessoryId(3)));
    assert_eq!(
        db.get_accessory(AccessoryId(3)).await.unwrap().pair_id,
        Some(candy.id)
    );
}

#[tokio::test]
async fn editing_an_accessory_can_unpair_it() {
    let (app, db, _) = setup(d(2026, 10, 5)).await;
    let (_, html) = get(&app, "/accessories/1").await;
    assert!(html.contains("Edit Pumpkin bow"));
    assert!(
        html.contains("<option value=\"2\" selected>Bat bow (bow)</option>"),
        "current partner"
    );

    let reply = post(
        &app,
        "/accessories/1",
        "kind=bow&status=in_progress&name=Pumpkin+bow&colors=orange&main_fabric=1\
         &middle_fabric=2&accent_fabric=&source=handmade&brand=&make_days=5&partner=",
    )
    .await;
    assert_eq!(reply.status, StatusCode::SEE_OTHER);
    let pumpkin = db.get_accessory(AccessoryId(1)).await.unwrap();
    assert_eq!(pumpkin.status, AccessoryStatus::InProgress);
    assert_eq!(pumpkin.make_days, Some(5));
    assert_eq!(pumpkin.pair_id, None);
    assert_eq!(
        db.get_accessory(AccessoryId(2)).await.unwrap().pair_id,
        None
    );
}

#[tokio::test]
async fn accessory_form_mistakes_are_explained() {
    let (app, _, _) = setup(d(2026, 10, 5)).await;
    let reply = post(
        &app,
        "/accessories/1",
        "kind=bow&status=idea&name=Pumpkin+bow&source=handmade&make_days=0&partner=1",
    )
    .await;
    assert_eq!(reply.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(
        reply
            .html
            .contains("Days to make must be a whole number of 1 or more.")
    );
    assert!(reply.html.contains("can&#39;t be its own partner"));
}

#[tokio::test]
async fn status_button_moves_along_and_returns_to_the_filtered_list() {
    let (app, db, _) = setup(d(2026, 10, 5)).await;
    let reply = post(
        &app,
        "/accessories/3/status",
        "status=in_progress&back=%2Faccessories%3Fkind%3Dscarf",
    )
    .await;
    assert_eq!(reply.location.as_deref(), Some("/accessories?kind=scarf"));
    assert_eq!(
        db.get_accessory(AccessoryId(3)).await.unwrap().status,
        AccessoryStatus::InProgress
    );

    // A "back" address pointing anywhere else is ignored.
    let reply = post(
        &app,
        "/accessories/3/status",
        "status=ready&back=https%3A%2F%2Fevil.example",
    )
    .await;
    assert_eq!(reply.location.as_deref(), Some("/accessories"));

    let reply = post(&app, "/accessories/3/status", "status=finished").await;
    assert_eq!(reply.status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn accessory_list_filters_by_kind() {
    let (app, _, _) = setup(d(2026, 10, 5)).await;
    let (_, html) = get(&app, "/accessories?kind=scarf").await;
    assert!(html.contains("Ghost scarf"));
    assert!(!html.contains("Pumpkin bow"));
    assert!(html.contains("<option value=\"scarf\" selected>"));

    let (_, html) = get(&app, "/accessories").await;
    assert!(html.contains("Orange velvet (main), Black satin (middle)"));
}

#[tokio::test]
async fn accessory_in_a_planned_post_cannot_be_deleted() {
    let (app, _, _) = setup(d(2026, 10, 5)).await;
    let reply = post(&app, "/accessories/2/delete", "").await;
    assert_eq!(reply.status, StatusCode::CONFLICT);
    assert!(reply.html.contains("a planned post uses it"));
}

#[tokio::test]
async fn forms_from_other_websites_are_refused() {
    let (app, db, _) = setup(d(2026, 10, 5)).await;

    let reply = post_with(
        &app,
        "/fabrics/1/stock",
        "",
        &[("sec-fetch-site", "cross-site")],
    )
    .await;
    assert_eq!(reply.status, StatusCode::FORBIDDEN);
    // Another port on localhost is "same-site" but still not our page.
    let reply = post_with(
        &app,
        "/fabrics/1/stock",
        "",
        &[("sec-fetch-site", "same-site")],
    )
    .await;
    assert_eq!(reply.status, StatusCode::FORBIDDEN);
    // Older browsers without Sec-Fetch-Site: Origin must match Host.
    let reply = post_with(
        &app,
        "/fabrics/1/stock",
        "",
        &[
            ("origin", "https://evil.example"),
            ("host", "127.0.0.1:3000"),
        ],
    )
    .await;
    assert_eq!(reply.status, StatusCode::FORBIDDEN);
    assert!(
        !db.get_fabric(FabricId(1)).await.unwrap().in_stock,
        "nothing changed"
    );

    let reply = post_with(
        &app,
        "/fabrics/1/stock",
        "",
        &[
            ("origin", "http://127.0.0.1:3000"),
            ("host", "127.0.0.1:3000"),
        ],
    )
    .await;
    assert_eq!(reply.status, StatusCode::SEE_OTHER);
}
