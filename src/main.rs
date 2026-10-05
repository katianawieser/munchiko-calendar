//! Milestone 1 demo: build the Halloween prep schedule and print it.
//! This gets replaced by the web server in Milestone 4.

use chrono::{Local, NaiveDate};
use munchiko_calendar::models::*;
use munchiko_calendar::planner::{Inventory, LeadTimes, TaskStatus, build_timeline};

fn main() {
    let fabrics = vec![
        Fabric {
            id: FabricId(1),
            name: "Orange velvet".into(),
            description: "Soft pumpkin-orange velvet".into(),
            colors: vec!["orange".into()],
            keywords: vec!["halloween".into(), "autumn".into(), "pumpkin".into()],
            pairings: vec![FabricId(2)],
            in_stock: false,
        },
        Fabric {
            id: FabricId(2),
            name: "Black satin".into(),
            description: "Shiny black satin".into(),
            colors: vec!["black".into()],
            keywords: vec!["halloween".into(), "formal".into()],
            pairings: vec![FabricId(1)],
            in_stock: true,
        },
    ];

    let accessories = vec![
        Accessory {
            id: AccessoryId(1),
            kind: AccessoryKind::Bow,
            name: "Pumpkin bow".into(),
            description: "Orange velvet with a black satin middle".into(),
            colors: vec!["orange".into(), "black".into()],
            fabrics: vec![
                FabricUse {
                    fabric_id: FabricId(1),
                    role: FabricRole::Main,
                },
                FabricUse {
                    fabric_id: FabricId(2),
                    role: FabricRole::Middle,
                },
            ],
            source: Source::Handmade,
            status: AccessoryStatus::Idea,
            pair_id: Some(AccessoryId(2)),
            make_days: None,
        },
        Accessory {
            id: AccessoryId(2),
            kind: AccessoryKind::Bow,
            name: "Bat bow".into(),
            description: "Black satin bat-wing bow".into(),
            colors: vec!["black".into()],
            fabrics: vec![FabricUse {
                fabric_id: FabricId(2),
                role: FabricRole::Main,
            }],
            source: Source::Handmade,
            status: AccessoryStatus::Idea,
            pair_id: Some(AccessoryId(1)),
            make_days: Some(2),
        },
        Accessory {
            id: AccessoryId(3),
            kind: AccessoryKind::Scarf,
            name: "Ghost scarf".into(),
            description: "Glow-in-the-dark ghost print".into(),
            colors: vec!["white".into()],
            fabrics: vec![],
            source: Source::Bought { brand: None },
            status: AccessoryStatus::Idea,
            pair_id: None,
            make_days: None,
        },
    ];

    let plan = PostPlan {
        id: PostPlanId(1),
        event_id: EventId(1),
        post_date: NaiveDate::from_ymd_opt(2026, 10, 31).unwrap(),
        looks: vec![
            Look {
                pet: Pet::Mochi,
                accessory_id: AccessoryId(1),
            },
            Look {
                pet: Pet::Mochi,
                accessory_id: AccessoryId(3),
            },
            Look {
                pet: Pet::Momo,
                accessory_id: AccessoryId(2),
            },
        ],
        backdrop: "hay bales and mini pumpkins".into(),
        notes: String::new(),
    };

    let inventory = Inventory {
        accessories: &accessories,
        fabrics: &fabrics,
    };
    let timeline = match build_timeline(&plan, &inventory, &LeadTimes::default()) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("Couldn't plan the post: {e}");
            std::process::exit(1);
        }
    };

    let today = Local::now().date_naive();
    println!(
        "🎃 Halloween post — publishes {}",
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
}
