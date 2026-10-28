//! Browsing the fabric and accessory lists: a table or a grid of photo cards,
//! a warm / cool filter, and sorting by color. Shared by both list pages.
//!
//! Everything comes from the page address (`?tone=warm&sort=color`), so a
//! filtered list can be bookmarked or reloaded. The one exception is the
//! table/grid choice, which is also remembered in a cookie: it's a standing
//! preference, not part of what you're looking for.

use std::collections::HashMap;

use axum::http::header::{COOKIE, SET_COOKIE};
use axum::http::{HeaderMap, HeaderValue};

use super::forms::{Choice, parse_choice};
use crate::models::Color;
use crate::palette::{self, Tone};

/// The cookie that remembers table or grid.
const VIEW_COOKIE: &str = "list_view";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    Table,
    Grid,
}

const VIEWS: [(View, &str, &str); 2] = [
    (View::Table, "table", "Table"),
    (View::Grid, "grid", "Grid"),
];

/// The tone of a whole item, from the tones of its colors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemTone {
    Warm,
    Cool,
    /// Both warm and cool colors (a pumpkin with green leaves).
    Mixed,
    /// Only neutrals: white, cream, greys, black.
    Neutral,
}

pub const TONES: [(ItemTone, &str, &str); 4] = [
    (ItemTone::Warm, "warm", "Warm"),
    (ItemTone::Cool, "cool", "Cool"),
    (ItemTone::Mixed, "mixed", "Warm + cool"),
    (ItemTone::Neutral, "neutral", "Neutral"),
];

/// The shade of a color: its picked swatch, or failing that the typical
/// shade of its name if it's one of [`palette::NAMES`] ("navy" → a navy).
/// `None` for a name the app doesn't know, like "sparkly".
fn shade(color: &Color) -> Option<[u8; 3]> {
    if let Some(rgb) = color.hex.as_deref().and_then(palette::rgb_from_hex) {
        return Some(rgb);
    }
    let name = color.name.trim().to_lowercase();
    palette::NAMES
        .iter()
        .find(|(known, _)| *known == name)
        .map(|(_, rgb)| *rgb)
}

/// Warm, cool, both, or neutral. A tone wins when it has more than twice as
/// many colors as the other: a red-and-orange bow with a navy accent is warm,
/// a red-and-green one is "warm + cool". Neutrals don't count either way (an
/// orange bow with a black middle is still warm). `None` when none of the
/// colors has a known shade (or there are no colors at all).
pub fn item_tone(colors: &[Color]) -> Option<ItemTone> {
    let tones: Vec<Tone> = colors.iter().filter_map(shade).map(palette::tone).collect();
    let count = |tone| tones.iter().filter(|t| **t == tone).count();
    let (warm, cool) = (count(Tone::Warm), count(Tone::Cool));
    if tones.is_empty() {
        None
    } else if warm == 0 && cool == 0 {
        Some(ItemTone::Neutral)
    } else if warm > 2 * cool {
        Some(ItemTone::Warm)
    } else if cool > 2 * warm {
        Some(ItemTone::Cool)
    } else {
        Some(ItemTone::Mixed)
    }
}

/// Where an item goes when sorted by color: rainbow order of its first
/// known shade (the main fabric's, since colors are listed main first);
/// items with no known shade go last.
fn color_key(colors: &[Color]) -> (bool, (bool, u16, u16)) {
    match colors.iter().find_map(shade) {
        Some(rgb) => (false, palette::rainbow_order(rgb)),
        None => (true, (true, 0, 0)),
    }
}

/// What the person asked to see, read from the address and the cookie.
#[derive(Debug, Clone, PartialEq)]
pub struct Browse {
    pub view: View,
    pub tone: Option<ItemTone>,
    pub by_color: bool,
    /// Set when the address chose a view, so the cookie gets updated.
    view_chosen: bool,
}

impl Browse {
    pub fn read(query: &HashMap<String, String>, headers: &HeaderMap) -> Self {
        let chosen = query.get("view").and_then(|v| parse_choice(&VIEWS, v));
        let remembered = headers
            .get_all(COOKIE)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .flat_map(|cookies| cookies.split(';'))
            .find_map(|cookie| cookie.trim().strip_prefix(VIEW_COOKIE)?.strip_prefix('='))
            .and_then(|v| parse_choice(&VIEWS, v));
        Self {
            view: chosen.or(remembered).unwrap_or(View::Table),
            tone: query.get("tone").and_then(|v| parse_choice(&TONES, v)),
            by_color: query.get("sort").is_some_and(|v| v == "color"),
            view_chosen: chosen.is_some(),
        }
    }

    pub fn is_grid(&self) -> bool {
        self.view == View::Grid
    }

    /// Does an item with these colors pass the tone filter?
    pub fn keeps(&self, colors: &[Color]) -> bool {
        self.tone
            .is_none_or(|wanted| item_tone(colors) == Some(wanted))
    }

    /// Sort by color if asked; otherwise leave the order alone (the database
    /// already lists things by name).
    pub fn sort<T>(&self, items: &mut [T], colors: impl Fn(&T) -> &[Color]) {
        if self.by_color {
            // A stable sort: items with the same main shade stay by name.
            items.sort_by_key(|item| color_key(colors(item)));
        }
    }

    /// The tone and sort as address parameters, for links back to this page.
    pub fn params(&self) -> Vec<String> {
        let mut params = Vec::new();
        if let Some(tone) = self.tone {
            let value = TONES.iter().find(|(t, _, _)| *t == tone).unwrap().1;
            params.push(format!("tone={value}"));
        }
        if self.by_color {
            params.push("sort=color".into());
        }
        params
    }

    /// The filter bar for the page at `path`. `filters` are the page's own
    /// dropdowns (shown first) and `params` their current values, so the
    /// Table | Grid links keep them.
    pub fn controls(&self, path: &str, mut filters: Vec<Filter>, params: &[String]) -> Controls {
        let mut tones = vec![Choice::new("", "Any tone", self.tone.is_none())];
        for (tone, value, label) in TONES {
            tones.push(Choice::new(value, label, self.tone == Some(tone)));
        }
        filters.push(Filter {
            name: "tone",
            label: "Tone",
            choices: tones,
        });
        filters.push(Filter {
            name: "sort",
            label: "Sort",
            choices: vec![
                Choice::new("", "By name", !self.by_color),
                Choice::new("color", "By color", self.by_color),
            ],
        });

        let mut kept = params.to_vec();
        kept.extend(self.params());
        let view_links = VIEWS
            .iter()
            .map(|(view, value, label)| {
                let mut all = kept.clone();
                all.push(format!("view={value}"));
                ViewLink {
                    href: format!("{path}?{}", all.join("&")),
                    label,
                    current: *view == self.view,
                }
            })
            .collect();
        Controls {
            path: path.to_string(),
            filters,
            view: VIEWS.iter().find(|(v, _, _)| *v == self.view).unwrap().1,
            view_links,
        }
    }

    /// A `Set-Cookie` header remembering the view, when the address chose
    /// one. Lasts a year; `SameSite=Lax` so other sites can't set it.
    pub fn cookie(&self) -> Option<(axum::http::HeaderName, HeaderValue)> {
        if !self.view_chosen {
            return None;
        }
        let value = VIEWS.iter().find(|(v, _, _)| *v == self.view).unwrap().1;
        let cookie = format!("{VIEW_COOKIE}={value}; Path=/; Max-Age=31536000; SameSite=Lax");
        Some((SET_COOKIE, HeaderValue::from_str(&cookie).ok()?))
    }
}

/// One dropdown in the filter bar.
pub struct Filter {
    /// The address parameter, like `tone`.
    pub name: &'static str,
    pub label: &'static str,
    pub choices: Vec<Choice>,
}

/// The filter bar above a list (`templates/browse_controls.html`).
pub struct Controls {
    /// The list page the filter form submits to.
    pub path: String,
    pub filters: Vec<Filter>,
    /// The current view, kept when the filters are submitted.
    pub view: &'static str,
    pub view_links: Vec<ViewLink>,
}

pub struct ViewLink {
    pub href: String,
    pub label: &'static str,
    pub current: bool,
}

/// Join address parameters into `?a=1&b=2` (or nothing).
pub fn query_string(params: &[String]) -> String {
    if params.is_empty() {
        String::new()
    } else {
        format!("?{}", params.join("&"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn colors(list: &[&str]) -> Vec<Color> {
        list.iter()
            .map(|c| match c.strip_prefix('#') {
                Some(_) => Color::with_hex("picked", *c),
                None => (*c).into(),
            })
            .collect()
    }

    #[test]
    fn an_item_is_warm_cool_mixed_or_neutral() {
        assert_eq!(
            item_tone(&colors(&["#f5821e", "black"])),
            Some(ItemTone::Warm)
        );
        assert_eq!(item_tone(&colors(&["navy", "white"])), Some(ItemTone::Cool));
        assert_eq!(
            item_tone(&colors(&["orange", "#32a046"])),
            Some(ItemTone::Mixed)
        );
        assert_eq!(
            item_tone(&colors(&["cream", "Black"])),
            Some(ItemTone::Neutral)
        );
        // Unknown names and no colors at all: no tone, so no tone filter shows them.
        assert_eq!(item_tone(&colors(&["sparkly"])), None);
        assert_eq!(item_tone(&[]), None);
    }

    #[test]
    fn a_picked_shade_beats_the_name() {
        // Named "red" but picked as a blue: the shade is what you see.
        assert_eq!(
            item_tone(&[Color::with_hex("red", "#2864c8")]),
            Some(ItemTone::Cool)
        );
    }

    #[test]
    fn sorting_by_color_follows_the_main_color() {
        let browse = Browse {
            by_color: true,
            ..Browse::read(&HashMap::new(), &HeaderMap::new())
        };
        let mut items = vec![
            ("Blue bow", colors(&["blue", "red"])),
            ("Mystery", colors(&["sparkly"])),
            ("Grey bow", colors(&["grey"])),
            ("Red bow", colors(&["red", "blue"])),
            ("Yellow bow", colors(&["yellow"])),
        ];
        browse.sort(&mut items, |(_, c)| c);
        let names: Vec<_> = items.iter().map(|(n, _)| *n).collect();
        assert_eq!(
            names,
            ["Red bow", "Yellow bow", "Blue bow", "Grey bow", "Mystery"]
        );
    }

    #[test]
    fn the_view_comes_from_the_address_then_the_cookie() {
        let query = |pairs: &[(&str, &str)]| -> HashMap<String, String> {
            pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect()
        };
        let mut cookie = HeaderMap::new();
        cookie.insert(COOKIE, HeaderValue::from_static("theme=x; list_view=grid"));

        let default = Browse::read(&query(&[]), &HeaderMap::new());
        assert_eq!(default.view, View::Table);
        assert!(
            default.cookie().is_none(),
            "nothing chosen, nothing to remember"
        );

        let remembered = Browse::read(&query(&[]), &cookie);
        assert!(remembered.is_grid());
        assert!(remembered.cookie().is_none());

        let chosen = Browse::read(&query(&[("view", "table"), ("tone", "cool")]), &cookie);
        assert_eq!(chosen.view, View::Table, "the address wins over the cookie");
        let (_, value) = chosen.cookie().unwrap();
        assert!(value.to_str().unwrap().starts_with("list_view=table;"));
        assert_eq!(chosen.params(), ["tone=cool"]);
    }
}
