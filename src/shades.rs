//! The stash's shades: every picked swatch on fabrics and accessories, grouped
//! so near-duplicates can be seen together and merged into one (M13c).
//!
//! Picking a shade with the color picker almost never lands on exactly the
//! same value twice, so over time one "black" becomes fifteen slightly
//! different blacks. Since the palette (M15a) every item uses palette
//! colors, so this module now only:
//! - collects the shades in use ([`collect`]) or in the palette
//!   ([`from_palette`]);
//! - matches a color found in a photo to one of them ([`match_shade`]).
//!
//! Like `planner` and `suggest`, it's pure, so it's easy to test.

use std::collections::BTreeMap;

use crate::models::{Accessory, AccessoryId, Fabric, FabricId};
use crate::palette;

/// Whose color list a color is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Owner {
    Fabric(FabricId),
    Accessory(AccessoryId),
}

/// One picked shade (`#rrggbb`), and everywhere it's used.
#[derive(Debug, Clone, PartialEq)]
pub struct Shade {
    pub hex: String,
    pub rgb: [u8; 3],
    /// The names it's used under, most used first.
    pub names: Vec<String>,
    /// Each (owner, name) it appears as.
    pub uses: Vec<(Owner, String)>,
}

impl Shade {
    /// The name it's used under most.
    pub fn name(&self) -> &str {
        self.names.first().map_or("", String::as_str)
    }
}

/// How close two shades must be to count as the same or as close.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Closeness {
    /// ΔE under 5: only differences that are barely visible.
    Tight,
    /// ΔE under 10: the default.
    Medium,
    /// ΔE under 15: clearly related shades.
    Loose,
}

impl Closeness {
    pub fn limit(self) -> f32 {
        match self {
            Closeness::Tight => 5.0,
            Closeness::Medium => 10.0,
            Closeness::Loose => 15.0,
        }
    }
}

/// Every picked shade on fabrics and accessories (their own colors; an
/// accessory's colors from its fabrics are the fabrics' shades), most used
/// first, then in rainbow order. Colors without a swatch have no shade and
/// aren't included.
pub fn collect(fabrics: &[Fabric], accessories: &[Accessory]) -> Vec<Shade> {
    let lists = fabrics
        .iter()
        .map(|f| (Owner::Fabric(f.id), &f.colors))
        .chain(
            accessories
                .iter()
                .map(|a| (Owner::Accessory(a.id), &a.colors)),
        );
    let mut by_hex: BTreeMap<String, Shade> = BTreeMap::new();
    for (owner, colors) in lists {
        for color in colors {
            let Some(hex) = &color.hex else { continue };
            let Some(rgb) = palette::rgb_from_hex(hex) else {
                continue;
            };
            let shade = by_hex.entry(hex.clone()).or_insert_with(|| Shade {
                hex: hex.clone(),
                rgb,
                names: Vec::new(),
                uses: Vec::new(),
            });
            shade.uses.push((owner, color.name.clone()));
        }
    }
    let mut shades: Vec<Shade> = by_hex
        .into_values()
        .map(|mut shade| {
            shade.names = most_used_names(&shade.uses);
            shade
        })
        .collect();
    shades.sort_by(|a, b| {
        b.uses
            .len()
            .cmp(&a.uses.len())
            .then(palette::rainbow_order(a.rgb).cmp(&palette::rainbow_order(b.rgb)))
    });
    shades
}

/// The names in `uses`, most used first (ties alphabetical).
fn most_used_names(uses: &[(Owner, String)]) -> Vec<String> {
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for (_, name) in uses {
        *counts.entry(name).or_default() += 1;
    }
    let mut names: Vec<(&str, usize)> = counts.into_iter().collect();
    names.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    names.into_iter().map(|(n, _)| n.to_string()).collect()
}

/// The palette's shaded colors as shades (with no uses), for matching a
/// photo's colors against the whole palette.
pub fn from_palette(palette: &[crate::models::PaletteColor]) -> Vec<Shade> {
    palette
        .iter()
        .filter_map(|c| {
            let hex = c.hex.clone()?;
            let rgb = palette::rgb_from_hex(&hex)?;
            Some(Shade {
                hex,
                rgb,
                names: vec![c.name.clone()],
                uses: Vec::new(),
            })
        })
        .collect()
}

/// How a newly found color (from a photo) compares with the stash's shades.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ShadeMatch<'a> {
    /// Practically identical (under [`Closeness::Tight`]): use the stash's.
    Same(&'a Shade),
    /// Close (under [`Closeness::Medium`]) but visibly different: offer both.
    Close(&'a Shade),
    /// Nothing like it yet: a new shade.
    New,
}

/// The stash shade most like `rgb`, and how close it is. Reusing a shade
/// that's already there keeps the stash from filling up with near-copies.
pub fn match_shade(rgb: [u8; 3], stash: &[Shade]) -> ShadeMatch<'_> {
    let nearest = stash
        .iter()
        .map(|s| (palette::difference(s.rgb, rgb), s))
        .min_by(|a, b| a.0.total_cmp(&b.0));
    match nearest {
        Some((d, shade)) if d < Closeness::Tight.limit() => ShadeMatch::Same(shade),
        Some((d, shade)) if d < Closeness::Medium.limit() => ShadeMatch::Close(shade),
        _ => ShadeMatch::New,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Color;

    fn fabric(id: i64, colors: Vec<Color>) -> Fabric {
        Fabric {
            id: FabricId(id),
            name: format!("Fabric {id}"),
            description: String::new(),
            colors,
            keywords: vec![],
            pairings: vec![],
            in_stock: true,
        }
    }

    fn c(name: &str, hex: &str) -> Color {
        Color::with_hex(name, hex)
    }

    /// Three blacks a picker might give, a charcoal, and a red.
    fn stash() -> Vec<Fabric> {
        vec![
            fabric(1, vec![c("black", "#101010"), c("red", "#cc0000")]),
            fabric(2, vec![c("black", "#101010")]),
            fabric(3, vec![c("black", "#141414"), c("charcoal", "#333333")]),
            fabric(4, vec![c("charcoal", "#121212"), Color::from("sparkly")]),
        ]
    }

    #[test]
    fn shades_are_collected_once_with_their_uses() {
        let shades = collect(&stash(), &[]);
        assert_eq!(shades.len(), 5, "colors without a swatch have no shade");
        assert_eq!(shades[0].hex, "#101010", "most used first");
        assert_eq!(shades[0].uses.len(), 2);
        assert_eq!(shades[0].name(), "black");
    }

    #[test]
    fn found_colors_reuse_a_matching_shade() {
        let stash = collect(&[fabric(1, vec![c("sage", "#a3a264")])], &[]);
        let sage = &stash[0];
        // Practically the same: reuse it.
        assert_eq!(
            match_shade([0xa4, 0xa3, 0x65], &stash),
            ShadeMatch::Same(sage)
        );
        // Visibly different but close: both are offered.
        assert_eq!(
            match_shade([0xb0, 0xaa, 0x60], &stash),
            ShadeMatch::Close(sage)
        );
        // Nothing like it.
        assert_eq!(match_shade([0, 0, 200], &stash), ShadeMatch::New);
        assert_eq!(match_shade([0, 0, 200], &[]), ShadeMatch::New);
    }
}
