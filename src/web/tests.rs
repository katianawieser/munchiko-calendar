//! Web tests: build the whole app on an in-memory database and send it
//! requests directly (no real server or network), then check the HTML.
//! "Today" is pinned to Oct 5, 2026 so the output never depends on the date.

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use chrono::NaiveDate;
use tempfile::TempDir;
use tower::ServiceExt;

use super::{AppState, router};
use crate::db::{NewEvent, Repository};
use crate::models::{EventKind, PostPlanId};
use std::sync::LazyLock;

use image::ImageFormat;

use crate::photos::heic::HeicConverter;
use crate::photos::process::{self, tests::sample};
use crate::photos::{ImageType, PhotoStore};
use crate::{demo, holidays};

fn d(y: i32, m: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, day).unwrap()
}

/// The app with the built-in holidays and the Halloween example, as it looks
/// on `today`. Its photo folder doesn't exist; tests that upload use
/// [`setup_with_photos`].
async fn setup(today: NaiveDate) -> (Router, Repository, PostPlanId) {
    let (app, db, post) = setup_in(today, PhotoStore::new("/nonexistent/photos")).await;
    (app, db, post)
}

/// Like [`setup`], with a real (temporary) photo folder. Keep the returned
/// `TempDir` alive for the test: the folder is deleted when it's dropped.
async fn setup_with_photos(today: NaiveDate) -> (Router, Repository, PostPlanId, TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let (app, db, post) = setup_in(today, PhotoStore::new(dir.path())).await;
    (app, db, post, dir)
}

async fn setup_in(today: NaiveDate, photos: PhotoStore) -> (Router, Repository, PostPlanId) {
    let db = Repository::in_memory().await.unwrap();
    let rules = holidays::parse(holidays::BUILT_IN).unwrap();
    db.sync_holidays(&rules, 2026..=2027).await.unwrap();
    let post = demo::seed_halloween(&db, 2026).await.unwrap();

    let mut state = AppState::new(db.clone(), photos);
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
async fn static_files_are_served_with_their_types() {
    let (app, _, _) = setup(d(2026, 10, 5)).await;
    for (uri, content_type) in [
        ("/static/pico.min.css", "text/css; charset=utf-8"),
        ("/static/style.css", "text/css; charset=utf-8"),
        ("/static/colors.js", "text/javascript; charset=utf-8"),
    ] {
        let response = app
            .clone()
            .oneshot(Request::get(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CONTENT_TYPE], content_type);
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

use crate::models::{
    AccessoryId, AccessoryStatus, Color, FabricId, FabricRole, PhotoOwner, Source,
};

#[tokio::test]
async fn adding_a_fabric_redirects_to_the_list() {
    let (app, db, _) = setup(d(2026, 10, 5)).await;
    let reply = post(
        &app,
        "/fabrics",
        "name=Gold+lame&description=&color_hex=%23D4AF37&color_name=Gold\
         &color_hex=&color_name=Yellow&color_hex=&color_name=\
         &keywords=New+Year&in_stock=on&pairs=2",
    )
    .await;
    assert_eq!(reply.status, StatusCode::SEE_OTHER);
    assert_eq!(reply.location.as_deref(), Some("/fabrics"));

    let gold = db.get_fabric(FabricId(3)).await.unwrap();
    // The picked shade is stored lowercase; the blank last row is skipped.
    assert_eq!(
        gold.colors,
        vec![Color::with_hex("gold", "#d4af37"), "yellow".into()]
    );
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
    assert!(
        html.contains("class=\"swatch\" style=\"background: #d4af37\""),
        "the list shows the swatch"
    );
}

#[tokio::test]
async fn a_swatch_without_a_name_is_explained() {
    let (app, _, _) = setup(d(2026, 10, 5)).await;
    let reply = post(
        &app,
        "/fabrics",
        "name=Mystery&color_hex=%23F4C2C2&color_name=",
    )
    .await;
    assert_eq!(reply.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(reply.html.contains("Every swatch needs a name"));
}

#[tokio::test]
async fn a_fabric_form_mistake_keeps_what_was_typed() {
    let (app, db, _) = setup(d(2026, 10, 5)).await;
    let reply = post(
        &app,
        "/fabrics",
        "name=+&color_hex=%23008080&color_name=teal&pairs=1",
    )
    .await;

    assert_eq!(reply.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(reply.html.contains("Give the fabric a name."));
    assert!(
        reply.html.contains("value=\"teal\""),
        "typed colors are kept"
    );
    assert!(
        reply.html.contains("name=\"color_hex\" value=\"#008080\""),
        "picked swatches are kept"
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
    // In use, so the delete button is replaced by what's in the way.
    assert!(html.contains("used by <a href=\"/accessories/1\">Pumpkin bow</a>"));
    assert!(!html.contains("Delete fabric"));

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
    assert!(reply.html.contains("Black satin is still in use"));
    // The whole chain: fabric ← both bows ← the Halloween post, which is
    // the place to start (listed once even though it blocks both bows).
    assert!(
        reply
            .html
            .contains("used by <a href=\"/accessories/2\">Bat bow</a>")
    );
    assert!(
        reply
            .html
            .contains("used by <a href=\"/accessories/1\">Pumpkin bow</a>")
    );
    assert!(
        reply
            .html
            .contains("which is in <a href=\"/posts/1\">Halloween post (Oct 31)</a>")
    );
    let start_here = reply.html.split("Start here:").nth(1).unwrap();
    assert_eq!(start_here.matches("href=\"/posts/1\"").count(), 1);

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
        "kind=bow&status=idea&name=Candy+corn+bow&description=&color_hex=&color_name=orange\
         &color_hex=&color_name=white&main_fabric=1&middle_fabric=&accent_fabric=2&source=bought&brand=Etsy\
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
        "kind=bow&status=in_progress&name=Pumpkin+bow&color_hex=&color_name=orange&main_fabric=1\
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
    assert!(reply.html.contains("Bat bow is still in use"));
    assert!(
        reply
            .html
            .contains("in <a href=\"/posts/1\">Halloween post (Oct 31)</a>")
    );
}

#[tokio::test]
async fn unused_things_still_get_a_delete_button() {
    let (app, db, _) = setup(d(2026, 10, 5)).await;
    let trip = db
        .create_event(&NewEvent {
            name: "Vet visit".into(),
            description: String::new(),
            kind: EventKind::Outing,
            start: d(2026, 11, 2),
            end: None,
            keywords: vec![],
        })
        .await
        .unwrap();
    let (_, html) = get(&app, &format!("/events/{}/edit", trip.0)).await;
    assert!(html.contains("Delete event"));
    assert!(!html.contains("Can&#39;t delete"));
}

#[tokio::test]
async fn accessory_colors_can_come_from_its_fabrics() {
    let (app, db, _) = setup(d(2026, 10, 5)).await;
    // New accessories start with the switch on.
    let (_, html) = get(&app, "/accessories/new").await;
    assert!(html.contains("name=\"colors_from_fabrics\" role=\"switch\" checked"));

    // Orange velvet main + black satin accent, plus a white trim typed in.
    let reply = post(
        &app,
        "/accessories",
        "kind=bow&status=idea&name=Candy+corn+bow&colors_from_fabrics=on&color_hex=&color_name=White\
         &main_fabric=1&middle_fabric=&accent_fabric=2&source=handmade&partner=",
    )
    .await;
    assert_eq!(reply.status, StatusCode::SEE_OTHER);
    let candy = db.get_accessory(AccessoryId(4)).await.unwrap();
    assert!(candy.colors_from_fabrics);
    assert_eq!(
        candy.colors,
        vec!["white".into()],
        "only the extra color is stored"
    );

    let (_, html) = get(&app, "/accessories").await;
    let row = html
        .split("Candy corn bow")
        .nth(1)
        .unwrap()
        .split("</tr>")
        .next()
        .unwrap();
    assert!(row.contains(">orange<") && row.contains(">black<") && row.contains(">white<"));
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

// ---------------------------------------------------------------------------
// Events and post plans (M4c)
// ---------------------------------------------------------------------------

use crate::models::{EventId, Pet};

async fn halloween(db: &Repository) -> EventId {
    db.find_holiday("Halloween", 2026)
        .await
        .unwrap()
        .unwrap()
        .id
}

#[tokio::test]
async fn events_list_shows_upcoming_with_their_posts() {
    let (app, _, post) = setup(d(2026, 10, 5)).await;
    let (_, html) = get(&app, "/events").await;
    assert!(html.contains("Upcoming events"));
    assert!(html.contains("Halloween"));
    assert!(
        html.contains(&format!("href=\"/posts/{}\"", post.0)),
        "link to the planned post"
    );
    // Mochi's adoption day: Jan 11, 2026 is past, Jan 11, 2027 is upcoming.
    assert!(html.contains("Mon Jan 11, 2027"));
    assert!(!html.contains("Sun Jan 11, 2026"));

    let (_, html) = get(&app, "/events?show=past&kind=notable_day").await;
    assert!(html.contains("Sun Jan 11, 2026"));
    assert!(!html.contains("Halloween"));
}

#[tokio::test]
async fn holidays_are_read_only() {
    let (app, db, _) = setup(d(2026, 10, 5)).await;
    let id = halloween(&db).await.0;

    let (_, html) = get(&app, &format!("/events/{id}")).await;
    assert!(html.contains("data/holidays.toml"));
    assert!(!html.contains("Edit event"));

    let (status, _) = get(&app, &format!("/events/{id}/edit")).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let reply = post(
        &app,
        &format!("/events/{id}"),
        "name=Spooky&kind=holiday&start=2026-10-30",
    )
    .await;
    assert_eq!(reply.status, StatusCode::FORBIDDEN);
    assert_eq!(db.get_event(EventId(id)).await.unwrap().name, "Halloween");
}

#[tokio::test]
async fn your_own_events_can_be_added_edited_and_deleted() {
    let (app, db, _) = setup(d(2026, 10, 5)).await;
    let reply = post(
        &app,
        "/events",
        "name=Pumpkin+patch+trip&kind=outing&start=2026-10-17&end=2026-10-18&description=&keywords=Autumn",
    )
    .await;
    assert_eq!(reply.status, StatusCode::SEE_OTHER);
    let href = reply.location.unwrap();
    let id: i64 = href.trim_start_matches("/events/").parse().unwrap();

    let trip = db.get_event(EventId(id)).await.unwrap();
    assert_eq!(
        (trip.start, trip.end),
        (d(2026, 10, 17), Some(d(2026, 10, 18)))
    );
    assert_eq!(trip.keywords, vec!["autumn"]);
    let (_, html) = get(&app, "/").await;
    assert!(html.contains("🚗 Pumpkin patch trip"), "on the calendar");
    assert!(
        html.contains(&format!("href=\"/events/{id}\"")),
        "calendar links to it"
    );

    // Same start and end date is stored as a one-day event.
    let reply = post(
        &app,
        &href,
        "name=Pumpkin+patch&kind=outing&start=2026-10-17&end=2026-10-17",
    )
    .await;
    assert_eq!(reply.status, StatusCode::SEE_OTHER);
    assert_eq!(db.get_event(EventId(id)).await.unwrap().end, None);

    let reply = post(&app, &format!("{href}/delete"), "").await;
    assert_eq!(reply.location.as_deref(), Some("/events"));
    assert!(db.get_event(EventId(id)).await.is_err());
}

#[tokio::test]
async fn event_form_mistakes_are_explained() {
    let (app, _, _) = setup(d(2026, 10, 5)).await;
    let reply = post(
        &app,
        "/events",
        "name=Trip&kind=outing&start=2026-10-17&end=2026-10-10",
    )
    .await;
    assert_eq!(reply.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(
        reply
            .html
            .contains("The end date can&#39;t be before the start date.")
    );
    assert!(
        reply.html.contains("value=\"2026-10-17\""),
        "typed date kept"
    );

    let reply = post(&app, "/events", "name=&kind=outing&start=").await;
    assert!(reply.html.contains("Give the event a name."));
    assert!(reply.html.contains("Choose the date it starts."));
}

#[tokio::test]
async fn planning_a_post_from_an_event() {
    let (app, db, _) = setup(d(2026, 10, 5)).await;
    let birthday = db
        .find_holiday("Mochi's Birthday", 2026)
        .await
        .unwrap()
        .unwrap();

    let (_, html) = get(&app, &format!("/posts/new?event={}", birthday.id.0)).await;
    assert!(html.contains("Plan a post for Mochi&#39;s Birthday"));
    assert!(html.contains(&format!("<option value=\"{}\" selected>", birthday.id.0)));
    assert!(
        html.contains("value=\"2026-10-15\""),
        "date defaults to the event"
    );

    // Mochi wears the pumpkin bow; Momo the bat bow. Date left empty.
    let body = format!(
        "event={}&post_date=&mochi=1&momo=2&backdrop=balloons&notes=",
        birthday.id.0
    );
    let reply = post(&app, "/posts", &body).await;
    assert_eq!(reply.status, StatusCode::SEE_OTHER);
    let href = reply.location.unwrap();
    let id: i64 = href.trim_start_matches("/posts/").parse().unwrap();

    let plan = db.get_post_plan(PostPlanId(id)).await.unwrap();
    assert_eq!(plan.post_date, Some(d(2026, 10, 15)));
    assert_eq!(
        plan.looks
            .iter()
            .map(|l| (l.pet, l.accessory_id.0))
            .collect::<Vec<_>>(),
        vec![(Pet::Mochi, 1), (Pet::Momo, 2)]
    );

    // The redirect lands on the timeline. The bows are needed by Oct 11,
    // so it's already tight.
    let (_, html) = get(&app, &href).await;
    assert!(html.contains("Mochi&#39;s Birthday"));
    assert!(html.contains("Make Pumpkin bow"));
}

#[tokio::test]
async fn post_form_needs_a_title_or_an_event() {
    let (app, db, _) = setup(d(2026, 10, 5)).await;
    let before = db.list_post_plans().await.unwrap().len();
    let reply = post(&app, "/posts", "title=&event=&when=date&post_date=&mochi=1").await;
    assert_eq!(reply.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(
        reply
            .html
            .contains("Give the post a title, or choose an event.")
    );
    assert!(
        reply
            .html
            .contains("Choose a post date, or save it as an idea"),
        "no event means no event day to fall back on"
    );
    assert!(
        reply.html.contains("value=\"1\" checked"),
        "ticked look kept"
    );
    assert_eq!(db.list_post_plans().await.unwrap().len(), before);
}

#[tokio::test]
async fn editing_and_deleting_a_post() {
    let (app, db, post_id) = setup(d(2026, 10, 5)).await;
    let event = halloween(&db).await;

    let (_, html) = get(&app, &format!("/posts/{}/edit", post_id.0)).await;
    assert!(html.contains("Edit post"));
    assert!(
        html.contains("name=\"momo\" value=\"2\" checked"),
        "Momo's bat bow ticked"
    );

    // Drop the ghost scarf and post a day early.
    let body = format!(
        "event={}&post_date=2026-10-30&mochi=1&momo=2&backdrop=hay+bales&notes=",
        event.0
    );
    let reply = post(&app, &format!("/posts/{}", post_id.0), &body).await;
    assert_eq!(reply.status, StatusCode::SEE_OTHER);
    let plan = db.get_post_plan(post_id).await.unwrap();
    assert_eq!(plan.post_date, Some(d(2026, 10, 30)));
    assert_eq!(plan.looks.len(), 2);

    // The scarf is free now, so it can be deleted.
    let reply = post(&app, "/accessories/3/delete", "").await;
    assert_eq!(reply.status, StatusCode::SEE_OTHER);

    let reply = post(&app, &format!("/posts/{}/delete", post_id.0), "").await;
    assert_eq!(reply.location.as_deref(), Some("/posts"));
    assert!(db.get_post_plan(post_id).await.is_err());
}

// ---------------------------------------------------------------------------
// Suggestions (M5b)
// ---------------------------------------------------------------------------

/// The part of the page inside the ideas box.
fn ideas_box(html: &str) -> &str {
    let start = html.find("Ideas for").expect("page has an ideas box");
    let rest = &html[start..];
    &rest[..rest.find("</article>").unwrap()]
}

#[tokio::test]
async fn event_page_suggests_accessories_with_reasons() {
    let (app, db, _) = setup(d(2026, 10, 5)).await;
    let (_, html) = get(&app, &format!("/events/{}", halloween(&db).await.0)).await;
    let ideas = ideas_box(&html);

    // Best first: pumpkin bow (11 points), bat bow (9), ghost scarf (3).
    let pumpkin = ideas.find("Pumpkin bow").unwrap();
    let bat = ideas.find("Bat bow</a>").unwrap();
    let ghost = ideas.find("Ghost scarf").unwrap();
    assert!(pumpkin < bat && bat < ghost, "ranked by score");
    assert!(
        ideas.contains(
            "halloween, pumpkin, black (color), orange (color) · matching set with Bat bow"
        )
    );
    assert!(ideas.contains("href=\"/accessories/1\""));
}

#[tokio::test]
async fn post_form_shows_ideas_without_ticking_anything() {
    let (app, db, _) = setup(d(2026, 10, 5)).await;
    let event = halloween(&db).await.0;
    let (_, html) = get(&app, &format!("/posts/new?event={event}")).await;
    assert!(ideas_box(&html).contains("Pumpkin bow"));
    // The only thing pre-selected is the "On a date" choice, not a look.
    assert_eq!(
        html.matches("checked").count(),
        1,
        "suggestions are only suggestions"
    );
    assert!(html.contains("value=\"date\" checked"));

    // A plain new post with no event chosen has no ideas box yet.
    let (_, html) = get(&app, "/posts/new").await;
    assert!(!html.contains("Ideas for"));
}

#[tokio::test]
async fn editing_a_post_shows_ideas_for_its_event() {
    let (app, _, post_id) = setup(d(2026, 10, 5)).await;
    let (_, html) = get(&app, &format!("/posts/{}/edit", post_id.0)).await;
    assert!(html.contains("Ideas for Halloween"));
}

#[tokio::test]
async fn when_nothing_fits_fabrics_are_suggested_with_a_start_date() {
    let (app, db, _) = setup(d(2026, 10, 5)).await;
    db.create_fabric(&crate::db::NewFabric {
        name: "Leaf print cotton".into(),
        description: String::new(),
        colors: vec!["green".into()],
        keywords: vec!["leaf".into()],
        in_stock: true,
    })
    .await
    .unwrap();
    let earth_day = db.find_holiday("Earth Day", 2027).await.unwrap().unwrap();

    let (_, html) = get(&app, &format!("/events/{}", earth_day.id.0)).await;
    let ideas = ideas_box(&html);
    assert!(ideas.contains("Nothing you own fits yet"));
    assert!(ideas.contains("Leaf print cotton"));
    assert!(ideas.contains("leaf, green (color)"));
    // Post Apr 22 → shoot Apr 19 → ready Apr 18 → a 4-day bow starts Apr 15.
    assert!(ideas.contains("Start by <strong>Thu Apr 15</strong>"));
}

// ---------------------------------------------------------------------------
// Photos (M6b, M6c)
// ---------------------------------------------------------------------------

/// A real (tiny) JPEG: uploads are decoded in full, so made-up bytes won't do.
static JPEG: LazyLock<Vec<u8>> = LazyLock::new(|| sample(32, 24, ImageFormat::Jpeg));

/// Send `files` the way a browser sends a file form ("multipart/form-data"):
/// each part is separated by a boundary line and says its field name.
async fn upload(app: &Router, uri: &str, files: &[&[u8]], headers: &[(&str, &str)]) -> Reply {
    const BOUNDARY: &str = "----munchiko-test-boundary";
    let mut body = Vec::new();
    for file in files {
        body.extend_from_slice(
            format!(
                "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"photo\"; \
                 filename=\"photo.jpg\"\r\nContent-Type: image/jpeg\r\n\r\n"
            )
            .as_bytes(),
        );
        body.extend_from_slice(file);
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{BOUNDARY}--\r\n").as_bytes());

    let mut request = Request::post(uri).header(
        header::CONTENT_TYPE,
        format!("multipart/form-data; boundary={BOUNDARY}"),
    );
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let response = app
        .clone()
        .oneshot(request.body(Body::from(body)).unwrap())
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
        html: String::from_utf8_lossy(&bytes).into_owned(),
    }
}

const SAME_SITE: &[(&str, &str)] = &[("sec-fetch-site", "same-origin")];

#[tokio::test]
async fn an_uploaded_photo_is_saved_shown_and_served() {
    let (app, db, _, dir) = setup_with_photos(d(2026, 10, 5)).await;
    let reply = upload(&app, "/fabrics/1/photos", &[&JPEG], SAME_SITE).await;
    assert_eq!(reply.status, StatusCode::SEE_OTHER);
    assert_eq!(reply.location.as_deref(), Some("/fabrics/1"));

    // A record in the database, a file named after its id on disk.
    let photos = db
        .list_photos(PhotoOwner::Fabric(FabricId(1)))
        .await
        .unwrap();
    assert_eq!(photos.len(), 1);
    let id = photos[0].id.0;
    assert_eq!(
        std::fs::read(dir.path().join(format!("{id}.jpg"))).unwrap(),
        *JPEG
    );

    // Shown on the edit page and as the list's thumbnail.
    let (_, html) = get(&app, "/fabrics/1").await;
    // The gallery shows the thumbnail and links to the full photo.
    assert!(html.contains(&format!(
        "<a href=\"/photos/{id}\" target=\"_blank\"><img src=\"/photos/{id}/thumb\""
    )));
    let (_, html) = get(&app, "/fabrics").await;
    assert!(html.contains(&format!("<img class=\"thumb\" src=\"/photos/{id}/thumb\"")));

    // Served with its real type, and the browser told not to guess.
    let response = app
        .clone()
        .oneshot(
            Request::get(format!("/photos/{id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CONTENT_TYPE], "image/jpeg");
    assert_eq!(
        response.headers()[header::X_CONTENT_TYPE_OPTIONS],
        "nosniff"
    );
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert_eq!(bytes.to_vec(), *JPEG);
}

#[tokio::test]
async fn several_photos_can_be_uploaded_at_once() {
    let (app, db, post, _dir) = setup_with_photos(d(2026, 10, 5)).await;
    let png = sample(10, 10, ImageFormat::Png);
    let reply = upload(
        &app,
        &format!("/posts/{}/photos", post.0),
        &[&JPEG, &png],
        SAME_SITE,
    )
    .await;
    assert_eq!(reply.status, StatusCode::SEE_OTHER);
    assert_eq!(
        db.list_photos(PhotoOwner::PostPlan(post))
            .await
            .unwrap()
            .len(),
        2
    );
    let (_, html) = get(&app, &format!("/posts/{}", post.0)).await;
    assert_eq!(html.matches("<img src=\"/photos/").count(), 2);
    assert!(html.contains("/thumb\""), "the gallery shows thumbnails");
}

#[tokio::test]
async fn files_that_arent_photos_are_refused_and_nothing_is_saved() {
    let (app, db, _, dir) = setup_with_photos(d(2026, 10, 5)).await;

    // One good file and one bad one: all or nothing.
    let reply = upload(
        &app,
        "/accessories/1/photos",
        &[&JPEG, b"just text"],
        SAME_SITE,
    )
    .await;
    assert_eq!(reply.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(reply.html.contains("isn&#39;t a photo the app can use"));
    assert!(reply.html.contains("href=\"/accessories/1\""), "links back");

    // Starts like a JPEG, but isn't one.
    let fake = b"\xFF\xD8\xFF\xE0 pretend this is a photo";
    let reply = upload(&app, "/accessories/1/photos", &[fake], SAME_SITE).await;
    assert_eq!(reply.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(reply.html.contains("couldn&#39;t be read"));

    // These tests have no HEIC converter (like a computer without sips).
    let heic = b"\0\0\0\x18ftypheic\0\0\0\0";
    let reply = upload(&app, "/accessories/1/photos", &[heic], SAME_SITE).await;
    assert_eq!(reply.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(reply.html.contains("can&#39;t convert HEIC"));

    let reply = upload(&app, "/accessories/1/photos", &[b""], SAME_SITE).await;
    assert_eq!(reply.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(reply.html.contains("Choose a photo"));

    assert!(db.list_all_photos().await.unwrap().is_empty());
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[tokio::test]
async fn photos_over_the_size_limit_are_refused() {
    let (app, db, _, _dir) = setup_with_photos(d(2026, 10, 5)).await;
    let mut huge = JPEG.to_vec();
    huge.resize(crate::photos::MAX_PHOTO_BYTES + 1, 0);
    let reply = upload(&app, "/fabrics/1/photos", &[&huge], SAME_SITE).await;
    assert_eq!(reply.status, StatusCode::PAYLOAD_TOO_LARGE);
    assert!(reply.html.contains("up to 15 MB"));
    assert!(db.list_all_photos().await.unwrap().is_empty());
}

#[tokio::test]
async fn uploads_need_an_owner_and_our_own_page() {
    let (app, _, _, _dir) = setup_with_photos(d(2026, 10, 5)).await;
    let reply = upload(&app, "/fabrics/99/photos", &[&JPEG], SAME_SITE).await;
    assert_eq!(reply.status, StatusCode::NOT_FOUND);
    let reply = upload(
        &app,
        "/fabrics/1/photos",
        &[&JPEG],
        &[("sec-fetch-site", "cross-site")],
    )
    .await;
    assert_eq!(reply.status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn deleting_a_photo_or_its_owner_removes_the_file() {
    let (app, db, plan, dir) = setup_with_photos(d(2026, 10, 5)).await;
    upload(&app, "/fabrics/1/photos", &[&JPEG], SAME_SITE).await;
    upload(
        &app,
        &format!("/posts/{}/photos", plan.0),
        &[&JPEG],
        SAME_SITE,
    )
    .await;
    let photos = db.list_all_photos().await.unwrap();
    let (fabric_photo, post_photo) = (photos[0].id.0, photos[1].id.0);

    let reply = post(&app, &format!("/photos/{fabric_photo}/delete"), "").await;
    assert_eq!(reply.location.as_deref(), Some("/fabrics/1"));
    assert!(!dir.path().join(format!("{fabric_photo}.jpg")).exists());
    let (status, _) = get(&app, &format!("/photos/{fabric_photo}")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Deleting the post takes its photos (records and files) with it.
    assert!(dir.path().join(format!("{post_photo}.jpg")).exists());
    post(&app, &format!("/posts/{}/delete", plan.0), "").await;
    assert!(!dir.path().join(format!("{post_photo}.jpg")).exists());
    assert!(db.list_all_photos().await.unwrap().is_empty());
}

#[tokio::test]
async fn new_forms_explain_photos_come_after_saving() {
    let (app, _, _) = setup(d(2026, 10, 5)).await;
    let (_, html) = get(&app, "/fabrics/new").await;
    assert!(html.contains("You can add photos once this is saved."));
    assert!(!html.contains("multipart/form-data"));
}

#[tokio::test]
async fn bmp_and_tiff_are_stored_as_png() {
    let (app, db, _, dir) = setup_with_photos(d(2026, 10, 5)).await;
    let bmp = sample(20, 10, ImageFormat::Bmp);
    let tiff = sample(20, 10, ImageFormat::Tiff);
    let reply = upload(&app, "/fabrics/1/photos", &[&bmp, &tiff], SAME_SITE).await;
    assert_eq!(reply.status, StatusCode::SEE_OTHER);

    for photo in db.list_all_photos().await.unwrap() {
        assert_eq!(photo.image_type, ImageType::Png);
        let stored = std::fs::read(dir.path().join(format!("{}.png", photo.id.0))).unwrap();
        assert!(stored.starts_with(b"\x89PNG"));
    }
}

#[tokio::test]
async fn thumbnails_are_made_on_upload_and_served_as_jpeg() {
    let (app, db, _, dir) = setup_with_photos(d(2026, 10, 5)).await;
    let big = sample(1200, 900, ImageFormat::Png);
    upload(&app, "/fabrics/1/photos", &[&big], SAME_SITE).await;
    let id = db.list_all_photos().await.unwrap()[0].id.0;
    assert!(dir.path().join(format!("{id}.thumb.jpg")).exists());

    let response = app
        .clone()
        .oneshot(
            Request::get(format!("/photos/{id}/thumb"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.headers()[header::CONTENT_TYPE], "image/jpeg");
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let thumb = process::decode(&bytes).unwrap();
    assert_eq!((thumb.width(), thumb.height()), (400, 300));
}

#[tokio::test]
async fn photos_from_before_thumbnails_get_one_when_first_shown() {
    let (app, db, _, dir) = setup_with_photos(d(2026, 10, 5)).await;
    upload(&app, "/fabrics/1/photos", &[&JPEG], SAME_SITE).await;
    let id = db.list_all_photos().await.unwrap()[0].id.0;
    // Pretend it was uploaded by the M6b version: no thumbnail file.
    let thumb = dir.path().join(format!("{id}.thumb.jpg"));
    std::fs::remove_file(&thumb).unwrap();

    let (status, _) = get_bytes(&app, &format!("/photos/{id}/thumb")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(thumb.exists(), "made once, then kept");
}

#[tokio::test]
async fn heic_is_converted_to_jpeg_where_sips_exists() {
    let converter = HeicConverter::system();
    let HeicConverter::Sips(sips) = &converter else {
        return; // not a Mac: covered by the "can't convert HEIC" test
    };
    let heic = crate::photos::heic::tests::sample_heic(sips).await;

    let dir = tempfile::tempdir().unwrap();
    let db = Repository::in_memory().await.unwrap();
    demo::seed_halloween(&db, 2026).await.ok();
    let velvet = db
        .create_fabric(&crate::db::NewFabric {
            name: "Velvet".into(),
            description: String::new(),
            colors: vec![],
            keywords: vec![],
            in_stock: true,
        })
        .await
        .unwrap();
    let mut state = AppState::new(db.clone(), PhotoStore::new(dir.path()));
    state.heic = converter.clone();
    let app = router(state);

    let reply = upload(
        &app,
        &format!("/fabrics/{}/photos", velvet.0),
        &[&heic],
        SAME_SITE,
    )
    .await;
    assert_eq!(reply.status, StatusCode::SEE_OTHER, "{}", reply.html);
    let photo = &db.list_all_photos().await.unwrap()[0];
    assert_eq!(photo.image_type, ImageType::Jpeg);
    let stored = std::fs::read(dir.path().join(format!("{}.jpg", photo.id.0))).unwrap();
    assert!(stored.starts_with(&[0xFF, 0xD8, 0xFF]));
}

async fn get_bytes(app: &Router, uri: &str) -> (StatusCode, Vec<u8>) {
    let request = Request::get(uri).body(Body::empty()).unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (status, bytes.to_vec())
}

// ---------------------------------------------------------------------------
// Color ideas from photos (M6d)
// ---------------------------------------------------------------------------

/// A PNG of one color.
fn solid(rgb: [u8; 3]) -> Vec<u8> {
    let image = image::RgbImage::from_pixel(60, 40, image::Rgb(rgb));
    let mut bytes = Vec::new();
    image::DynamicImage::ImageRgb8(image)
        .write_to(&mut std::io::Cursor::new(&mut bytes), ImageFormat::Png)
        .unwrap();
    bytes
}

#[tokio::test]
async fn photo_colors_are_offered_until_they_have_a_swatch() {
    let (app, db, _, _dir) = setup_with_photos(d(2026, 10, 5)).await;
    // No photos, no ideas.
    let (_, html) = get(&app, "/fabrics/1").await;
    assert!(!html.contains("From your photos"));

    // Orange velvet (fabric 1) has "orange" with no swatch yet.
    upload(
        &app,
        "/fabrics/1/photos",
        &[&solid([245, 130, 30])],
        SAME_SITE,
    )
    .await;
    let (_, html) = get(&app, "/fabrics/1").await;
    assert!(html.contains("From your photos"));
    // The shade is read from the JPEG thumbnail, which shifts colors by a
    // level or two (invisible, but not byte-for-byte #f5821e).
    let prefix = "data-name=\"orange\" data-hex=\"";
    let at = html.find(prefix).expect("orange offered") + prefix.len();
    let hex = &html[at..at + 7];
    let channel = |i: usize| i64::from_str_radix(&hex[i..i + 2], 16).unwrap();
    let off = [(1, 0xf5), (3, 0x82), (5, 0x1e)]
        .iter()
        .map(|&(i, want)| (channel(i) - want).abs())
        .max()
        .unwrap();
    assert!(off <= 4, "{hex} is within 4 levels of #f5821e");

    // Once "orange" has a swatch there's nothing left to offer.
    let mut velvet = db.get_fabric(FabricId(1)).await.unwrap();
    velvet.colors = vec![Color::with_hex("orange", "#f5821e")];
    db.update_fabric(
        velvet.id,
        &crate::db::NewFabric {
            name: velvet.name,
            description: velvet.description,
            colors: velvet.colors,
            keywords: velvet.keywords,
            in_stock: velvet.in_stock,
        },
    )
    .await
    .unwrap();
    let (_, html) = get(&app, "/fabrics/1").await;
    assert!(!html.contains("From your photos"));
}

#[tokio::test]
async fn accessory_photos_offer_colors_its_fabrics_dont_already_cover() {
    let (app, _, _, _dir) = setup_with_photos(d(2026, 10, 5)).await;
    // The Ghost scarf (accessory 3): a white scarf with a black print.
    let mut pixels = image::RgbImage::from_pixel(50, 50, image::Rgb([250, 250, 250]));
    for x in 0..50 {
        for y in 0..20 {
            pixels.put_pixel(x, y, image::Rgb([20, 20, 20]));
        }
    }
    let mut png = Vec::new();
    image::DynamicImage::ImageRgb8(pixels)
        .write_to(&mut std::io::Cursor::new(&mut png), ImageFormat::Png)
        .unwrap();
    upload(&app, "/accessories/3/photos", &[&png], SAME_SITE).await;

    let (_, html) = get(&app, "/accessories/3").await;
    let white = html.find("data-name=\"white\"").expect("white offered");
    let black = html.find("data-name=\"black\"").expect("black offered");
    assert!(white < black, "the bigger color first");
}

// ---------------------------------------------------------------------------
// Post ideas: no date yet (M8a)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_idea_is_saved_without_a_date_and_scheduled_later() {
    let (app, db, _) = setup(d(2026, 10, 5)).await;
    // Momo in the Bat bow (accessory 2: not made yet, 2 days to make).
    let reply = post(
        &app,
        "/posts",
        "title=Pok%C3%A9mon+fossil+exhibit&event=&when=idea&post_date=&momo=2\
         &backdrop=museum+props&notes=",
    )
    .await;
    assert_eq!(reply.status, StatusCode::SEE_OTHER);
    let href = reply.location.unwrap();
    let id = PostPlanId(href.trim_start_matches("/posts/").parse().unwrap());
    assert!(db.get_post_plan(id).await.unwrap().is_idea());

    // Its page: what's left to do and the soonest it could go up.
    // 2 days making + ready the day before the shoot + 3 days' editing = 5 days.
    let (_, html) = get(&app, &href).await;
    assert!(html.contains("<h2>Pokémon fossil exhibit</h2>"));
    assert!(html.contains("An idea: no date yet"));
    assert!(html.contains("Still to do: Make Bat bow"));
    assert!(html.contains("could post on <strong>Sat Oct 10, 2026</strong>"));
    assert!(html.contains("name=\"post_date\" value=\"2026-10-10\""));
    assert!(!html.contains("Prep timeline"));

    // Listed under Ideas, and kept off the calendar (it has no dates).
    let (_, html) = get(&app, "/posts").await;
    let ideas = &html[html.find("<h3>Ideas</h3>").unwrap()..];
    assert!(ideas.contains("Pokémon fossil exhibit"));
    assert!(ideas.contains("Oct 10"));
    let (_, html) = get(&app, "/").await;
    assert!(!html.contains("fossil"));
    assert!(!html.contains("Can&#39;t plan"), "an idea isn't a problem");

    // Schedule it: now it's a normal post with a timeline on the calendar.
    let reply = post(&app, &format!("{href}/schedule"), "post_date=2026-10-20").await;
    assert_eq!(reply.location.as_deref(), Some(href.as_str()));
    let (_, html) = get(&app, &href).await;
    assert!(html.contains("Publishes Tue Oct 20, 2026"));
    assert!(html.contains("Prep timeline"));
    assert!(html.contains("Photo shoot with Momo"), "only Momo is in it");
    let (_, html) = get(&app, "/").await;
    assert!(html.contains("Post: Pokémon fossil exhibit"));
}

#[tokio::test]
async fn an_idea_can_belong_to_an_event_and_be_edited_back_into_an_idea() {
    let (app, db, post_id) = setup(d(2026, 10, 5)).await;
    let event = halloween(&db).await.0;

    // An extra Halloween idea, no title: it's called after the event.
    let reply = post(
        &app,
        "/posts",
        &format!("title=&event={event}&when=idea&post_date=&mochi=2&backdrop=&notes="),
    )
    .await;
    assert_eq!(reply.status, StatusCode::SEE_OTHER);
    let (_, html) = get(&app, &format!("/events/{event}")).await;
    assert!(html.contains("Idea (no date yet)"));

    // The existing Halloween post, edited to "no date yet".
    let reply = post(
        &app,
        &format!("/posts/{}", post_id.0),
        &format!("title=&event={event}&when=idea&post_date=2026-10-31&mochi=1&backdrop=&notes="),
    )
    .await;
    assert_eq!(reply.status, StatusCode::SEE_OTHER);
    assert_eq!(db.get_post_plan(post_id).await.unwrap().post_date, None);

    // The edit form remembers it's an idea.
    let (_, html) = get(&app, &format!("/posts/{}/edit", post_id.0)).await;
    assert!(html.contains("value=\"idea\" checked"));
    assert!(html.contains("Edit post idea"));
}

#[tokio::test]
async fn new_idea_button_starts_the_form_as_an_idea() {
    let (app, _, _) = setup(d(2026, 10, 5)).await;
    let (_, html) = get(&app, "/posts").await;
    assert!(html.contains("href=\"/posts/new?idea=1\""));
    let (_, html) = get(&app, "/posts/new?idea=1").await;
    assert!(html.contains("New post idea"));
    assert!(html.contains("value=\"idea\" checked"));
    // Past events are offered too, for ideas from an outing that's over.
    assert!(html.contains("Past: "));
}

#[tokio::test]
async fn accessory_page_shows_the_colors_it_gets_from_its_fabrics() {
    let (app, _, _) = setup(d(2026, 10, 5)).await;
    // The Pumpkin bow: orange velvet (main) and black satin (middle).
    let (_, html) = get(&app, "/accessories/1").await;
    let start = html
        .find("class=\"fabric-colors\"")
        .expect("shown under the switch");
    let shown = &html[start..html[start..].find("</p>").unwrap() + start];
    let orange = shown.find(">orange</span>").expect("orange shown");
    let black = shown.find(">black</span>").expect("black shown");
    assert!(orange < black, "main fabric's colors first");
    // Every fabric's chips are in the page, for the script to swap in.
    assert!(html.contains("<template class=\"fabric-chip-set\" data-fabric=\"1\">"));
    assert!(html.contains("<template class=\"fabric-chip-set\" data-fabric=\"2\">"));

    // The Ghost scarf doesn't use fabric colors: the line starts hidden.
    let (_, html) = get(&app, "/accessories/3").await;
    assert!(html.contains("<p class=\"fabric-colors\" hidden>"));
}
