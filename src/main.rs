//! Milestone 3 demo: sync the holidays file into SQLite, list what's coming
//! up, and print each post's prep schedule. On the first run there are no
//! posts, so we save the Halloween example first; later runs read it back.
//! This gets replaced by the web server in Milestone 4.

use std::error::Error;
use std::path::Path;

use chrono::{Datelike, Duration, Local};
use munchiko_calendar::db::{NewAccessory, NewFabric, NewPostPlan, Repository};
use munchiko_calendar::holidays;
use munchiko_calendar::models::*;
use munchiko_calendar::planner::{Inventory, LeadTimes, TaskStatus, build_timeline};

/// Where the data lives unless `DATABASE_URL` says otherwise.
const DEFAULT_DATABASE_URL: &str = "sqlite:munchiko.db";

/// The editable holidays list (the built-in copy is used if it's missing).
const HOLIDAYS_FILE: &str = "data/holidays.toml";

/// How far ahead the "coming up" list looks.
const LOOK_AHEAD_DAYS: i64 = 60;

#[tokio::main]
async fn main() {
    if let Err(e) = run().await {
        eprintln!("Error: {e}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), Box<dyn Error>> {
    let url = std::env::var("DATABASE_URL").unwrap_or_else(|_| DEFAULT_DATABASE_URL.into());
    let db = Repository::connect(&url).await?;
    let today = Local::now().date_naive();

    // Keep this year's and next year's holidays in step with the file.
    let (rules, loaded_from) = holidays::load(Path::new(HOLIDAYS_FILE))?;
    let this_year = today.year();
    let sync = db.sync_holidays(&rules, this_year..=this_year + 1).await?;
    println!(
        "Holidays from {loaded_from}: {} added, {} updated, {} removed",
        sync.added, sync.updated, sync.removed
    );
    if sync.kept_for_plans > 0 {
        println!(
            "  ({} no longer in the file but kept because a post is planned)",
            sync.kept_for_plans
        );
    }

    if db.list_post_plans().await?.is_empty() {
        seed_halloween(&db, this_year).await?;
        println!("(First run: saved the Halloween example to {url})");
    }

    let horizon = today + Duration::days(LOOK_AHEAD_DAYS);
    println!("\nComing up in the next {LOOK_AHEAD_DAYS} days:");
    for event in db.list_events_between(today, horizon).await? {
        let when = match event.end {
            Some(end) => format!(
                "{} – {}",
                event.start.format("%b %-d"),
                end.format("%b %-d")
            ),
            None => event.start.format("%a %b %-d").to_string(),
        };
        println!("  {when:<16} {}", event.name);
    }
    println!();

    let fabrics = db.list_fabrics().await?;
    let accessories = db.list_accessories().await?;
    let inventory = Inventory {
        accessories: &accessories,
        fabrics: &fabrics,
    };
    for plan in db.list_post_plans().await? {
        let event = db.get_event(plan.event_id).await?;
        let timeline = build_timeline(&plan, &inventory, &LeadTimes::default())?;

        println!(
            "{} post — publishes {}",
            event.name,
            timeline.post_date.format("%a %b %-d")
        );
        println!(
            "   Photo shoot: {}",
            timeline.shoot_date.format("%a %b %-d")
        );
        match timeline.slack_days(today) {
            s if s > 0 => println!("   On track: {s} days before prep needs to start\n"),
            0 => println!("   Prep starts today!\n"),
            s => println!("   ⚠ Behind schedule by {} days\n", -s),
        }

        for task in &timeline.tasks {
            let when = if task.start == task.due {
                task.due.format("%b %-d").to_string()
            } else {
                format!(
                    "{} – {}",
                    task.start.format("%b %-d"),
                    task.due.format("%b %-d")
                )
            };
            let status = match task.status(today) {
                TaskStatus::Upcoming { starts_in_days } => format!("in {starts_in_days}d"),
                TaskStatus::Active => "NOW".to_string(),
                TaskStatus::Overdue { days_late } => format!("{days_late}d late"),
            };
            println!("  {when:<16} {status:>9}   {}", task.title);
        }
        println!();
    }
    Ok(())
}

/// The M1 Halloween example, saved through the repository and attached to
/// this year's Halloween from the holidays file.
async fn seed_halloween(db: &Repository, year: i32) -> Result<(), Box<dyn Error>> {
    let halloween = db
        .find_holiday("Halloween", year)
        .await?
        .ok_or("the holidays file has no Halloween")?;

    let velvet = db
        .create_fabric(&NewFabric {
            name: "Orange velvet".into(),
            description: "Soft pumpkin-orange velvet".into(),
            colors: vec!["orange".into()],
            keywords: vec!["halloween".into(), "autumn".into(), "pumpkin".into()],
            in_stock: false,
        })
        .await?;
    let satin = db
        .create_fabric(&NewFabric {
            name: "Black satin".into(),
            description: "Shiny black satin".into(),
            colors: vec!["black".into()],
            keywords: vec!["halloween".into(), "formal".into()],
            in_stock: true,
        })
        .await?;
    db.pair_fabrics(velvet, satin).await?;

    let pumpkin = db
        .create_accessory(&NewAccessory {
            kind: AccessoryKind::Bow,
            name: "Pumpkin bow".into(),
            description: "Orange velvet with a black satin middle".into(),
            colors: vec!["orange".into(), "black".into()],
            fabrics: vec![
                FabricUse {
                    fabric_id: velvet,
                    role: FabricRole::Main,
                },
                FabricUse {
                    fabric_id: satin,
                    role: FabricRole::Middle,
                },
            ],
            source: Source::Handmade,
            status: AccessoryStatus::Idea,
            make_days: None,
        })
        .await?;
    let bat = db
        .create_accessory(&NewAccessory {
            kind: AccessoryKind::Bow,
            name: "Bat bow".into(),
            description: "Black satin bat-wing bow".into(),
            colors: vec!["black".into()],
            fabrics: vec![FabricUse {
                fabric_id: satin,
                role: FabricRole::Main,
            }],
            source: Source::Handmade,
            status: AccessoryStatus::Idea,
            make_days: Some(2),
        })
        .await?;
    db.pair_accessories(pumpkin, bat).await?;

    let ghost = db
        .create_accessory(&NewAccessory {
            kind: AccessoryKind::Scarf,
            name: "Ghost scarf".into(),
            description: "Glow-in-the-dark ghost print".into(),
            colors: vec!["white".into()],
            fabrics: vec![],
            source: Source::Bought { brand: None },
            status: AccessoryStatus::Idea,
            make_days: None,
        })
        .await?;

    db.create_post_plan(&NewPostPlan {
        event_id: halloween.id,
        post_date: halloween.start,
        looks: vec![
            Look {
                pet: Pet::Mochi,
                accessory_id: pumpkin,
            },
            Look {
                pet: Pet::Mochi,
                accessory_id: ghost,
            },
            Look {
                pet: Pet::Momo,
                accessory_id: bat,
            },
        ],
        backdrop: "hay bales and mini pumpkins".into(),
        notes: String::new(),
    })
    .await?;
    Ok(())
}
