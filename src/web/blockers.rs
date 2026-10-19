//! "Why can't I delete this?" Deletes are blocked while something still uses
//! the item (the database's foreign keys guarantee that), and the blocker can
//! itself be blocked: a fabric is used by a bow, which is in a planned post.
//! This module follows those chains so the page can show all of it at once,
//! with links, and point at the item to deal with first ("Start here").
//!
//! The chain only ever goes fabric → accessory → post, or event → post, or
//! accessory → post. Posts are the top: deleting a post is never blocked.
//! Plain logic with no database or web code, so it's unit tested.

use chrono::NaiveDate;

use crate::models::{Accessory, AccessoryId, Event, EventId, FabricId, PostPlan};

/// Everything that can use something else. Loaded once per page.
pub struct Usage {
    pub accessories: Vec<Accessory>,
    pub plans: Vec<PostPlan>,
    pub events: Vec<Event>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    pub label: String,
    pub href: String,
}

/// One line of the chain, e.g. "used by Pumpkin bow" or, indented under it,
/// "which is in Halloween post (Oct 31)".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockerLine {
    pub relation: &'static str,
    pub link: Link,
    /// 0 for things using the item directly, 1 for what uses those.
    pub depth: u8,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Blockers {
    pub lines: Vec<BlockerLine>,
    /// The tops of the chains: deal with these first. No duplicates.
    pub start_here: Vec<Link>,
}

impl Blockers {
    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    fn add_start(&mut self, link: Link) {
        if !self.start_here.contains(&link) {
            self.start_here.push(link);
        }
    }
}

fn accessory_link(accessory: &Accessory) -> Link {
    Link {
        label: accessory.name.clone(),
        href: format!("/accessories/{}", accessory.id.0),
    }
}

/// "Halloween post (Oct 31)" or "Pokémon fossil exhibit (idea)", linking to the post.
fn post_link(plan: &PostPlan, events: &[Event]) -> Link {
    let event = plan
        .event_id
        .and_then(|id| events.iter().find(|e| e.id == id));
    let name = plan.name(event);
    Link {
        label: match plan.post_date {
            Some(date) => format!("{name} post ({})", short_date(date)),
            None => format!("{name} (idea)"),
        },
        href: format!("/posts/{}", plan.id.0),
    }
}

fn short_date(date: NaiveDate) -> String {
    date.format("%b %-d").to_string()
}

fn plans_wearing(accessory: AccessoryId, plans: &[PostPlan]) -> impl Iterator<Item = &PostPlan> {
    plans
        .iter()
        .filter(move |p| p.looks.iter().any(|l| l.accessory_id == accessory))
}

/// What's stopping a fabric from being deleted: the accessories made from it,
/// and the posts those accessories are in.
pub fn for_fabric(fabric: FabricId, usage: &Usage) -> Blockers {
    let mut blockers = Blockers::default();
    let users = usage
        .accessories
        .iter()
        .filter(|a| a.fabrics.iter().any(|u| u.fabric_id == fabric));
    for accessory in users {
        blockers.lines.push(BlockerLine {
            relation: "used by",
            link: accessory_link(accessory),
            depth: 0,
        });
        let mut in_a_post = false;
        for plan in plans_wearing(accessory.id, &usage.plans) {
            in_a_post = true;
            let link = post_link(plan, &usage.events);
            blockers.lines.push(BlockerLine {
                relation: "which is in",
                link: link.clone(),
                depth: 1,
            });
            blockers.add_start(link);
        }
        // Not in any post: the accessory itself is the top of this chain.
        // Delete it, or edit it to use a different fabric.
        if !in_a_post {
            blockers.add_start(accessory_link(accessory));
        }
    }
    blockers
}

/// What's stopping an accessory from being deleted: the posts it's in.
pub fn for_accessory(accessory: AccessoryId, usage: &Usage) -> Blockers {
    let mut blockers = Blockers::default();
    for plan in plans_wearing(accessory, &usage.plans) {
        let link = post_link(plan, &usage.events);
        blockers.lines.push(BlockerLine {
            relation: "in",
            link: link.clone(),
            depth: 0,
        });
        blockers.add_start(link);
    }
    blockers
}

/// What's stopping an event from being deleted: the posts planned for it.
pub fn for_event(event: EventId, usage: &Usage) -> Blockers {
    let mut blockers = Blockers::default();
    for plan in usage.plans.iter().filter(|p| p.event_id == Some(event)) {
        let link = post_link(plan, &usage.events);
        blockers.lines.push(BlockerLine {
            relation: "has",
            link: link.clone(),
            depth: 0,
        });
        blockers.add_start(link);
    }
    blockers
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{
        AccessoryKind, AccessoryStatus, EventKind, FabricRole, FabricUse, Look, Pet, PostPlanId,
        Source,
    };

    fn d(m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, m, day).unwrap()
    }

    fn bow(id: i64, name: &str, fabric_ids: &[i64]) -> Accessory {
        Accessory {
            id: AccessoryId(id),
            kind: AccessoryKind::Bow,
            name: name.into(),
            description: String::new(),
            colors: vec![],
            colors_from_fabrics: true,
            fabrics: fabric_ids
                .iter()
                .map(|&f| FabricUse {
                    fabric_id: FabricId(f),
                    role: FabricRole::Main,
                })
                .collect(),
            source: Source::Handmade,
            status: AccessoryStatus::Idea,
            pair_id: None,
            make_days: None,
        }
    }

    fn event(id: i64, name: &str, date: NaiveDate) -> Event {
        Event {
            id: EventId(id),
            name: name.into(),
            description: String::new(),
            kind: EventKind::Holiday,
            start: date,
            end: None,
            keywords: vec![],
            from_holidays_file: true,
        }
    }

    fn plan(id: i64, event: i64, date: NaiveDate, wearing: &[i64]) -> PostPlan {
        PostPlan {
            id: PostPlanId(id),
            title: String::new(),
            event_id: Some(EventId(event)),
            post_date: Some(date),
            series_id: None,
            looks: wearing
                .iter()
                .map(|&a| Look {
                    pet: Pet::Mochi,
                    accessory_id: AccessoryId(a),
                })
                .collect(),
            backdrop: String::new(),
            notes: String::new(),
        }
    }

    /// Black satin (fabric 2) is in the pumpkin and bat bows, which are both
    /// in the Halloween post; the spare bow uses it too but isn't in a post.
    fn usage() -> Usage {
        Usage {
            accessories: vec![
                bow(1, "Pumpkin bow", &[1, 2]),
                bow(2, "Bat bow", &[2]),
                bow(3, "Spare bow", &[2]),
                bow(4, "Snowflake bow", &[5]),
            ],
            plans: vec![plan(7, 1, d(10, 31), &[1, 2])],
            events: vec![
                event(1, "Halloween", d(10, 31)),
                event(2, "Earth Day", d(4, 22)),
            ],
        }
    }

    fn labels(links: &[Link]) -> Vec<&str> {
        links.iter().map(|l| l.label.as_str()).collect()
    }

    #[test]
    fn fabric_shows_the_whole_chain_and_where_to_start() {
        let blockers = for_fabric(FabricId(2), &usage());
        let lines: Vec<(u8, &str, &str)> = blockers
            .lines
            .iter()
            .map(|l| (l.depth, l.relation, l.link.label.as_str()))
            .collect();
        assert_eq!(
            lines,
            vec![
                (0, "used by", "Pumpkin bow"),
                (1, "which is in", "Halloween post (Oct 31)"),
                (0, "used by", "Bat bow"),
                (1, "which is in", "Halloween post (Oct 31)"),
                (0, "used by", "Spare bow"),
            ]
        );
        // The post blocks two bows but is listed once; the spare bow is
        // in no post, so it's a starting point itself.
        assert_eq!(
            labels(&blockers.start_here),
            vec!["Halloween post (Oct 31)", "Spare bow"]
        );
        assert_eq!(blockers.start_here[0].href, "/posts/7");
        assert_eq!(blockers.start_here[1].href, "/accessories/3");
    }

    #[test]
    fn unused_things_have_no_blockers() {
        let usage = usage();
        assert!(for_fabric(FabricId(9), &usage).is_empty());
        assert!(for_accessory(AccessoryId(3), &usage).is_empty());
        assert!(for_event(EventId(2), &usage).is_empty());
    }

    #[test]
    fn accessories_and_events_are_blocked_by_posts() {
        let usage = usage();
        assert_eq!(
            labels(&for_accessory(AccessoryId(1), &usage).start_here),
            vec!["Halloween post (Oct 31)"]
        );
        let event = for_event(EventId(1), &usage);
        assert_eq!(event.lines[0].relation, "has");
        assert_eq!(labels(&event.start_here), vec!["Halloween post (Oct 31)"]);
    }
}
