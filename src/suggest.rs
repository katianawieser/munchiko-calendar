//! Suggestions (Milestone 5): "for Earth Day, consider the leaf bow".
//!
//! For an event, every accessory gets a score for how well it fits, and
//! every suggestion carries its *reasons*, so it's easy to see why it was
//! picked (and to trust it, or not). Like the planner, this module is plain
//! logic with no database or web code.
//!
//! How scoring works:
//! - The event is turned into a set of words: its name and keywords, in
//!   lowercase, without filler like "day", "the" or "national".
//! - Plurals match their singular ("Fall Leaves" finds the leaf bow): every
//!   word on both sides goes through [`singular`] before comparing.
//! - An accessory has no keywords of its own, so it borrows: the words in its
//!   name, plus its fabrics' keywords. Its colors are
//!   [`Accessory::effective_colors`]: its fabrics' colors only if it's set to
//!   use them, plus any colors of its own.
//! - Each event word scores once: [`KEYWORD_POINTS`] if it matches a keyword,
//!   otherwise [`COLOR_POINTS`] if it matches a color.
//! - Small bonuses, only for accessories that already matched something:
//!   [`SET_POINTS`] for half of a Mochi & Momo set (one decision dresses both
//!   pets) and [`READY_POINTS`] if it's ready (no prep needed).
//!
//! When no accessory fits, [`suggest_fabrics`] scores fabrics the same way
//! and works out when you'd have to start to make a new bow in time, by
//! running the real planner on a hypothetical bow.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use chrono::NaiveDate;

use crate::models::{
    Accessory, AccessoryId, AccessoryKind, AccessoryStatus, Event, Fabric, FabricId, FabricRole,
    FabricUse, Look, Pet, PostPlan, PostPlanId, Source,
};
use crate::planner::{Inventory, LeadTimes, build_timeline};

/// An event word matched a keyword (or a word in the name).
pub const KEYWORD_POINTS: u32 = 3;
/// An event word matched a color.
pub const COLOR_POINTS: u32 = 2;
/// Bonus: part of a matching Mochi & Momo set.
pub const SET_POINTS: u32 = 1;
/// Bonus: ready to wear, no prep needed.
pub const READY_POINTS: u32 = 1;

/// Words too common to mean anything when matching.
const FILLER: &[&str] = &[
    "a",
    "an",
    "and",
    "the",
    "of",
    "for",
    "with",
    "to",
    "in",
    "on",
    "day",
    "days",
    "week",
    "national",
    "international",
    "world",
    "first",
    "bow",
    "bows",
    "scarf",
    "costume",
];

/// Why something was suggested.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reason {
    Keyword(String),
    Color(String),
    MatchingSet { partner: String },
    Ready,
}

impl fmt::Display for Reason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Reason::Keyword(word) => write!(f, "{word}"),
            Reason::Color(color) => write!(f, "{color} (color)"),
            Reason::MatchingSet { partner } => write!(f, "matching set with {partner}"),
            Reason::Ready => write!(f, "ready to wear"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccessorySuggestion {
    pub accessory_id: AccessoryId,
    pub name: String,
    pub score: u32,
    pub reasons: Vec<Reason>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FabricSuggestion {
    pub fabric_id: FabricId,
    pub name: String,
    pub score: u32,
    pub reasons: Vec<Reason>,
    /// The last day to start (buying the fabric if needed, then making a bow)
    /// for a post on the given date, using the planner's lead times.
    pub start_by: NaiveDate,
}

// ---------------------------------------------------------------------------
// Words
// ---------------------------------------------------------------------------

/// Plurals that don't follow a rule.
const IRREGULAR: &[(&str, &str)] = &[
    ("leaves", "leaf"),
    ("scarves", "scarf"),
    ("wolves", "wolf"),
    ("elves", "elf"),
    ("halves", "half"),
    ("loaves", "loaf"),
    ("shelves", "shelf"),
    ("calves", "calf"),
    ("knives", "knife"),
    ("wives", "wife"),
    ("lives", "life"),
    ("mice", "mouse"),
    ("geese", "goose"),
    ("teeth", "tooth"),
    ("feet", "foot"),
    ("children", "child"),
    ("men", "man"),
    ("women", "woman"),
    ("people", "person"),
    ("cacti", "cactus"),
    ("fungi", "fungus"),
    ("tomatoes", "tomato"),
    ("potatoes", "potato"),
    ("mangoes", "mango"),
    ("heroes", "hero"),
];

/// Words that end in "s" but aren't plurals.
const NOT_PLURAL: &[&str] = &[
    "christmas",
    "news",
    "series",
    "species",
    "always",
    "canvas",
    "lens",
    "plus",
    "atlas",
];

/// The singular of an English word, by simple rules. It doesn't need to be
/// perfect, only consistent: both sides of a comparison go through it, so as
/// long as "pumpkins" and "pumpkin" end up the same, they match.
///
/// (A full "stemmer" such as Porter's would also strip "-ing" and "-ed", but it
/// produces non-words like "christma" and "berri", which are harder to explain
/// and show up in reasons. Plurals are what keywords actually differ by.)
pub fn singular(word: &str) -> String {
    if let Some((_, one)) = IRREGULAR.iter().find(|(many, _)| *many == word) {
        return one.to_string();
    }
    // Short words ("gas", "bus", "yes") and known exceptions stay as they are.
    if word.len() <= 3 || NOT_PLURAL.contains(&word) {
        return word.to_string();
    }
    // berries → berry, bunnies → bunny (but "pies" → "pie", below).
    if let Some(stem) = word.strip_suffix("ies").filter(|stem| stem.len() >= 2) {
        return format!("{stem}y");
    }
    // peaches → peach, wishes → wish, glasses → glass, boxes → box.
    for ending in ["ches", "shes", "sses", "xes"] {
        if word.ends_with(ending) {
            return word[..word.len() - 2].to_string();
        }
    }
    // dress, cactus, iris: these end in "s" without being plural.
    if word.ends_with("ss") || word.ends_with("us") || word.ends_with("is") {
        return word.to_string();
    }
    // pumpkins → pumpkin, ghosts → ghost, tacos → taco.
    match word.strip_suffix('s') {
        Some(stem) => stem.to_string(),
        None => word.to_string(),
    }
}

/// Split text into lowercase words, dropping filler and single letters.
/// "Mochi's Birthdays" → "mochi", "birthdays".
fn split_words<'a>(texts: impl IntoIterator<Item = &'a String>) -> impl Iterator<Item = String> {
    texts
        .into_iter()
        .flat_map(|text| {
            text.split(|c: char| !c.is_alphanumeric())
                .map(str::to_lowercase)
                .collect::<Vec<_>>()
        })
        .filter(|w| w.len() > 1 && !FILLER.contains(&w.as_str()))
}

/// The words of an accessory or fabric, in singular form, for matching.
fn words<'a>(texts: impl IntoIterator<Item = &'a String>) -> BTreeSet<String> {
    split_words(texts).map(|w| singular(&w)).collect()
}

/// The words that describe an event: singular form (for matching) → the
/// word as the event wrote it (for showing in reasons).
fn event_words(event: &Event) -> BTreeMap<String, String> {
    let mut found = BTreeMap::new();
    for word in split_words(std::iter::once(&event.name).chain(&event.keywords)) {
        found.entry(singular(&word)).or_insert(word);
    }
    found
}

/// Score one thing (an accessory or a fabric) against the event's words.
/// Each event word counts once, as a keyword if possible, else as a color.
fn match_words(
    event_words: &BTreeMap<String, String>,
    keywords: &BTreeSet<String>,
    colors: &BTreeSet<String>,
) -> (u32, Vec<Reason>) {
    let mut score = 0;
    let mut reasons = Vec::new();
    for (word, as_written) in event_words {
        if keywords.contains(word) {
            score += KEYWORD_POINTS;
            reasons.push(Reason::Keyword(as_written.clone()));
        } else if colors.contains(word) {
            score += COLOR_POINTS;
            reasons.push(Reason::Color(as_written.clone()));
        }
    }
    (score, reasons)
}

/// Best first; ties go alphabetically so the order never jumps around.
fn rank<T>(items: &mut [T], key: impl Fn(&T) -> (u32, &str)) {
    items.sort_by(|a, b| {
        let (sa, na) = key(a);
        let (sb, nb) = key(b);
        sb.cmp(&sa).then_with(|| na.cmp(nb))
    });
}

/// Accessories that fit `event`, best first. Only accessories that match at
/// least one word are included; the bonuses alone don't make a suggestion.
pub fn suggest_accessories(
    event: &Event,
    accessories: &[Accessory],
    fabrics: &[Fabric],
) -> Vec<AccessorySuggestion> {
    let wanted = event_words(event);
    let mut suggestions: Vec<AccessorySuggestion> = accessories
        .iter()
        .filter_map(|acc| {
            let used: Vec<&Fabric> = acc
                .fabrics
                .iter()
                .filter_map(|u| fabrics.iter().find(|f| f.id == u.fabric_id))
                .collect();
            let keywords =
                words(std::iter::once(&acc.name).chain(used.iter().flat_map(|f| &f.keywords)));
            // Only color names count; swatches are for people, not matching.
            let effective = acc.effective_colors(fabrics);
            let colors = words(effective.iter().map(|c| &c.name));

            let (mut score, mut reasons) = match_words(&wanted, &keywords, &colors);
            if score == 0 {
                return None;
            }
            if let Some(partner) = acc
                .pair_id
                .and_then(|p| accessories.iter().find(|a| a.id == p))
            {
                score += SET_POINTS;
                reasons.push(Reason::MatchingSet {
                    partner: partner.name.clone(),
                });
            }
            if acc.status == AccessoryStatus::Ready {
                score += READY_POINTS;
                reasons.push(Reason::Ready);
            }
            Some(AccessorySuggestion {
                accessory_id: acc.id,
                name: acc.name.clone(),
                score,
                reasons,
            })
        })
        .collect();
    rank(&mut suggestions, |s| (s.score, &s.name));
    suggestions
}

/// Fabrics that fit `event`, best first, each with the date you'd have to
/// start to make a new bow from it for a post on `post_date`.
pub fn suggest_fabrics(
    event: &Event,
    fabrics: &[Fabric],
    post_date: NaiveDate,
    lead: &LeadTimes,
) -> Vec<FabricSuggestion> {
    let wanted = event_words(event);
    let mut suggestions: Vec<FabricSuggestion> = fabrics
        .iter()
        .filter_map(|fabric| {
            let keywords = words(std::iter::once(&fabric.name).chain(&fabric.keywords));
            let colors = words(fabric.colors.iter().map(|c| &c.name));
            let (score, reasons) = match_words(&wanted, &keywords, &colors);
            (score > 0).then(|| FabricSuggestion {
                fabric_id: fabric.id,
                name: fabric.name.clone(),
                score,
                reasons,
                start_by: start_by_for_new_bow(fabric, post_date, lead),
            })
        })
        .collect();
    rank(&mut suggestions, |s| (s.score, &s.name));
    suggestions
}

/// Ask the planner: if we made a brand-new bow from `fabric` for a post on
/// `post_date`, when would prep have to start? Reusing the planner (instead
/// of re-doing its arithmetic here) keeps the two from ever disagreeing.
fn start_by_for_new_bow(fabric: &Fabric, post_date: NaiveDate, lead: &LeadTimes) -> NaiveDate {
    let bow = Accessory {
        id: AccessoryId(0),
        kind: AccessoryKind::Bow,
        name: format!("new {} bow", fabric.name),
        description: String::new(),
        colors: vec![],
        colors_from_fabrics: true,
        fabrics: vec![FabricUse {
            fabric_id: fabric.id,
            role: FabricRole::Main,
        }],
        source: Source::Handmade,
        status: AccessoryStatus::Idea,
        pair_id: None,
        make_days: None,
    };
    let plan = PostPlan {
        id: PostPlanId(0),
        title: String::new(),
        event_id: None,
        post_date: Some(post_date),
        series_id: None,
        looks: vec![Look {
            pet: Pet::Mochi,
            accessory_id: bow.id,
        }],
        backdrop: String::new(),
        notes: String::new(),
    };
    let bows = [bow];
    let inventory = Inventory {
        accessories: &bows,
        fabrics: std::slice::from_ref(fabric),
    };
    build_timeline(&plan, &inventory, lead)
        .expect("the hypothetical bow and its fabric are both in the inventory")
        .earliest_start()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{Color, EventId, EventKind};

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    fn list(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    fn colors_of(items: &[&str]) -> Vec<Color> {
        items.iter().map(|&s| s.into()).collect()
    }

    fn event(name: &str, keywords: &[&str], start: NaiveDate) -> Event {
        Event {
            id: EventId(1),
            name: name.into(),
            description: String::new(),
            kind: EventKind::Holiday,
            start,
            end: None,
            keywords: list(keywords),
            from_holidays_file: true,
        }
    }

    fn fabric(id: i64, name: &str, colors: &[&str], keywords: &[&str], in_stock: bool) -> Fabric {
        Fabric {
            id: FabricId(id),
            name: name.into(),
            description: String::new(),
            colors: colors_of(colors),
            keywords: list(keywords),
            pairings: vec![],
            in_stock,
        }
    }

    fn accessory(id: i64, name: &str, colors: &[&str], fabric_ids: &[i64]) -> Accessory {
        Accessory {
            id: AccessoryId(id),
            kind: AccessoryKind::Bow,
            name: name.into(),
            description: String::new(),
            colors: colors_of(colors),
            colors_from_fabrics: false,
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

    fn halloween() -> Event {
        event(
            "Halloween",
            &[
                "halloween",
                "spooky",
                "pumpkin",
                "orange",
                "black",
                "ghost",
                "bat",
            ],
            d(2026, 10, 31),
        )
    }

    fn halloween_stash() -> (Vec<Accessory>, Vec<Fabric>) {
        let fabrics = vec![
            fabric(
                1,
                "Orange velvet",
                &["orange"],
                &["halloween", "autumn", "pumpkin"],
                false,
            ),
            fabric(2, "Black satin", &["black"], &["formal"], true),
        ];
        let mut pumpkin = accessory(1, "Pumpkin bow", &["orange", "black"], &[1, 2]);
        let mut bat = accessory(2, "Bat bow", &["black"], &[2]);
        pumpkin.pair_id = Some(bat.id);
        bat.pair_id = Some(pumpkin.id);
        let mut ghost = accessory(3, "Ghost scarf", &["white"], &[]);
        ghost.kind = AccessoryKind::Scarf;
        ghost.status = AccessoryStatus::Ready;
        let snowflake = accessory(4, "Snowflake bow", &["white", "blue"], &[]);
        (vec![pumpkin, bat, ghost, snowflake], fabrics)
    }

    #[test]
    fn halloween_ranks_the_pumpkin_bow_first_with_reasons() {
        let (accessories, fabrics) = halloween_stash();
        let found = suggest_accessories(&halloween(), &accessories, &fabrics);

        let ranking: Vec<(&str, u32)> = found.iter().map(|s| (s.name.as_str(), s.score)).collect();
        assert_eq!(
            ranking,
            vec![
                // halloween + pumpkin (keywords, 3 each) + orange + black
                // (colors, 2 each) + matching set (1).
                ("Pumpkin bow", 3 + 3 + 2 + 2 + 1),
                // bat (from its name, 3) + black (2) + matching set (1).
                ("Bat bow", 3 + 2 + 1),
                // ghost (3) + ready (1).
                ("Ghost scarf", 3 + 1),
            ]
        );
        assert_eq!(
            found[0].reasons,
            vec![
                Reason::Color("black".into()),
                Reason::Keyword("halloween".into()),
                Reason::Color("orange".into()),
                Reason::Keyword("pumpkin".into()),
                Reason::MatchingSet {
                    partner: "Bat bow".into()
                },
            ]
        );
    }

    #[test]
    fn nothing_matching_means_no_suggestion_even_if_ready() {
        let (mut accessories, fabrics) = halloween_stash();
        accessories[3].status = AccessoryStatus::Ready; // the snowflake bow
        let found = suggest_accessories(&halloween(), &accessories, &fabrics);
        assert!(found.iter().all(|s| s.name != "Snowflake bow"));
    }

    #[test]
    fn a_word_counts_once_and_keyword_beats_color() {
        // "orange" is both a keyword and a color of this fabric.
        let fabrics = vec![fabric(1, "Citrus", &["orange"], &["orange"], true)];
        let accessories = vec![accessory(1, "Citrus bow", &["orange"], &[1])];
        let found = suggest_accessories(
            &event("Orange Day", &["orange"], d(2026, 5, 1)),
            &accessories,
            &fabrics,
        );
        assert_eq!(found[0].score, KEYWORD_POINTS);
        assert_eq!(found[0].reasons, vec![Reason::Keyword("orange".into())]);
    }

    #[test]
    fn filler_words_and_capitals_dont_matter() {
        let accessories = vec![
            accessory(1, "Day-at-the-park bow", &[], &[]),
            accessory(2, "DOG bone bow", &[], &[]),
        ];
        let dog_day = event("National Dog Day", &["Dog", "Mochi"], d(2026, 8, 26));
        let found = suggest_accessories(&dog_day, &accessories, &[]);
        // "day", "the" and "national" don't count; "dog" matches whatever the case.
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "DOG bone bow");
        assert_eq!(found[0].reasons, vec![Reason::Keyword("dog".into())]);
    }

    #[test]
    fn possessives_in_event_names_still_match() {
        let accessories = vec![accessory(1, "Mochi party bow", &[], &[])];
        let birthday = event("Mochi's Birthday", &["birthday", "party"], d(2026, 10, 15));
        let found = suggest_accessories(&birthday, &accessories, &[]);
        assert_eq!(
            found[0].reasons,
            vec![
                Reason::Keyword("mochi".into()),
                Reason::Keyword("party".into())
            ]
        );
    }

    #[test]
    fn fabric_colors_count_only_when_the_accessory_uses_them() {
        let fabrics = vec![fabric(1, "Kelly green felt", &["green"], &[], true)];
        let mut typed = accessory(1, "Plain bow", &["white"], &[1]);
        let st_patricks = event("St. Patrick's Day", &["green"], d(2027, 3, 17));
        assert!(
            suggest_accessories(&st_patricks, std::slice::from_ref(&typed), &fabrics).is_empty()
        );

        typed.colors_from_fabrics = true;
        let found = suggest_accessories(&st_patricks, &[typed], &fabrics);
        assert_eq!(found[0].reasons, vec![Reason::Color("green".into())]);
    }

    #[test]
    fn ties_are_broken_alphabetically() {
        let accessories = vec![
            accessory(1, "Zebra green bow", &["green"], &[]),
            accessory(2, "Apple green bow", &["green"], &[]),
        ];
        let found = suggest_accessories(&event("Green", &[], d(2026, 3, 17)), &accessories, &[]);
        let names: Vec<&str> = found.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["Apple green bow", "Zebra green bow"]);
    }

    #[test]
    fn fabrics_come_with_a_start_by_date_from_the_planner() {
        let earth_day = event(
            "Earth Day",
            &["earth", "green", "blue", "leaf", "nature", "flowers"],
            d(2027, 4, 22),
        );
        let fabrics = vec![
            fabric(
                1,
                "Leaf print cotton",
                &["green"],
                &["leaf", "nature"],
                false,
            ),
            fabric(2, "Sky blue linen", &["blue"], &[], true),
            fabric(3, "Black satin", &["black"], &[], true),
        ];
        let found = suggest_fabrics(&earth_day, &fabrics, earth_day.start, &LeadTimes::default());

        let names: Vec<&str> = found.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["Leaf print cotton", "Sky blue linen"]);
        // Default lead times: shoot Apr 19, ready by Apr 18, a 4-day bow
        // starts Apr 15. The leaf print isn't in stock, so add 3 days of
        // shopping before that: Apr 12.
        assert_eq!(found[0].start_by, d(2027, 4, 12));
        assert_eq!(found[1].start_by, d(2027, 4, 15));
        assert_eq!(found[0].score, 3 + 3 + 2, "leaf + nature + green");
    }

    #[test]
    fn reasons_read_naturally() {
        let text: Vec<String> = [
            Reason::Keyword("pumpkin".into()),
            Reason::Color("orange".into()),
            Reason::MatchingSet {
                partner: "Bat bow".into(),
            },
            Reason::Ready,
        ]
        .iter()
        .map(ToString::to_string)
        .collect();
        assert_eq!(
            text,
            vec![
                "pumpkin",
                "orange (color)",
                "matching set with Bat bow",
                "ready to wear"
            ]
        );
    }

    #[test]
    fn plurals_become_singular_by_simple_rules() {
        let cases = [
            ("pumpkins", "pumpkin"),
            ("ghosts", "ghost"),
            ("tacos", "taco"),
            ("berries", "berry"),
            ("bunnies", "bunny"),
            ("pies", "pie"),
            ("peaches", "peach"),
            ("wishes", "wish"),
            ("glasses", "glass"),
            ("boxes", "box"),
            ("leaves", "leaf"),
            ("scarves", "scarf"),
            ("mice", "mouse"),
            ("tomatoes", "tomato"),
            ("reds", "red"),
            // Already singular, or only looks plural: unchanged.
            ("pumpkin", "pumpkin"),
            ("dress", "dress"),
            ("cactus", "cactus"),
            ("iris", "iris"),
            ("christmas", "christmas"),
            ("news", "news"),
            ("gas", "gas"),
        ];
        for (word, expected) in cases {
            assert_eq!(singular(word), expected, "{word}");
        }
    }

    #[test]
    fn plural_and_singular_words_match_each_other() {
        // "Fall Leaves" should find a bow made from a "leaf" fabric, and the
        // reason uses the event's own word.
        let fabrics = vec![fabric(1, "Maple print", &[], &["leaf", "autumn"], true)];
        let maple = accessory(1, "Maple bow", &[], &[1]);
        let fall = event("Fall Leaves", &[], d(2026, 10, 1));
        let found = suggest_accessories(&fall, &[maple], &fabrics);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].reasons, vec![Reason::Keyword("leaves".into())]);

        // The other way round: a "pumpkins" keyword on the fabric, "pumpkin"
        // on the event; and a plural color name ("reds") matching "Red".
        let fabrics = vec![fabric(1, "Patch", &["reds"], &["pumpkins"], true)];
        let mut patch = accessory(1, "Patch bow", &[], &[1]);
        patch.colors_from_fabrics = true;
        let harvest = event("Pumpkin Red Harvest", &[], d(2026, 10, 1));
        let found = suggest_accessories(&harvest, &[patch], &fabrics);
        assert_eq!(found[0].score, KEYWORD_POINTS + COLOR_POINTS);
    }

    #[test]
    fn words_that_only_look_plural_dont_cause_false_matches() {
        // A newspaper-print fabric has nothing to do with New Year's Eve.
        let fabrics = vec![fabric(1, "Newsprint", &[], &["news"], true)];
        let new_year = event("New Year's Eve", &["new", "year"], d(2026, 12, 31));
        assert!(
            suggest_fabrics(&new_year, &fabrics, d(2026, 12, 31), &LeadTimes::default()).is_empty()
        );
    }
}
