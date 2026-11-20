//! The stash's shades: every picked swatch on fabrics and accessories, grouped
//! so near-duplicates can be seen together and merged into one (M13c).
//!
//! Picking a shade with the color picker almost never lands on exactly the
//! same value twice, so over time one "black" becomes fifteen slightly
//! different blacks. This module finds those groups two ways:
//! - [`look_alike_groups`]: shades that look alike, whatever they're called;
//! - [`name_groups`]: every shade used under one name ("black": 15 shades).
//!
//! and works out what a merge would change ([`MergePlan`]), without touching
//! a database: the caller saves the result. Like `planner` and `suggest`, it's
//! pure, so it's easy to test.

use std::collections::BTreeMap;

use crate::models::{Accessory, AccessoryId, Color, Fabric, FabricId};
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

/// Shades shown together, most used first.
#[derive(Debug, Clone, PartialEq)]
pub struct ShadeGroup {
    /// Set for a "same name" group: only uses under this name count, and a
    /// merge only changes those.
    pub name: Option<String>,
    pub shades: Vec<Shade>,
}

impl ShadeGroup {
    /// The most used shade: the one to keep unless chosen otherwise.
    pub fn leader(&self) -> &Shade {
        &self.shades[0]
    }

    /// How many colors use a shade in this group.
    pub fn use_count(&self) -> usize {
        self.shades.iter().map(|s| s.uses.len()).sum()
    }
}

/// How close two shades must be to count as look-alikes.
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

/// Shades that look alike, grouped. Going from the most used shade down,
/// each shade joins the group whose leader (its most used shade) is closest,
/// if that's within `closeness`; otherwise it starts a group of its own. So
/// every shade in a group looks like the leader, and the leader is the
/// shade people have used most. `shades` must be in [`collect`]'s order.
/// Groups come back in rainbow order of their leaders.
pub fn look_alike_groups(shades: &[Shade], closeness: Closeness) -> Vec<ShadeGroup> {
    let mut groups: Vec<ShadeGroup> = Vec::new();
    for shade in shades {
        let nearest = groups
            .iter_mut()
            .map(|g| (palette::difference(g.leader().rgb, shade.rgb), g))
            .filter(|(d, _)| *d < closeness.limit())
            .min_by(|a, b| a.0.total_cmp(&b.0));
        match nearest {
            Some((_, group)) => group.shades.push(shade.clone()),
            None => groups.push(ShadeGroup {
                name: None,
                shades: vec![shade.clone()],
            }),
        }
    }
    groups.sort_by_key(|g| palette::rainbow_order(g.leader().rgb));
    groups
}

/// Every name used with more than one shade, with those shades (only their
/// uses under that name), most used first. Names in alphabetical order.
pub fn name_groups(shades: &[Shade]) -> Vec<ShadeGroup> {
    let mut by_name: BTreeMap<String, Vec<Shade>> = BTreeMap::new();
    for shade in shades {
        for name in &shade.names {
            let uses: Vec<(Owner, String)> = shade
                .uses
                .iter()
                .filter(|(_, n)| n == name)
                .cloned()
                .collect();
            by_name.entry(name.clone()).or_default().push(Shade {
                names: vec![name.clone()],
                uses,
                ..shade.clone()
            });
        }
    }
    by_name
        .into_iter()
        .filter(|(_, shades)| shades.len() > 1)
        .map(|(name, mut shades)| {
            shades.sort_by_key(|s| std::cmp::Reverse(s.uses.len()));
            ShadeGroup {
                name: Some(name),
                shades,
            }
        })
        .collect()
}

/// A merge: every color using one of `from` (and, for a "same name" merge,
/// called `only_name`) becomes `to`: the kept shade, under its name.
#[derive(Debug, Clone, PartialEq)]
pub struct MergePlan {
    pub from: Vec<String>,
    pub only_name: Option<String>,
    pub to: Color,
}

impl MergePlan {
    /// `colors` after the merge, or `None` if nothing in it changes. If the
    /// merge leaves two colors with the same name, the first one stays.
    pub fn apply(&self, colors: &[Color]) -> Option<Vec<Color>> {
        let mut changed = false;
        let mut merged: Vec<Color> = Vec::new();
        for color in colors {
            let hit = color.hex.as_ref().is_some_and(|h| self.from.contains(h))
                && self.only_name.as_ref().is_none_or(|n| *n == color.name);
            let color = if hit {
                changed |= *color != self.to;
                self.to.clone()
            } else {
                color.clone()
            };
            if merged.iter().any(|c| c.name == color.name) {
                changed = true; // a repeat left by the merge
            } else {
                merged.push(color);
            }
        }
        changed.then_some(merged)
    }

    /// Every fabric and accessory whose colors change, with their new colors.
    pub fn changes(
        &self,
        fabrics: &[Fabric],
        accessories: &[Accessory],
    ) -> Vec<(Owner, Vec<Color>)> {
        let lists = fabrics
            .iter()
            .map(|f| (Owner::Fabric(f.id), &f.colors))
            .chain(
                accessories
                    .iter()
                    .map(|a| (Owner::Accessory(a.id), &a.colors)),
            );
        lists
            .filter_map(|(owner, colors)| self.apply(colors).map(|new| (owner, new)))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn look_alikes_group_around_the_most_used_shade() {
        let shades = collect(&stash(), &[]);
        let groups = look_alike_groups(&shades, Closeness::Medium);
        let blacks = groups.iter().find(|g| g.leader().hex == "#101010").unwrap();
        let mut hexes: Vec<&str> = blacks.shades.iter().map(|s| s.hex.as_str()).collect();
        assert_eq!(hexes[0], "#101010", "the most used leads");
        hexes.sort();
        // Both near-blacks join, whatever they're called; #333333 is too far.
        assert_eq!(hexes, ["#101010", "#121212", "#141414"]);
        assert_eq!(blacks.use_count(), 4);
        // Tight keeps more apart than loose.
        assert!(
            look_alike_groups(&shades, Closeness::Tight).len()
                >= look_alike_groups(&shades, Closeness::Loose).len()
        );
    }

    #[test]
    fn same_name_groups_only_count_that_name() {
        let groups = name_groups(&collect(&stash(), &[]));
        let names: Vec<&str> = groups.iter().map(|g| g.name.as_deref().unwrap()).collect();
        assert_eq!(
            names,
            ["black", "charcoal"],
            "red has one shade: not listed"
        );
        let charcoal = &groups[1];
        assert_eq!(charcoal.shades.len(), 2);
        assert!(charcoal.shades.iter().all(|s| s.uses.len() == 1));
    }

    #[test]
    fn a_merge_renames_and_reshades_and_drops_repeats() {
        let plan = MergePlan {
            from: vec!["#121212".into(), "#141414".into()],
            only_name: None,
            to: c("black", "#101010"),
        };
        let changes = plan.changes(&stash(), &[]);
        let owners: Vec<Owner> = changes.iter().map(|(o, _)| *o).collect();
        assert_eq!(
            owners,
            [Owner::Fabric(FabricId(3)), Owner::Fabric(FabricId(4))]
        );
        // Fabric 4's near-black "charcoal" becomes black (the kept name).
        assert_eq!(
            changes[1].1,
            [c("black", "#101010"), Color::from("sparkly")]
        );

        // Two colors that end up with one name: the first stays.
        let both = [
            c("black", "#141414"),
            c("black", "#101010"),
            c("red", "#cc0000"),
        ];
        assert_eq!(
            plan.apply(&both).unwrap(),
            [c("black", "#101010"), c("red", "#cc0000")]
        );
        // Nothing to do: None.
        assert_eq!(plan.apply(&[c("red", "#cc0000")]), None);
    }

    #[test]
    fn a_same_name_merge_leaves_other_names_alone() {
        let plan = MergePlan {
            from: vec!["#121212".into(), "#141414".into()],
            only_name: Some("black".into()),
            to: c("black", "#101010"),
        };
        let changes = plan.changes(&stash(), &[]);
        // Fabric 4's "charcoal" #121212 isn't called black: untouched.
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].0, Owner::Fabric(FabricId(3)));
    }
}
