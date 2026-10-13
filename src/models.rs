//! Domain models, taken from the hand-written outline (page 2 of the notes).
//!
//! Design notes:
//! - IDs are "newtypes" (`FabricId(i64)`) instead of bare integers, so the
//!   compiler stops us from passing a fabric ID where a bow ID is expected.
//! - Bows, scarves and costumes share almost every field in the outline, so they
//!   are one `Accessory` type with an `AccessoryKind`. Each one is still its own
//!   separate item, and `pair_id` links two of them into a matching set
//!   ("bows need to be separate but have pair option").

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// IDs
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct FabricId(pub i64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct AccessoryId(pub i64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct EventId(pub i64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct PostPlanId(pub i64);

// ---------------------------------------------------------------------------
// The models
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Pet {
    Mochi,
    Momo,
}

impl std::fmt::Display for Pet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Pet::Mochi => write!(f, "Mochi"),
            Pet::Momo => write!(f, "Momo"),
        }
    }
}

/// A fabric in the stash (or one we still need to buy).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Fabric {
    pub id: FabricId,
    pub name: String,
    pub description: String,
    pub colors: Vec<String>,
    pub keywords: Vec<String>,
    /// Other fabrics that look good with this one.
    pub pairings: Vec<FabricId>,
    /// `false` means it's on the shopping list, not in the stash yet.
    pub in_stock: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AccessoryKind {
    Bow,
    Scarf,
    /// The "sometimes other elaborate get-ups".
    Costume,
}

impl AccessoryKind {
    /// Default number of working days to make one by hand.
    /// Individual accessories can override this with `Accessory::make_days`.
    pub fn default_make_days(self) -> u32 {
        match self {
            AccessoryKind::Bow => 4,
            AccessoryKind::Scarf => 3,
            AccessoryKind::Costume => 10,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            AccessoryKind::Bow => "bow",
            AccessoryKind::Scarf => "scarf",
            AccessoryKind::Costume => "costume",
        }
    }
}

/// Where a fabric sits on a bow: the outline lists "main" and "middle".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FabricRole {
    Main,
    Middle,
    Accent,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FabricUse {
    pub fabric_id: FabricId,
    pub role: FabricRole,
}

/// "Made / bought — brand"
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Source {
    Handmade,
    Bought { brand: Option<String> },
}

/// How far along an accessory is. "Idea" covers the "to be made" items.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AccessoryStatus {
    Idea,
    InProgress,
    Ready,
}

/// A bow, scarf or costume.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Accessory {
    pub id: AccessoryId,
    pub kind: AccessoryKind,
    pub name: String,
    pub description: String,
    /// With `colors_from_fabrics`, only extra colors on top of the fabrics'.
    /// Use [`Accessory::effective_colors`] rather than reading this directly.
    pub colors: Vec<String>,
    /// Take colors from the fabrics instead of typing them in.
    pub colors_from_fabrics: bool,
    /// Empty is allowed (bought scarves often have no tracked fabric).
    pub fabrics: Vec<FabricUse>,
    pub source: Source,
    pub status: AccessoryStatus,
    /// The matching accessory, if this is half of a Mochi & Momo set.
    pub pair_id: Option<AccessoryId>,
    /// Override for how long this particular one takes to make.
    pub make_days: Option<u32>,
}

impl Accessory {
    pub fn effective_make_days(&self) -> u32 {
        self.make_days
            .unwrap_or_else(|| self.kind.default_make_days())
    }

    /// The accessory's colors: its fabrics' colors (main fabric first) when
    /// `colors_from_fabrics` is on, then its own extra colors, without repeats.
    /// `fabrics` may be the whole stash; only the ones this accessory uses count.
    pub fn effective_colors(&self, fabrics: &[Fabric]) -> Vec<String> {
        let from_fabrics = self
            .fabrics
            .iter()
            .filter(|_| self.colors_from_fabrics)
            .filter_map(|used| fabrics.iter().find(|f| f.id == used.fabric_id))
            .flat_map(|f| &f.colors);
        let mut colors: Vec<String> = Vec::new();
        for color in from_fabrics.chain(&self.colors) {
            if !colors.contains(color) {
                colors.push(color.clone());
            }
        }
        colors
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EventKind {
    /// Christmas, Halloween, Easter...
    Holiday,
    /// Fun or awareness days: Earth Day, Ides of March, Shark Week...
    NotableDay,
    /// First day of spring, cherry blossom season...
    Seasonal,
    /// Real-life outings with the pets.
    Outing,
}

/// Something on the calendar. Single-day events have no `end`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Event {
    pub id: EventId,
    pub name: String,
    pub description: String,
    pub kind: EventKind,
    pub start: NaiveDate,
    pub end: Option<NaiveDate>,
    pub keywords: Vec<String>,
    /// True for events created from `data/holidays.toml`. The file is their
    /// source of truth, so the app treats them as read-only.
    pub from_holidays_file: bool,
}

impl Event {
    pub fn end_date(&self) -> NaiveDate {
        self.end.unwrap_or(self.start)
    }

    pub fn is_multi_day(&self) -> bool {
        self.end_date() > self.start
    }
}

/// Which pet wears what in a post.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Look {
    pub pet: Pet,
    pub accessory_id: AccessoryId,
}

/// A planned post: which event it's for, when it goes up, and what everyone wears.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PostPlan {
    pub id: PostPlanId,
    pub event_id: EventId,
    pub post_date: NaiveDate,
    pub looks: Vec<Look>,
    /// Free-text idea for the seasonal backdrop.
    pub backdrop: String,
    pub notes: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fabric(id: i64, colors: &[&str]) -> Fabric {
        Fabric {
            id: FabricId(id),
            name: format!("Fabric {id}"),
            description: String::new(),
            colors: colors.iter().map(|c| c.to_string()).collect(),
            keywords: vec![],
            pairings: vec![],
            in_stock: true,
        }
    }

    fn bow(colors: &[&str], from_fabrics: bool) -> Accessory {
        Accessory {
            id: AccessoryId(1),
            kind: AccessoryKind::Bow,
            name: "Pumpkin bow".into(),
            description: String::new(),
            colors: colors.iter().map(|c| c.to_string()).collect(),
            colors_from_fabrics: from_fabrics,
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
            pair_id: None,
            make_days: None,
        }
    }

    #[test]
    fn colors_can_come_from_fabrics_plus_extras() {
        let stash = [
            fabric(1, &["orange"]),
            fabric(2, &["black", "orange"]),
            fabric(3, &["teal"]),
        ];
        // Fabric colors in order (main first), extras after, no repeats;
        // fabric 3 isn't used by this bow so teal doesn't appear.
        assert_eq!(
            bow(&["white", "black"], true).effective_colors(&stash),
            vec!["orange", "black", "white"]
        );
        // Box unticked: exactly what was typed.
        assert_eq!(
            bow(&["white"], false).effective_colors(&stash),
            vec!["white"]
        );
    }
}
