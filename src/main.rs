//! Milestone 2 demo: load everything from SQLite and print each post's prep
//! schedule. On the first run the database is empty, so we save the Halloween
//! example into it first; later runs read it straight back from the file.
//! This gets replaced by the web server in Milestone 4.

use std::error::Error;

use chrono::{Local, NaiveDate};
use munchiko_calendar::db::{NewAccessory, NewEvent, NewFabric, NewPostPlan, Repository};
use munchiko_calendar::models::*;
use munchiko_calendar::planner::{Inventory, LeadTimes, TaskStatus, build_timeline};

/// Where the data lives unless `DATABASE_URL` says otherwise.
const DEFAULT_DATABASE_URL: &str = "sqlite:munchiko.db";

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

    if db.list_post_plans().await?.is_empty() {
        seed_halloween(&db).await?;
        println!("(First run: saved the Halloween example to {url})\n");
    }

    let fabrics = db.list_fabrics().await?;
    let accessories = db.list_accessories().await?;
    let inventory = Inventory {
        accessories: &accessories,
        fabrics: &fabrics,
    };
    let today = Local::now().date_naive();

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

/// The M1 Halloween example, saved through the repository.
async fn seed_halloween(db: &Repository) -> Result<(), Box<dyn Error>> {
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

    let halloween_day = NaiveDate::from_ymd_opt(2026, 10, 31).unwrap();
    let halloween = db
        .create_event(&NewEvent {
            name: "Halloween".into(),
            description: "Costumes, pumpkins and spooky backdrops".into(),
            kind: EventKind::Holiday,
            start: halloween_day,
            end: None,
            keywords: vec!["halloween".into(), "pumpkin".into(), "spooky".into()],
        })
        .await?;

    db.create_post_plan(&NewPostPlan {
        event_id: halloween,
        post_date: halloween_day,
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
