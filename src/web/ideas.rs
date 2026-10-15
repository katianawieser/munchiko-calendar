//! Turning the suggestion engine's results into the "Ideas for Halloween"
//! box on the event page and the post form.
//!
//! [`crate::suggest`] does the scoring; this module decides what's worth
//! showing: the top few accessories, and only if none fit, fabrics to make
//! something new from, with the date you'd have to start by. Plain logic,
//! unit tested.

use chrono::NaiveDate;

use crate::models::{Accessory, Event, Fabric};
use crate::planner::LeadTimes;
use crate::suggest::{self, Reason};

/// Enough to choose from without turning into a second inventory list.
pub const MAX_ACCESSORY_IDEAS: usize = 5;
pub const MAX_FABRIC_IDEAS: usize = 3;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccessoryIdea {
    pub name: String,
    pub href: String,
    /// "halloween, pumpkin, orange (color) · matching set with Bat bow"
    pub reasons: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FabricIdea {
    pub name: String,
    pub href: String,
    pub reasons: String,
    /// "Apr 12"
    pub start_by: String,
    /// The start-by date has already passed.
    pub too_late: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ideas {
    pub event_name: String,
    pub accessories: Vec<AccessoryIdea>,
    /// Only filled in when no accessory fits.
    pub fabrics: Vec<FabricIdea>,
}

impl Ideas {
    pub fn is_empty(&self) -> bool {
        self.accessories.is_empty() && self.fabrics.is_empty()
    }
}

/// Matched words first (keywords, then colors), then the bonuses after a
/// dot: "halloween, pumpkin, orange (color) · matching set with Bat bow".
/// The engine lists reasons alphabetically; this order reads better.
fn describe(reasons: &[Reason]) -> String {
    let keywords = reasons.iter().filter(|r| matches!(r, Reason::Keyword(_)));
    let colors = reasons.iter().filter(|r| matches!(r, Reason::Color(_)));
    let bonuses = reasons
        .iter()
        .filter(|r| matches!(r, Reason::MatchingSet { .. } | Reason::Ready));

    let words: Vec<String> = keywords.chain(colors).map(ToString::to_string).collect();
    let mut parts = vec![words.join(", ")];
    parts.extend(bonuses.map(ToString::to_string));
    parts.join(" · ")
}

/// Ideas for `event`, for a post going up on `post_date`.
pub fn for_event(
    event: &Event,
    post_date: NaiveDate,
    today: NaiveDate,
    accessories: &[Accessory],
    fabrics: &[Fabric],
    lead: &LeadTimes,
) -> Ideas {
    let accessory_ideas: Vec<AccessoryIdea> =
        suggest::suggest_accessories(event, accessories, fabrics)
            .into_iter()
            .take(MAX_ACCESSORY_IDEAS)
            .map(|s| AccessoryIdea {
                name: s.name,
                href: format!("/accessories/{}", s.accessory_id.0),
                reasons: describe(&s.reasons),
            })
            .collect();

    // Fabrics are the fallback: if something you own already fits, that's
    // the better suggestion.
    let fabric_ideas = if accessory_ideas.is_empty() {
        suggest::suggest_fabrics(event, fabrics, post_date, lead)
            .into_iter()
            .take(MAX_FABRIC_IDEAS)
            .map(|s| FabricIdea {
                name: s.name,
                href: format!("/fabrics/{}", s.fabric_id.0),
                reasons: describe(&s.reasons),
                start_by: s.start_by.format("%a %b %-d").to_string(),
                too_late: s.start_by < today,
            })
            .collect()
    } else {
        Vec::new()
    };

    Ideas {
        event_name: event.name.clone(),
        accessories: accessory_ideas,
        fabrics: fabric_ideas,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{
        AccessoryId, AccessoryKind, AccessoryStatus, Color, EventId, EventKind, FabricId,
        FabricRole, FabricUse, Source,
    };

    fn d(m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2027, m, day).unwrap()
    }

    fn list(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    fn colors_of(items: &[&str]) -> Vec<Color> {
        items.iter().map(|&s| s.into()).collect()
    }

    fn event(name: &str, keywords: &[&str], date: NaiveDate) -> Event {
        Event {
            id: EventId(1),
            name: name.into(),
            description: String::new(),
            kind: EventKind::Holiday,
            start: date,
            end: None,
            keywords: list(keywords),
            from_holidays_file: true,
        }
    }

    fn fabric(id: i64, name: &str, colors: &[&str], keywords: &[&str]) -> Fabric {
        Fabric {
            id: FabricId(id),
            name: name.into(),
            description: String::new(),
            colors: colors_of(colors),
            keywords: list(keywords),
            pairings: vec![],
            in_stock: true,
        }
    }

    fn bow(id: i64, name: &str, colors: &[&str]) -> Accessory {
        Accessory {
            id: AccessoryId(id),
            kind: AccessoryKind::Bow,
            name: name.into(),
            description: String::new(),
            colors: colors_of(colors),
            colors_from_fabrics: false,
            fabrics: vec![],
            source: Source::Handmade,
            status: AccessoryStatus::Idea,
            pair_id: None,
            make_days: None,
        }
    }

    #[test]
    fn reasons_read_keywords_then_colors_then_bonuses() {
        let text = describe(&[
            Reason::Color("black".into()),
            Reason::Keyword("halloween".into()),
            Reason::Color("orange".into()),
            Reason::Keyword("pumpkin".into()),
            Reason::MatchingSet {
                partner: "Bat bow".into(),
            },
            Reason::Ready,
        ]);
        assert_eq!(
            text,
            "halloween, pumpkin, black (color), orange (color) · matching set with Bat bow · ready to wear"
        );
    }

    #[test]
    fn accessories_win_and_are_capped() {
        let accessories: Vec<Accessory> = (1..=7)
            .map(|i| bow(i, &format!("Green bow {i}"), &["green"]))
            .collect();
        let fabrics = vec![fabric(1, "Shamrock cotton", &["green"], &["shamrock"])];
        let ideas = for_event(
            &event("St. Patrick's Day", &["green", "shamrock"], d(3, 17)),
            d(3, 17),
            d(1, 1),
            &accessories,
            &fabrics,
            &LeadTimes::default(),
        );
        assert_eq!(ideas.accessories.len(), MAX_ACCESSORY_IDEAS);
        assert_eq!(ideas.accessories[0].href, "/accessories/1");
        assert!(
            ideas.fabrics.is_empty(),
            "fabrics only when nothing you own fits"
        );
    }

    #[test]
    fn fabrics_are_the_fallback_with_a_start_by_date() {
        let accessories = vec![bow(1, "Snowflake bow", &["white"])];
        let fabrics = vec![
            fabric(1, "Leaf print", &["green"], &["leaf"]),
            fabric(2, "Plain white", &["white"], &[]),
        ];
        let earth_day = event("Earth Day", &["earth", "green", "leaf"], d(4, 22));
        let ideas = for_event(
            &earth_day,
            d(4, 22),
            d(1, 1),
            &accessories,
            &fabrics,
            &LeadTimes::default(),
        );

        assert!(ideas.accessories.is_empty());
        assert_eq!(ideas.fabrics.len(), 1);
        let leaf = &ideas.fabrics[0];
        assert_eq!(leaf.name, "Leaf print");
        assert_eq!(leaf.reasons, "leaf, green (color)");
        // In stock, 4-day bow, ready by Apr 18: start Apr 15.
        assert_eq!(leaf.start_by, "Thu Apr 15");
        assert!(!leaf.too_late);

        let late = for_event(
            &earth_day,
            d(4, 22),
            d(4, 20),
            &accessories,
            &fabrics,
            &LeadTimes::default(),
        );
        assert!(late.fabrics[0].too_late);
    }

    #[test]
    fn no_match_at_all_means_no_ideas() {
        let ideas = for_event(
            &event("Pi Day", &["pi", "pie"], d(3, 14)),
            d(3, 14),
            d(1, 1),
            &[bow(1, "Snowflake bow", &["white"])],
            &[fabric(1, "Plain white", &["white"], &[])],
            &LeadTimes::default(),
        );
        assert!(ideas.is_empty());
    }

    #[test]
    fn fabric_ideas_link_to_the_fabric() {
        let bow_from_fabric = Accessory {
            fabrics: vec![FabricUse {
                fabric_id: FabricId(1),
                role: FabricRole::Main,
            }],
            ..bow(1, "Plain bow", &[])
        };
        // The bow doesn't use its fabric's colors, so only the fabric matches.
        let ideas = for_event(
            &event("Earth Day", &["green"], d(4, 22)),
            d(4, 22),
            d(1, 1),
            &[bow_from_fabric],
            &[fabric(1, "Leaf print", &["green"], &[])],
            &LeadTimes::default(),
        );
        assert_eq!(ideas.fabrics[0].href, "/fabrics/1");
    }
}
