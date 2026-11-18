//! Domain models, taken from the hand-written outline (page 2 of the notes).
//!
//! Design notes:
//! - IDs are "newtypes" (`FabricId(i64)`) instead of bare integers, so the
//!   compiler stops us from passing a fabric ID where a bow ID is expected.
//! - Bows, scarves, costumes, hats and props share almost every field, so they
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct PhotoId(pub i64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SeriesId(pub i64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct OutfitId(pub i64);

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

/// A color: the words used to describe it, plus (optionally) the exact shade.
///
/// The name is what suggestions match against ("orange" for Halloween), so
/// it's always required. The hex value is only for people: "baby pink" means
/// different things to different people, and a swatch settles it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Color {
    pub name: String,
    /// Lowercase `#rrggbb`, as an `<input type="color">` sends it. Left out of
    /// the saved JSON when there is none, so old data reads the same.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hex: Option<String>,
}

impl Color {
    /// A color with just a name, no swatch yet.
    pub fn named(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            hex: None,
        }
    }

    /// A color with a swatch. `hex` must already be valid (see [`Color::parse_hex`]).
    pub fn with_hex(name: impl Into<String>, hex: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            hex: Some(hex.into()),
        }
    }

    /// "#F4C2C2" → `Some("#f4c2c2")`. Anything that isn't `#` plus six hex
    /// digits is refused, so a hex value is always safe to put in a page's CSS.
    pub fn parse_hex(text: &str) -> Option<String> {
        let digits = text.trim().strip_prefix('#')?;
        (digits.len() == 6 && digits.chars().all(|c| c.is_ascii_hexdigit()))
            .then(|| format!("#{}", digits.to_ascii_lowercase()))
    }
}

/// Lets tests and example data write `"orange".into()`.
impl From<&str> for Color {
    fn from(name: &str) -> Self {
        Color::named(name)
    }
}

/// A fabric in the stash (or one we still need to buy).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Fabric {
    pub id: FabricId,
    pub name: String,
    pub description: String,
    pub colors: Vec<Color>,
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
    /// Crowns, witch hats, party hats.
    Hat,
    /// Something the pets pose with rather than wear: a wand, a sign, a basket.
    Prop,
}

impl AccessoryKind {
    /// Default number of working days to make one by hand.
    /// Individual accessories can override this with `Accessory::make_days`.
    pub fn default_make_days(self) -> u32 {
        match self {
            AccessoryKind::Bow => 4,
            AccessoryKind::Scarf => 3,
            AccessoryKind::Costume => 10,
            AccessoryKind::Hat => 3,
            // Often bought or quick to make (a cardboard sign).
            AccessoryKind::Prop => 2,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            AccessoryKind::Bow => "bow",
            AccessoryKind::Scarf => "scarf",
            AccessoryKind::Costume => "costume",
            AccessoryKind::Hat => "hat",
            AccessoryKind::Prop => "prop",
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

/// A bow, scarf, costume, hat or prop.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Accessory {
    pub id: AccessoryId,
    pub kind: AccessoryKind,
    pub name: String,
    pub description: String,
    /// With `colors_from_fabrics`, only extra colors on top of the fabrics'.
    /// Use [`Accessory::effective_colors`] rather than reading this directly.
    pub colors: Vec<Color>,
    /// Take colors from the fabrics instead of typing them in.
    pub colors_from_fabrics: bool,
    /// Its own tags, like "batik". See [`Accessory::all_tags`] for these plus
    /// its fabrics' keywords.
    pub tags: Vec<String>,
    /// Empty is allowed (bought scarves often have no tracked fabric).
    pub fabrics: Vec<FabricUse>,
    pub source: Source,
    pub status: AccessoryStatus,
    /// The matching accessory, if this is half of a Mochi & Momo set.
    pub pair_id: Option<AccessoryId>,
    /// Override for how long this particular one takes to make.
    pub make_days: Option<u32>,
    /// How many there are (or will be, once made). At least 1.
    pub quantity: u32,
}

impl Accessory {
    /// Its own tags, then its fabrics' keywords (main fabric first), each
    /// once ignoring capitals. A bow made of batik fabric is a batik bow
    /// without tagging it twice.
    pub fn all_tags(&self, fabrics: &[Fabric]) -> Vec<String> {
        let from_fabrics = self
            .fabrics
            .iter()
            .filter_map(|u| fabrics.iter().find(|f| f.id == u.fabric_id))
            .flat_map(|f| &f.keywords);
        let mut seen = Vec::new();
        for tag in self.tags.iter().chain(from_fabrics) {
            if !seen.iter().any(|s: &String| s.eq_ignore_ascii_case(tag)) {
                seen.push(tag.clone());
            }
        }
        seen
    }

    pub fn effective_make_days(&self) -> u32 {
        self.make_days
            .unwrap_or_else(|| self.kind.default_make_days())
    }

    /// The accessory's colors: its fabrics' colors (main fabric first) when
    /// `colors_from_fabrics` is on, then its own extra colors, without repeats.
    /// A repeat is the same name; the first one wins, swatch and all.
    /// `fabrics` may be the whole stash; only the ones this accessory uses count.
    pub fn effective_colors(&self, fabrics: &[Fabric]) -> Vec<Color> {
        let from_fabrics = self
            .fabrics
            .iter()
            .filter(|_| self.colors_from_fabrics)
            .filter_map(|used| fabrics.iter().find(|f| f.id == used.fabric_id))
            .flat_map(|f| &f.colors);
        without_repeats(from_fabrics.chain(&self.colors))
    }
}

/// Colors in order, keeping only the first color of each name (so a fabric's
/// swatch wins over an accessory's bare extra of the same name).
pub fn without_repeats<'a>(colors: impl IntoIterator<Item = &'a Color>) -> Vec<Color> {
    let mut kept: Vec<Color> = Vec::new();
    for color in colors {
        if !kept.iter().any(|c| c.name == color.name) {
            kept.push(color.clone());
        }
    }
    kept
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

impl PostPlan {
    pub fn is_idea(&self) -> bool {
        self.post_date.is_none()
    }

    /// What to call the post: its title, else its event's name.
    /// `event` is the event this post is for, if any.
    pub fn name(&self, event: Option<&Event>) -> String {
        match (self.title.trim(), event) {
            ("", Some(event)) => event.name.clone(),
            ("", None) => "Untitled post".to_string(),
            (title, _) => title.to_string(),
        }
    }

    /// Accessories this post needs more of than there are: each pet wearing
    /// one needs its own copy, so a bow worn by both pets needs 2. In the
    /// order the looks list them. Accessories missing from `accessories` are
    /// skipped (the planner already reports those).
    pub fn shortages(&self, accessories: &[Accessory]) -> Vec<Shortage> {
        let mut wearers: Vec<(AccessoryId, Vec<Pet>)> = Vec::new();
        for look in &self.looks {
            match wearers.iter_mut().find(|(id, _)| *id == look.accessory_id) {
                Some((_, pets)) if pets.contains(&look.pet) => {}
                Some((_, pets)) => pets.push(look.pet),
                None => wearers.push((look.accessory_id, vec![look.pet])),
            }
        }
        wearers
            .into_iter()
            .filter_map(|(id, pets)| {
                let acc = accessories.iter().find(|a| a.id == id)?;
                (pets.len() > acc.quantity as usize).then(|| Shortage {
                    accessory_id: id,
                    name: acc.name.clone(),
                    pets,
                    owned: acc.quantity,
                })
            })
            .collect()
    }
}

impl PostPlan {
    /// Pieces added to a chosen outfit since it was chosen, that the pet
    /// isn't wearing yet: (pet, outfit, piece), in the outfit's order. A
    /// piece that was in the outfit already but left out of the look on
    /// purpose is never offered again.
    pub fn outfit_offers(&self, outfits: &[Outfit]) -> Vec<(Pet, OutfitId, AccessoryId)> {
        let mut offers = Vec::new();
        for choice in &self.outfits {
            let Some(outfit) = outfits.iter().find(|o| o.id == choice.outfit_id) else {
                continue;
            };
            for &piece in &outfit.pieces {
                let worn = self
                    .looks
                    .iter()
                    .any(|l| l.pet == choice.pet && l.accessory_id == piece);
                if !worn && !choice.known.contains(&piece) {
                    offers.push((choice.pet, outfit.id, piece));
                }
            }
        }
        offers
    }
}

/// An outfit chosen for one pet in a post.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutfitChoice {
    pub pet: Pet,
    pub outfit_id: OutfitId,
    /// The outfit's pieces when it was chosen, plus pieces offered since
    /// (added or turned down). Anything else in the outfit is new.
    pub known: Vec<AccessoryId>,
}

/// Apply the outfits ticked in the post form (`ticked`) to the looks.
///
/// An outfit that was already chosen keeps what it knew, so pieces left out
/// on purpose stay out. A newly ticked one remembers the outfit's pieces
/// now, and with `add_pieces` they're added to the pet's looks (the form
/// without JavaScript; with it, the page has ticked them already, and the
/// person may have unticked one). Outfits no longer ticked are forgotten;
/// their pieces stay in the looks unless unticked too.
pub fn choose_outfits(
    previous: &[OutfitChoice],
    ticked: &[(Pet, OutfitId)],
    mut looks: Vec<Look>,
    outfits: &[Outfit],
    add_pieces: bool,
) -> (Vec<Look>, Vec<OutfitChoice>) {
    let mut chosen: Vec<OutfitChoice> = Vec::new();
    for &(pet, outfit_id) in ticked {
        if chosen
            .iter()
            .any(|c| c.pet == pet && c.outfit_id == outfit_id)
        {
            continue;
        }
        if let Some(before) = previous
            .iter()
            .find(|c| c.pet == pet && c.outfit_id == outfit_id)
        {
            chosen.push(before.clone());
            continue;
        }
        let Some(outfit) = outfits.iter().find(|o| o.id == outfit_id) else {
            continue;
        };
        if add_pieces {
            for &piece in &outfit.pieces {
                let look = Look {
                    pet,
                    accessory_id: piece,
                };
                if !looks.contains(&look) {
                    looks.push(look);
                }
            }
        }
        chosen.push(OutfitChoice {
            pet,
            outfit_id,
            known: outfit.pieces.clone(),
        });
    }
    (looks, chosen)
}

/// A post needs more copies of an accessory than there are.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shortage {
    pub accessory_id: AccessoryId,
    pub name: String,
    /// Who wears it; one copy each.
    pub pets: Vec<Pet>,
    pub owned: u32,
}

impl Shortage {
    pub fn needed(&self) -> u32 {
        self.pets.len() as u32
    }
}

impl std::fmt::Display for Shortage {
    /// "Mochi and Momo both wear Red bow, but there's only 1."
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let who: Vec<String> = self.pets.iter().map(Pet::to_string).collect();
        write!(
            f,
            "{} both wear {}, but there's only {}.",
            who.join(" and "),
            self.name,
            self.owned
        )
    }
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

/// A planned post: what it's for, when it goes up, and what everyone wears.
///
/// Without a date it's an *idea*: everything can be planned (looks, backdrop)
/// and it's scheduled later, when there's a gap or a fitting day.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PostPlan {
    pub id: PostPlanId,
    /// "Pokémon fossil exhibit". Optional when there's an event (its name is
    /// used instead); posts without an event must have one.
    pub title: String,
    pub event_id: Option<EventId>,
    /// `None` = an idea, not scheduled yet.
    pub post_date: Option<NaiveDate>,
    /// Part of a series: several posts sharing one setup and photo shoot.
    pub series_id: Option<SeriesId>,
    pub looks: Vec<Look>,
    /// Outfits chosen for a pet. Their pieces are already in `looks`; this
    /// remembers the choice, for grouping and for offering pieces added to
    /// the outfit later.
    pub outfits: Vec<OutfitChoice>,
    /// Free-text idea for the seasonal backdrop. In a series, the series'
    /// backdrop is used instead.
    pub backdrop: String,
    pub notes: String,
}

/// Several posts from one setup and one photo shoot, published on different
/// days: "Momo's Earth Day", "Mochi's Earth Day", then both together.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Series {
    pub id: SeriesId,
    pub title: String,
    /// The shared setup.
    pub backdrop: String,
    pub notes: String,
}

/// Pieces worn together, like Glinda's crown, dress and wand. Each piece is
/// still its own accessory; choosing an outfit in a post ticks all of them.
/// A piece can be in several outfits.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Outfit {
    pub id: OutfitId,
    pub name: String,
    pub description: String,
    /// In the order they were chosen.
    pub pieces: Vec<AccessoryId>,
}

impl Outfit {
    /// The pieces themselves, skipping any that aren't in `accessories`.
    pub fn piece_items<'a>(&self, accessories: &'a [Accessory]) -> Vec<&'a Accessory> {
        self.pieces
            .iter()
            .filter_map(|id| accessories.iter().find(|a| a.id == *id))
            .collect()
    }

    /// As far along as its least-finished piece: ready only when every piece
    /// is. An outfit with no pieces yet is just an idea.
    pub fn status(&self, accessories: &[Accessory]) -> AccessoryStatus {
        let pieces = self.piece_items(accessories);
        let has = |status| pieces.iter().any(|a| a.status == status);
        if pieces.is_empty() || has(AccessoryStatus::Idea) {
            AccessoryStatus::Idea
        } else if has(AccessoryStatus::InProgress) {
            AccessoryStatus::InProgress
        } else {
            AccessoryStatus::Ready
        }
    }

    /// (ready, total) pieces, for "2 of 3 ready".
    pub fn ready_count(&self, accessories: &[Accessory]) -> (usize, usize) {
        let pieces = self.piece_items(accessories);
        let ready = pieces
            .iter()
            .filter(|a| a.status == AccessoryStatus::Ready)
            .count();
        (ready, pieces.len())
    }
}

/// What a photo is a picture of.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PhotoOwner {
    Fabric(FabricId),
    Accessory(AccessoryId),
    /// Reference shots, or the finished photo after the shoot.
    PostPlan(PostPlanId),
    /// The whole look together.
    Outfit(OutfitId),
}

/// A photo's record. The image itself is a file; see [`crate::photos`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Photo {
    pub id: PhotoId,
    pub owner: PhotoOwner,
    pub image_type: crate::photos::ImageType,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fabric(id: i64, colors: &[&str]) -> Fabric {
        Fabric {
            id: FabricId(id),
            name: format!("Fabric {id}"),
            description: String::new(),
            colors: colors.iter().map(|&c| c.into()).collect(),
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
            colors: colors.iter().map(|&c| c.into()).collect(),
            colors_from_fabrics: from_fabrics,
            tags: vec![],
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
            quantity: 1,
        }
    }

    #[test]
    fn colors_can_come_from_fabrics_plus_extras() {
        let stash = [
            fabric(1, &["orange"]),
            fabric(2, &["black", "orange"]),
            fabric(3, &["teal"]),
        ];
        let names =
            |colors: Vec<Color>| -> Vec<String> { colors.into_iter().map(|c| c.name).collect() };
        // Fabric colors in order (main first), extras after, no repeats;
        // fabric 3 isn't used by this bow so teal doesn't appear.
        assert_eq!(
            names(bow(&["white", "black"], true).effective_colors(&stash)),
            vec!["orange", "black", "white"]
        );
        // Box unticked: exactly what was typed.
        assert_eq!(
            names(bow(&["white"], false).effective_colors(&stash)),
            vec!["white"]
        );
    }

    #[test]
    fn a_repeated_color_keeps_the_fabrics_swatch() {
        let mut velvet = fabric(1, &[]);
        velvet.colors = vec![Color::with_hex("orange", "#f28c28")];
        // The bow's own "orange" has no swatch; the fabric's comes first and wins.
        let colors = bow(&["orange"], true).effective_colors(&[velvet]);
        assert_eq!(colors, vec![Color::with_hex("orange", "#f28c28")]);
    }

    #[test]
    fn tags_include_the_fabrics_keywords_once() {
        let mut velvet = fabric(1, &[]);
        velvet.keywords = vec!["Batik".into(), "pumpkin".into()];
        let mut satin = fabric(2, &[]);
        satin.keywords = vec!["halloween".into(), "pumpkin".into()];
        let mut bow = bow(&[], true);
        bow.tags = vec!["batik".into(), "japanese".into()];
        // Own tags first; "Batik" and the second "pumpkin" are repeats.
        assert_eq!(
            bow.all_tags(&[velvet, satin]),
            vec!["batik", "japanese", "pumpkin", "halloween"]
        );
    }

    fn post_with(looks: &[(Pet, i64)]) -> PostPlan {
        PostPlan {
            id: PostPlanId(1),
            title: "Test".into(),
            event_id: None,
            post_date: None,
            series_id: None,
            looks: looks
                .iter()
                .map(|&(pet, id)| Look {
                    pet,
                    accessory_id: AccessoryId(id),
                })
                .collect(),
            outfits: vec![],
            backdrop: String::new(),
            notes: String::new(),
        }
    }

    #[test]
    fn choosing_an_outfit_copies_its_pieces_and_offers_new_ones_later() {
        let mut glinda = Outfit {
            id: OutfitId(1),
            name: "Glinda".into(),
            description: String::new(),
            pieces: vec![AccessoryId(1), AccessoryId(2), AccessoryId(3)],
        };
        let ticked = [(Pet::Momo, OutfitId(1))];

        // Without JavaScript the server adds the pieces.
        let (looks, chosen) = choose_outfits(&[], &ticked, vec![], &[glinda.clone()], true);
        assert_eq!(looks.len(), 3);
        assert_eq!(chosen[0].known, glinda.pieces);

        // With it the page ticked them, and the wand (3) was left out.
        let worn = |ids: &[i64]| -> Vec<Look> {
            ids.iter()
                .map(|&id| Look {
                    pet: Pet::Momo,
                    accessory_id: AccessoryId(id),
                })
                .collect()
        };
        let (looks, chosen) = choose_outfits(&[], &ticked, worn(&[1, 2]), &[glinda.clone()], false);
        let mut post = PostPlan {
            looks,
            outfits: chosen,
            ..post_with(&[])
        };
        assert!(
            post.outfit_offers(&[glinda.clone()]).is_empty(),
            "left out on purpose"
        );

        // A cape (4) is added to the outfit: offered once, for Momo only.
        glinda.pieces.push(AccessoryId(4));
        assert_eq!(
            post.outfit_offers(&[glinda.clone()]),
            [(Pet::Momo, OutfitId(1), AccessoryId(4))]
        );
        // Saving the form again keeps what the choice knew, so it's still offered.
        let (looks, chosen) = choose_outfits(
            &post.outfits,
            &ticked,
            post.looks.clone(),
            &[glinda.clone()],
            true,
        );
        assert_eq!(looks.len(), 2, "the wand isn't put back");
        post.outfits = chosen;
        assert_eq!(post.outfit_offers(&[glinda.clone()]).len(), 1);
        // Turned down: remembered, not offered again.
        post.outfits[0].known.push(AccessoryId(4));
        assert!(post.outfit_offers(&[glinda.clone()]).is_empty());

        // Unticking the outfit forgets it but keeps the pieces.
        let (looks, chosen) =
            choose_outfits(&post.outfits, &[], post.looks.clone(), &[glinda], true);
        assert_eq!(looks.len(), 2);
        assert!(chosen.is_empty());
    }

    #[test]
    fn both_pets_in_one_bow_need_two() {
        let one = bow(&[], false);
        let mut two = bow(&[], false);
        two.id = AccessoryId(2);
        two.name = "Twin bow".into();
        two.quantity = 2;
        let stock = [one, two];

        // Bow 1 is worn by both but there's only one; bow 2 has a spare.
        let post = post_with(&[
            (Pet::Mochi, 1),
            (Pet::Momo, 1),
            (Pet::Mochi, 2),
            (Pet::Momo, 2),
        ]);
        let short = post.shortages(&stock);
        assert_eq!(short.len(), 1);
        assert_eq!(short[0].accessory_id, AccessoryId(1));
        assert_eq!(short[0].needed(), 2);
        assert_eq!(
            short[0].to_string(),
            "Mochi and Momo both wear Pumpkin bow, but there's only 1."
        );

        // One pet each, a repeated look, or an unknown accessory: all fine.
        assert!(
            post_with(&[(Pet::Mochi, 1), (Pet::Momo, 2)])
                .shortages(&stock)
                .is_empty()
        );
        assert!(
            post_with(&[(Pet::Mochi, 1), (Pet::Mochi, 1)])
                .shortages(&stock)
                .is_empty()
        );
        assert!(
            post_with(&[(Pet::Mochi, 9), (Pet::Momo, 9)])
                .shortages(&stock)
                .is_empty()
        );
    }

    #[test]
    fn an_outfit_is_ready_when_every_piece_is() {
        let piece = |id, status| Accessory {
            id: AccessoryId(id),
            status,
            ..bow(&[], false)
        };
        let outfit = |pieces: &[i64]| Outfit {
            id: OutfitId(1),
            name: "Glinda".into(),
            description: String::new(),
            pieces: pieces.iter().map(|&id| AccessoryId(id)).collect(),
        };
        let stock = [
            piece(1, AccessoryStatus::Ready),
            piece(2, AccessoryStatus::InProgress),
            piece(3, AccessoryStatus::Idea),
            piece(4, AccessoryStatus::Ready),
        ];
        use AccessoryStatus::*;
        assert_eq!(outfit(&[1, 4]).status(&stock), Ready);
        assert_eq!(outfit(&[1, 2]).status(&stock), InProgress);
        assert_eq!(
            outfit(&[1, 2, 3]).status(&stock),
            Idea,
            "least finished wins"
        );
        assert_eq!(outfit(&[]).status(&stock), Idea, "nothing in it yet");
        assert_eq!(outfit(&[1, 2, 4]).ready_count(&stock), (2, 3));
        // A piece that's gone doesn't count either way.
        assert_eq!(outfit(&[1, 99]).ready_count(&stock), (1, 1));
        assert_eq!(outfit(&[1, 99]).status(&stock), Ready);
    }

    #[test]
    fn hex_values_are_checked_and_lowercased() {
        assert_eq!(Color::parse_hex("#F4C2C2"), Some("#f4c2c2".into()));
        assert_eq!(Color::parse_hex(" #a1b2c3 "), Some("#a1b2c3".into()));
        assert_eq!(Color::parse_hex("f4c2c2"), None, "needs the #");
        assert_eq!(
            Color::parse_hex("#fff"),
            None,
            "short form isn't sent by color inputs"
        );
        assert_eq!(Color::parse_hex("#zzzzzz"), None);
        assert_eq!(Color::parse_hex("red; background: url(x)"), None);
    }
}
