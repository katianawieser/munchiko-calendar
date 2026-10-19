//! The scheduling engine: given a post date, work backward to find when each
//! piece of prep has to happen.
//!
//! The chain (with default lead times) looks like this:
//!
//! ```text
//!  buy fabric ──► make bow ──┐
//!                            ├──► ready day ──► photo shoot ──(editing buffer)──► publish
//!  order bought item ────────┤
//!  build backdrop ───────────┘
//! ```
//!
//! Everything has to be ready the day *before* the shoot. Bows, bought items
//! and the backdrop are prepared in parallel; the one that has to start
//! earliest is the critical path, and the gap between today and that start
//! date is the plan's *slack*.

use std::collections::{BTreeMap, HashSet};
use std::fmt;

use chrono::{Duration, NaiveDate};

use crate::models::{
    Accessory, AccessoryId, AccessoryStatus, Fabric, FabricId, Look, Pet, PostPlan, Series, Source,
};

/// How long each kind of prep takes. All values are in days.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeadTimes {
    /// Days between the photo shoot and publishing (editing, captions, backup reshoot).
    pub editing_buffer_days: u32,
    /// Days of work to put the backdrop together.
    pub backdrop_build_days: u32,
    /// Days to wait for a bought accessory to arrive.
    pub shipping_days: u32,
    /// Days allowed to go out and get fabric that isn't in stock.
    pub fabric_shopping_days: u32,
}

impl Default for LeadTimes {
    fn default() -> Self {
        Self {
            editing_buffer_days: 3,
            backdrop_build_days: 2,
            shipping_days: 7,
            fabric_shopping_days: 3,
        }
    }
}

/// The variants are listed in the order they normally happen, which is also
/// used to break ties when two tasks start on the same day.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TaskKind {
    BuyFabric,
    OrderAccessory,
    MakeAccessory,
    BuildBackdrop,
    PhotoShoot,
    Publish,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanTask {
    pub kind: TaskKind,
    pub title: String,
    /// First day to work on this.
    pub start: NaiveDate,
    /// Last day it can be finished without pushing the post back.
    pub due: NaiveDate,
    pub accessory_id: Option<AccessoryId>,
    pub fabric_id: Option<FabricId>,
}

/// Where a task stands relative to "today".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskStatus {
    Upcoming { starts_in_days: i64 },
    Active,
    Overdue { days_late: i64 },
}

impl PlanTask {
    fn new(kind: TaskKind, title: String, start: NaiveDate, due: NaiveDate) -> Self {
        Self {
            kind,
            title,
            start,
            due,
            accessory_id: None,
            fabric_id: None,
        }
    }

    pub fn status(&self, today: NaiveDate) -> TaskStatus {
        if today < self.start {
            TaskStatus::Upcoming {
                starts_in_days: (self.start - today).num_days(),
            }
        } else if today <= self.due {
            TaskStatus::Active
        } else {
            TaskStatus::Overdue {
                days_late: (today - self.due).num_days(),
            }
        }
    }
}

/// The full prep schedule for one post.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Timeline {
    pub post_date: NaiveDate,
    pub shoot_date: NaiveDate,
    /// Everything must be finished by this day (the day before the shoot).
    pub ready_by: NaiveDate,
    /// Sorted by start date.
    pub tasks: Vec<PlanTask>,
}

impl Timeline {
    /// The start date of the critical-path task.
    pub fn earliest_start(&self) -> NaiveDate {
        self.tasks
            .iter()
            .map(|t| t.start)
            .min()
            .unwrap_or(self.post_date)
    }

    /// Days of breathing room before work must begin.
    /// Negative means the plan is already behind schedule.
    pub fn slack_days(&self, today: NaiveDate) -> i64 {
        (self.earliest_start() - today).num_days()
    }
}

/// Read-only view of what's in the craft room. The planner only needs to look
/// things up, so it borrows slices rather than owning the data.
#[derive(Debug, Clone, Copy)]
pub struct Inventory<'a> {
    pub accessories: &'a [Accessory],
    pub fabrics: &'a [Fabric],
}

impl<'a> Inventory<'a> {
    pub fn accessory(&self, id: AccessoryId) -> Option<&'a Accessory> {
        self.accessories.iter().find(|a| a.id == id)
    }

    pub fn fabric(&self, id: FabricId) -> Option<&'a Fabric> {
        self.fabrics.iter().find(|f| f.id == id)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanError {
    MissingAccessory(AccessoryId),
    MissingFabric(FabricId),
    /// The post is an idea with no date yet; see [`readiness`] instead.
    NotScheduled,
}

impl fmt::Display for PlanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PlanError::MissingAccessory(id) => {
                write!(
                    f,
                    "post plan uses accessory #{} which isn't in the inventory",
                    id.0
                )
            }
            PlanError::MissingFabric(id) => {
                write!(
                    f,
                    "an accessory uses fabric #{} which isn't in the inventory",
                    id.0
                )
            }
            PlanError::NotScheduled => write!(f, "it's an idea with no date yet"),
        }
    }
}

impl std::error::Error for PlanError {}

fn days(n: u32) -> Duration {
    Duration::days(i64::from(n))
}

/// A block of `work_days` days that finishes on `due`.
/// A 1-day task starts and ends on the same day.
fn window(due: NaiveDate, work_days: u32) -> (NaiveDate, NaiveDate) {
    let work = work_days.max(1);
    (due - days(work - 1), due)
}

/// Build the prep schedule for a scheduled post. An idea (no date yet) gets
/// [`PlanError::NotScheduled`]; use [`readiness`] for those.
pub fn build_timeline(
    plan: &PostPlan,
    inventory: &Inventory<'_>,
    lead: &LeadTimes,
) -> Result<Timeline, PlanError> {
    let post_date = plan.post_date.ok_or(PlanError::NotScheduled)?;
    build_timeline_on(plan, post_date, inventory, lead)
}

/// "Photo shoot with Momo": whoever is wearing something in this post.
fn shoot_title(plan: &PostPlan) -> String {
    let in_it = |pet| plan.looks.iter().any(|l| l.pet == pet);
    match (in_it(Pet::Mochi), in_it(Pet::Momo)) {
        (true, false) => "Photo shoot with Mochi".into(),
        (false, true) => "Photo shoot with Momo".into(),
        // Both, or no looks chosen yet.
        _ => "Photo shoot with Mochi & Momo".into(),
    }
}

/// The prep schedule if the post went up on `post_date`, whatever date the
/// plan itself has (or doesn't have).
pub fn build_timeline_on(
    plan: &PostPlan,
    post_date: NaiveDate,
    inventory: &Inventory<'_>,
    lead: &LeadTimes,
) -> Result<Timeline, PlanError> {
    let shoot_date = post_date - days(lead.editing_buffer_days);
    let ready_by = shoot_date - days(1);

    let mut tasks = vec![
        PlanTask::new(
            TaskKind::Publish,
            "Publish the post".into(),
            post_date,
            post_date,
        ),
        PlanTask::new(
            TaskKind::PhotoShoot,
            shoot_title(plan),
            shoot_date,
            shoot_date,
        ),
    ];

    let (start, due) = window(ready_by, lead.backdrop_build_days);
    let backdrop_title = if plan.backdrop.trim().is_empty() {
        "Build the backdrop".to_string()
    } else {
        format!("Build the backdrop: {}", plan.backdrop.trim())
    };
    tasks.push(PlanTask::new(
        TaskKind::BuildBackdrop,
        backdrop_title,
        start,
        due,
    ));

    // Out-of-stock fabric -> the latest day we can have it in hand.
    // Collected first so a fabric used in two bows only gets bought once.
    let mut fabric_needed_by: BTreeMap<FabricId, NaiveDate> = BTreeMap::new();
    let mut seen = HashSet::new();

    for look in &plan.looks {
        if !seen.insert(look.accessory_id) {
            continue; // same accessory listed twice
        }
        let acc = inventory
            .accessory(look.accessory_id)
            .ok_or(PlanError::MissingAccessory(look.accessory_id))?;

        match (&acc.source, acc.status) {
            (_, AccessoryStatus::Ready) => {}

            (Source::Bought { .. }, _) => {
                let order_on = ready_by - days(lead.shipping_days);
                let mut task = PlanTask::new(
                    TaskKind::OrderAccessory,
                    format!("Order {} ({}) for {}", acc.name, acc.kind.label(), look.pet),
                    order_on,
                    order_on,
                );
                task.accessory_id = Some(acc.id);
                tasks.push(task);
            }

            (Source::Handmade, status) => {
                let full = acc.effective_make_days();
                // Rough rule: something already in progress is about half done.
                let (work, verb) = if status == AccessoryStatus::InProgress {
                    (full.div_ceil(2), "Finish")
                } else {
                    (full, "Make")
                };
                let (start, due) = window(ready_by, work);
                let mut task = PlanTask::new(
                    TaskKind::MakeAccessory,
                    format!(
                        "{verb} {} ({}) for {}",
                        acc.name,
                        acc.kind.label(),
                        look.pet
                    ),
                    start,
                    due,
                );
                task.accessory_id = Some(acc.id);
                tasks.push(task);

                // Fabric only matters if we haven't started cutting yet.
                if status == AccessoryStatus::Idea {
                    for used in &acc.fabrics {
                        let fabric = inventory
                            .fabric(used.fabric_id)
                            .ok_or(PlanError::MissingFabric(used.fabric_id))?;
                        if !fabric.in_stock {
                            let needed_by = start - days(1);
                            fabric_needed_by
                                .entry(fabric.id)
                                .and_modify(|d| *d = (*d).min(needed_by))
                                .or_insert(needed_by);
                        }
                    }
                }
            }
        }
    }

    for (fabric_id, needed_by) in fabric_needed_by {
        // Safe: every id in the map was looked up successfully above.
        let fabric = inventory
            .fabric(fabric_id)
            .expect("fabric was found earlier");
        let (start, due) = window(needed_by, lead.fabric_shopping_days);
        let mut task = PlanTask::new(
            TaskKind::BuyFabric,
            format!("Buy fabric: {}", fabric.name),
            start,
            due,
        );
        task.fabric_id = Some(fabric_id);
        tasks.push(task);
    }

    tasks.sort_by(|a, b| a.start.cmp(&b.start).then(a.kind.cmp(&b.kind)));

    Ok(Timeline {
        post_date,
        shoot_date,
        ready_by,
        tasks,
    })
}

// ---------------------------------------------------------------------------
// Series: several posts from one setup and one shoot
// ---------------------------------------------------------------------------

/// One post of a series, with what to call it in its publish task.
#[derive(Debug, Clone, Copy)]
pub struct SeriesPost<'a> {
    pub plan: &'a PostPlan,
    pub name: &'a str,
}

/// A series' posts as if they were one post: everyone's looks (each once),
/// the shared backdrop, and the first post's date (`None` if none is dated).
/// Planning this is planning the shared prep and shoot.
pub fn combined_plan(series: &Series, posts: &[&PostPlan]) -> PostPlan {
    let mut looks: Vec<Look> = Vec::new();
    for look in posts.iter().flat_map(|p| &p.looks) {
        if !looks.contains(look) {
            looks.push(look.clone());
        }
    }
    PostPlan {
        id: posts.first().map_or(crate::models::PostPlanId(0), |p| p.id),
        title: series.title.clone(),
        event_id: None,
        post_date: posts.iter().filter_map(|p| p.post_date).min(),
        series_id: Some(series.id),
        looks,
        backdrop: series.backdrop.clone(),
        notes: String::new(),
    }
}

/// The prep schedule for a series: the setup and one shoot in time for the
/// *first* post, then a publish task on each post's own day. Later posts need
/// no editing buffer of their own; their photos are edited while waiting.
///
/// Posts without a date are still photographed at the shoot (their looks
/// count), but get no publish task until they're scheduled.
pub fn build_series_timeline(
    series: &Series,
    posts: &[SeriesPost<'_>],
    inventory: &Inventory<'_>,
    lead: &LeadTimes,
) -> Result<Timeline, PlanError> {
    let plans: Vec<&PostPlan> = posts.iter().map(|p| p.plan).collect();
    let combined = combined_plan(series, &plans);
    let first = combined.post_date.ok_or(PlanError::NotScheduled)?;
    let mut timeline = build_timeline_on(&combined, first, inventory, lead)?;

    timeline.tasks.retain(|t| t.kind != TaskKind::Publish);
    for post in posts {
        if let Some(date) = post.plan.post_date {
            timeline.tasks.push(PlanTask::new(
                TaskKind::Publish,
                format!("Publish: {}", post.name),
                date,
                date,
            ));
        }
    }
    timeline
        .tasks
        .sort_by(|a, b| a.start.cmp(&b.start).then(a.kind.cmp(&b.kind)));
    // The series is finished once its last post is up.
    timeline.post_date = plans
        .iter()
        .filter_map(|p| p.post_date)
        .max()
        .unwrap_or(first);
    Ok(timeline)
}

// ---------------------------------------------------------------------------
// Ideas: how ready is a post that has no date yet?
// ---------------------------------------------------------------------------

/// One piece of prep an idea still needs, and how many days it takes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrepItem {
    pub kind: TaskKind,
    pub title: String,
    pub days: i64,
}

/// What an idea needs before it can go up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Readiness {
    /// The prep still to do (bows, fabric, orders, the backdrop), longest
    /// first. Empty accessory work means everything to wear is ready.
    pub prep: Vec<PrepItem>,
    /// Days from starting prep to publishing, through the critical path,
    /// the shoot and the editing buffer.
    pub days_needed: i64,
}

impl Readiness {
    /// The soonest it could be posted if prep starts `today`.
    pub fn earliest_post_date(&self, today: NaiveDate) -> NaiveDate {
        today + Duration::days(self.days_needed)
    }

    /// Nothing to make, buy or order: only the backdrop and the shoot.
    pub fn everything_to_wear_is_ready(&self) -> bool {
        self.prep.iter().all(|p| p.kind == TaskKind::BuildBackdrop)
    }
}

/// How ready an idea is, worked out by planning it for a made-up date and
/// measuring the schedule, so ideas and scheduled posts follow the same rules.
pub fn readiness(
    plan: &PostPlan,
    inventory: &Inventory<'_>,
    lead: &LeadTimes,
) -> Result<Readiness, PlanError> {
    // Any date works: only the distances between dates matter.
    let reference = NaiveDate::from_ymd_opt(2000, 1, 1).expect("a valid date");
    let timeline = build_timeline_on(plan, reference, inventory, lead)?;
    let mut prep: Vec<PrepItem> = timeline
        .tasks
        .iter()
        .filter(|t| !matches!(t.kind, TaskKind::Publish | TaskKind::PhotoShoot))
        .map(|t| PrepItem {
            kind: t.kind,
            title: t.title.clone(),
            days: (t.due - t.start).num_days() + 1,
        })
        .collect();
    prep.sort_by(|a, b| b.days.cmp(&a.days).then(a.title.cmp(&b.title)));
    Ok(Readiness {
        prep,
        days_needed: (reference - timeline.earliest_start()).num_days(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::*;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    fn fabric(id: i64, name: &str, in_stock: bool) -> Fabric {
        Fabric {
            id: FabricId(id),
            name: name.into(),
            description: String::new(),
            colors: vec![],
            keywords: vec![],
            pairings: vec![],
            in_stock,
        }
    }

    fn handmade_bow(id: i64, name: &str, fabric_ids: &[i64], status: AccessoryStatus) -> Accessory {
        Accessory {
            id: AccessoryId(id),
            kind: AccessoryKind::Bow,
            name: name.into(),
            description: String::new(),
            colors: vec![],
            fabrics: fabric_ids
                .iter()
                .map(|&f| FabricUse {
                    fabric_id: FabricId(f),
                    role: FabricRole::Main,
                })
                .collect(),
            source: Source::Handmade,
            status,
            pair_id: None,
            make_days: None,
            colors_from_fabrics: false,
        }
    }

    fn plan(post_date: NaiveDate, looks: &[(Pet, i64)]) -> PostPlan {
        PostPlan {
            id: PostPlanId(1),
            title: String::new(),
            event_id: Some(EventId(1)),
            post_date: Some(post_date),
            series_id: None,
            looks: looks
                .iter()
                .map(|&(pet, a)| Look {
                    pet,
                    accessory_id: AccessoryId(a),
                })
                .collect(),
            backdrop: "pumpkin patch".into(),
            notes: String::new(),
        }
    }

    fn tasks_of(t: &Timeline, kind: TaskKind) -> Vec<&PlanTask> {
        t.tasks.iter().filter(|task| task.kind == kind).collect()
    }

    /// The Halloween example used in the README.
    fn halloween() -> (Vec<Accessory>, Vec<Fabric>, PostPlan) {
        let fabrics = vec![
            fabric(1, "Orange velvet", false),
            fabric(2, "Black satin", true),
        ];
        let mut pumpkin = handmade_bow(1, "Pumpkin bow", &[1], AccessoryStatus::Idea);
        let mut bat = handmade_bow(2, "Bat bow", &[2], AccessoryStatus::Idea);
        pumpkin.pair_id = Some(bat.id);
        bat.pair_id = Some(pumpkin.id);
        bat.make_days = Some(2);
        let accessories = vec![pumpkin, bat];
        let p = plan(d(2026, 10, 31), &[(Pet::Mochi, 1), (Pet::Momo, 2)]);
        (accessories, fabrics, p)
    }

    #[test]
    fn works_backward_from_post_date() {
        let (accessories, fabrics, p) = halloween();
        let inv = Inventory {
            accessories: &accessories,
            fabrics: &fabrics,
        };
        let t = build_timeline(&p, &inv, &LeadTimes::default()).unwrap();

        assert_eq!(t.shoot_date, d(2026, 10, 28));
        assert_eq!(t.ready_by, d(2026, 10, 27));

        let backdrop = tasks_of(&t, TaskKind::BuildBackdrop)[0];
        assert_eq!(
            (backdrop.start, backdrop.due),
            (d(2026, 10, 26), d(2026, 10, 27))
        );

        let makes = tasks_of(&t, TaskKind::MakeAccessory);
        assert_eq!(makes.len(), 2, "paired bows are still two separate items");
        let pumpkin = makes
            .iter()
            .find(|m| m.accessory_id == Some(AccessoryId(1)))
            .unwrap();
        assert_eq!(
            (pumpkin.start, pumpkin.due),
            (d(2026, 10, 24), d(2026, 10, 27))
        );
        let bat = makes
            .iter()
            .find(|m| m.accessory_id == Some(AccessoryId(2)))
            .unwrap();
        assert_eq!(
            bat.start,
            d(2026, 10, 26),
            "per-bow make_days override is used"
        );

        // Orange velvet is out of stock: it must be bought before the pumpkin bow starts.
        let buys = tasks_of(&t, TaskKind::BuyFabric);
        assert_eq!(
            buys.len(),
            1,
            "black satin is in stock, so only one shopping trip"
        );
        assert_eq!(
            (buys[0].start, buys[0].due),
            (d(2026, 10, 21), d(2026, 10, 23))
        );

        // Fabric shopping is the critical path.
        assert_eq!(t.earliest_start(), d(2026, 10, 21));
        assert_eq!(t.tasks.first().unwrap().kind, TaskKind::BuyFabric);
    }

    #[test]
    fn ready_accessories_need_no_prep() {
        let accessories = vec![handmade_bow(
            1,
            "Classic red bow",
            &[],
            AccessoryStatus::Ready,
        )];
        let inv = Inventory {
            accessories: &accessories,
            fabrics: &[],
        };
        let t = build_timeline(
            &plan(d(2026, 12, 25), &[(Pet::Mochi, 1)]),
            &inv,
            &LeadTimes::default(),
        )
        .unwrap();
        assert!(tasks_of(&t, TaskKind::MakeAccessory).is_empty());
        assert_eq!(t.tasks.len(), 3, "just backdrop, shoot and publish");
    }

    #[test]
    fn shared_fabric_is_bought_once_by_the_earliest_need() {
        let fabrics = vec![fabric(1, "Green plaid", false)];
        let mut big = handmade_bow(1, "Big plaid bow", &[1], AccessoryStatus::Idea);
        big.make_days = Some(6);
        let small = handmade_bow(2, "Small plaid bow", &[1], AccessoryStatus::Idea);
        let accessories = vec![big, small];
        let inv = Inventory {
            accessories: &accessories,
            fabrics: &fabrics,
        };

        let t = build_timeline(
            &plan(d(2026, 12, 25), &[(Pet::Mochi, 1), (Pet::Momo, 2)]),
            &inv,
            &LeadTimes::default(),
        )
        .unwrap();

        let buys = tasks_of(&t, TaskKind::BuyFabric);
        assert_eq!(buys.len(), 1);
        // ready_by = Dec 21; big bow starts Dec 16; fabric due Dec 15.
        assert_eq!(buys[0].due, d(2026, 12, 15));
    }

    #[test]
    fn bought_accessories_get_an_order_date() {
        let accessories = vec![Accessory {
            source: Source::Bought {
                brand: Some("Etsy shop".into()),
            },
            ..handmade_bow(1, "Shamrock scarf", &[], AccessoryStatus::Idea)
        }];
        let inv = Inventory {
            accessories: &accessories,
            fabrics: &[],
        };
        let t = build_timeline(
            &plan(d(2027, 3, 17), &[(Pet::Momo, 1)]),
            &inv,
            &LeadTimes::default(),
        )
        .unwrap();
        let order = tasks_of(&t, TaskKind::OrderAccessory)[0];
        // shoot Mar 14, ready Mar 13, minus 7 days shipping.
        assert_eq!(order.start, d(2027, 3, 6));
        assert!(tasks_of(&t, TaskKind::MakeAccessory).is_empty());
    }

    #[test]
    fn in_progress_bows_take_about_half_as_long() {
        let accessories = vec![handmade_bow(
            1,
            "Snowflake bow",
            &[],
            AccessoryStatus::InProgress,
        )];
        let inv = Inventory {
            accessories: &accessories,
            fabrics: &[],
        };
        let t = build_timeline(
            &plan(d(2026, 12, 25), &[(Pet::Mochi, 1)]),
            &inv,
            &LeadTimes::default(),
        )
        .unwrap();
        let make = tasks_of(&t, TaskKind::MakeAccessory)[0];
        assert!(make.title.starts_with("Finish"));
        assert_eq!((make.due - make.start).num_days() + 1, 2); // 4 days -> 2
    }

    #[test]
    fn missing_accessory_is_an_error() {
        let inv = Inventory {
            accessories: &[],
            fabrics: &[],
        };
        let err = build_timeline(
            &plan(d(2026, 12, 25), &[(Pet::Mochi, 99)]),
            &inv,
            &LeadTimes::default(),
        )
        .unwrap_err();
        assert_eq!(err, PlanError::MissingAccessory(AccessoryId(99)));
    }

    #[test]
    fn status_and_slack_relative_to_today() {
        let (accessories, fabrics, p) = halloween();
        let inv = Inventory {
            accessories: &accessories,
            fabrics: &fabrics,
        };
        let t = build_timeline(&p, &inv, &LeadTimes::default()).unwrap();

        assert_eq!(t.slack_days(d(2026, 10, 5)), 16);
        assert_eq!(t.slack_days(d(2026, 10, 23)), -2, "behind schedule");

        let shoot = tasks_of(&t, TaskKind::PhotoShoot)[0];
        assert_eq!(
            shoot.status(d(2026, 10, 25)),
            TaskStatus::Upcoming { starts_in_days: 3 }
        );
        assert_eq!(shoot.status(d(2026, 10, 28)), TaskStatus::Active);
        assert_eq!(
            shoot.status(d(2026, 10, 30)),
            TaskStatus::Overdue { days_late: 2 }
        );
    }

    /// The same plan, but as an idea: no date yet.
    fn idea(looks: &[(Pet, i64)]) -> PostPlan {
        PostPlan {
            post_date: None,
            ..plan(d(2026, 1, 1), looks)
        }
    }

    #[test]
    fn ideas_have_no_timeline_until_scheduled() {
        let accessories = vec![handmade_bow(1, "Leaf bow", &[], AccessoryStatus::Ready)];
        let inv = Inventory {
            accessories: &accessories,
            fabrics: &[],
        };
        assert_eq!(
            build_timeline(&idea(&[(Pet::Momo, 1)]), &inv, &LeadTimes::default()),
            Err(PlanError::NotScheduled)
        );
    }

    #[test]
    fn an_idea_says_what_prep_it_needs_and_how_long_it_takes() {
        let accessories = vec![handmade_bow(1, "Fossil bow", &[], AccessoryStatus::Idea)];
        let inv = Inventory {
            accessories: &accessories,
            fabrics: &[],
        };
        let r = readiness(&idea(&[(Pet::Momo, 1)]), &inv, &LeadTimes::default()).unwrap();

        // Longest first: making the bow (4 days), then the backdrop (2 days).
        let prep: Vec<(TaskKind, i64)> = r.prep.iter().map(|p| (p.kind, p.days)).collect();
        assert_eq!(
            prep,
            vec![(TaskKind::MakeAccessory, 4), (TaskKind::BuildBackdrop, 2)]
        );
        assert!(!r.everything_to_wear_is_ready());
        // 4 days making, ready the day before the shoot, 3 days' editing:
        // prep starts 7 days before the post.
        assert_eq!(r.days_needed, 7);
        assert_eq!(r.earliest_post_date(d(2026, 10, 19)), d(2026, 10, 26));
    }

    #[test]
    fn an_idea_with_everything_ready_only_needs_the_backdrop_and_shoot() {
        let accessories = vec![handmade_bow(1, "Fossil bow", &[], AccessoryStatus::Ready)];
        let inv = Inventory {
            accessories: &accessories,
            fabrics: &[],
        };
        let r = readiness(&idea(&[(Pet::Momo, 1)]), &inv, &LeadTimes::default()).unwrap();
        assert!(r.everything_to_wear_is_ready());
        assert_eq!(r.days_needed, 5, "2 days backdrop + shoot + 3 days editing");
    }

    #[test]
    fn the_shoot_names_who_is_in_the_post() {
        let accessories = vec![
            handmade_bow(1, "Earth bow", &[], AccessoryStatus::Ready),
            handmade_bow(2, "Leaf bow", &[], AccessoryStatus::Ready),
        ];
        let inv = Inventory {
            accessories: &accessories,
            fabrics: &[],
        };
        let shoot = |looks: &[(Pet, i64)]| {
            let t =
                build_timeline(&plan(d(2027, 4, 22), looks), &inv, &LeadTimes::default()).unwrap();
            tasks_of(&t, TaskKind::PhotoShoot)[0].title.clone()
        };
        assert_eq!(shoot(&[(Pet::Momo, 1)]), "Photo shoot with Momo");
        assert_eq!(shoot(&[(Pet::Mochi, 2)]), "Photo shoot with Mochi");
        assert_eq!(
            shoot(&[(Pet::Momo, 1), (Pet::Mochi, 2)]),
            "Photo shoot with Mochi & Momo"
        );
    }

    fn earth_day_series() -> (Series, Vec<Accessory>) {
        let series = Series {
            id: SeriesId(1),
            title: "Earth Day".into(),
            backdrop: "moss and ferns".into(),
            notes: String::new(),
        };
        let accessories = vec![
            handmade_bow(1, "Leaf bow", &[], AccessoryStatus::Idea),
            handmade_bow(2, "Globe bow", &[], AccessoryStatus::Ready),
        ];
        (series, accessories)
    }

    #[test]
    fn a_series_shares_one_setup_and_shoot_before_its_first_post() {
        let (series, accessories) = earth_day_series();
        let inv = Inventory {
            accessories: &accessories,
            fabrics: &[],
        };
        // Momo (Leaf bow) Apr 20, Mochi (Globe bow) Apr 21, both Apr 22.
        let momo = plan(d(2027, 4, 20), &[(Pet::Momo, 1)]);
        let mochi = plan(d(2027, 4, 21), &[(Pet::Mochi, 2)]);
        let both = plan(d(2027, 4, 22), &[(Pet::Momo, 1), (Pet::Mochi, 2)]);
        let posts = [
            SeriesPost {
                plan: &momo,
                name: "Momo's Earth Day",
            },
            SeriesPost {
                plan: &mochi,
                name: "Mochi's Earth Day",
            },
            SeriesPost {
                plan: &both,
                name: "Earth Day together",
            },
        ];
        let t = build_series_timeline(&series, &posts, &inv, &LeadTimes::default()).unwrap();

        // One shoot, with both pets, 3 days' editing before the FIRST post.
        let shoots = tasks_of(&t, TaskKind::PhotoShoot);
        assert_eq!(shoots.len(), 1);
        assert_eq!(shoots[0].start, d(2027, 4, 17));
        assert_eq!(shoots[0].title, "Photo shoot with Mochi & Momo");
        // One backdrop, the series' own; the Leaf bow is made once.
        let backdrops = tasks_of(&t, TaskKind::BuildBackdrop);
        assert_eq!(backdrops.len(), 1);
        assert!(backdrops[0].title.ends_with("moss and ferns"));
        assert_eq!(tasks_of(&t, TaskKind::MakeAccessory).len(), 1);
        // A publish task per post, on its own day.
        let publish: Vec<(NaiveDate, &str)> = tasks_of(&t, TaskKind::Publish)
            .iter()
            .map(|p| (p.start, p.title.as_str()))
            .collect();
        assert_eq!(
            publish,
            vec![
                (d(2027, 4, 20), "Publish: Momo's Earth Day"),
                (d(2027, 4, 21), "Publish: Mochi's Earth Day"),
                (d(2027, 4, 22), "Publish: Earth Day together"),
            ]
        );
        assert_eq!(t.shoot_date, d(2027, 4, 17));
        assert_eq!(t.post_date, d(2027, 4, 22), "finished after the last post");
    }

    #[test]
    fn a_series_with_no_dates_is_still_an_idea() {
        let (series, accessories) = earth_day_series();
        let inv = Inventory {
            accessories: &accessories,
            fabrics: &[],
        };
        let momo = idea(&[(Pet::Momo, 1)]);
        let mochi = idea(&[(Pet::Mochi, 2)]);
        let posts = [
            SeriesPost {
                plan: &momo,
                name: "Momo",
            },
            SeriesPost {
                plan: &mochi,
                name: "Mochi",
            },
        ];
        assert_eq!(
            build_series_timeline(&series, &posts, &inv, &LeadTimes::default()),
            Err(PlanError::NotScheduled)
        );
        // Its readiness covers everyone's looks: the Leaf bow still needs making.
        let combined = combined_plan(&series, &[&momo, &mochi]);
        assert_eq!(combined.looks.len(), 2);
        let r = readiness(&combined, &inv, &LeadTimes::default()).unwrap();
        assert!(!r.everything_to_wear_is_ready());
        assert_eq!(r.days_needed, 7);
    }
}
