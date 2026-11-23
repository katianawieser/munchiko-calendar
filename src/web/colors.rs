//! Color helpers for the fabric and accessory forms:
//! - [`name_for_hex`]: the page asks what to call a shade that was just picked,
//!   so naming uses the same code (and list of names) as the photo colors;
//! - [`palette_options`]: the palette, for the forms' name boxes and chips;
//! - [`name_ideas`]: names for a newly picked shade, palette-aware.

use std::collections::HashMap;

use askama::Template;
use axum::extract::{Query, State};
use axum::http::header;
use axum::response::{IntoResponse, Response};

use super::forms::read_colors;
use super::photos::{self, FormWithPhotos, IdeaGroup};
use super::{AppError, AppState, render};
use crate::models::PaletteColor;
use crate::palette;
use crate::shades::{self, ShadeMatch};

/// Most names to offer for one shade.
const MAX_NAMES: usize = 5;

/// `GET /colors/name?hex=%23f4c2c2` → `baby pink` (plain text). With
/// `&count=3`, the best name and alternatives, one per line.
pub async fn name_for_hex(
    State(state): State<AppState>,
    Query(query): Query<HashMap<String, String>>,
) -> Result<impl IntoResponse, AppError> {
    let rgb = query
        .get("hex")
        .and_then(|hex| palette::rgb_from_hex(&hex.to_ascii_lowercase()))
        .ok_or(AppError::BadRequest)?;
    let count = match query.get("count") {
        None => 1,
        Some(n) => n
            .parse::<usize>()
            .ok()
            .filter(|n| (1..=MAX_NAMES).contains(n))
            .ok_or(AppError::BadRequest)?,
    };
    let palette = state.db.list_palette().await?;
    Ok((
        [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
        name_ideas(rgb, &palette, count).join("\n"),
    ))
}

/// The chips for photos chosen in a form but not saved yet: a piece of HTML
/// that colors.js puts on the page as it is.
#[derive(Template)]
#[template(path = "photo_color_ideas.html")]
struct ChosenPhotoIdeas {
    ideas: Vec<IdeaGroup>,
}

/// `POST /colors/from-photos`: the main colors of the photos just chosen in a
/// fabric or accessory form, as color chips, before anything is saved. The
/// form's current colors come along (`color_name` / `color_hex`), so colors
/// it already has a swatch for aren't offered again. Same rules as for saved
/// photos ([`photos::color_ideas`]); the page only shows the result.
pub async fn from_photos(
    State(state): State<AppState>,
    FormWithPhotos { form, photos }: FormWithPhotos,
) -> Result<Response, AppError> {
    // Colors the form can't read yet just aren't used for skipping.
    let existing = read_colors(&form).unwrap_or_default();
    let ideas = photos::ideas_for_uploads(&state, photos, &existing).await;
    Ok(render(ChosenPhotoIdeas { ideas })?.into_response())
}

/// A palette color offered on a form: in the name boxes' suggestions and as
/// a "Pick from your palette" chip. `hex` is empty for a name-only color.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaletteOption {
    pub name: String,
    pub hex: String,
}

/// The whole palette for a form, in rainbow order (name-only colors last).
pub fn palette_options(palette: &[PaletteColor]) -> Vec<PaletteOption> {
    let mut colors = palette.to_vec();
    super::palette::rainbow(&mut colors);
    colors
        .into_iter()
        .map(|c| PaletteOption {
            name: c.name,
            hex: c.hex.unwrap_or_default(),
        })
        .collect()
}

/// Names to offer for a newly picked shade, best first (up to `count`): a
/// palette color that's practically the same shade (it *is* that color),
/// then the closest built-in names the palette doesn't use yet. A built-in
/// name the palette already has would mean that palette color, whose shade
/// is different, so it's skipped.
pub fn name_ideas(rgb: [u8; 3], palette: &[PaletteColor], count: usize) -> Vec<String> {
    let mut ideas: Vec<String> = Vec::new();
    let stash = shades::from_palette(palette);
    if let ShadeMatch::Same(shade) = shades::match_shade(rgb, &stash) {
        ideas.push(shade.name().to_string());
    }
    let taken = |name: &str| palette.iter().any(|c| c.name.eq_ignore_ascii_case(name));
    for name in palette::nearest_names(rgb, palette::NAMES.len()) {
        if ideas.len() >= count {
            break;
        }
        if !taken(name) {
            ideas.push(name.to_string());
        }
    }
    ideas.truncate(count);
    ideas
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::PaletteColorId;

    fn color(id: i64, name: &str, hex: Option<&str>) -> PaletteColor {
        PaletteColor {
            id: PaletteColorId(id),
            name: name.into(),
            hex: hex.map(String::from),
        }
    }

    #[test]
    fn the_palette_is_offered_in_rainbow_order() {
        let palette = [
            color(1, "sparkly", None),
            color(2, "baby blue", Some("#afd2f0")),
            color(3, "orange", Some("#f5821e")),
        ];
        let names: Vec<(String, String)> = palette_options(&palette)
            .into_iter()
            .map(|o| (o.name, o.hex))
            .collect();
        assert_eq!(
            names,
            [
                ("orange".to_string(), "#f5821e".to_string()),
                ("baby blue".to_string(), "#afd2f0".to_string()),
                ("sparkly".to_string(), String::new()),
            ]
        );
    }

    #[test]
    fn a_picked_shade_is_named_after_the_palette_color_it_is() {
        let palette = [
            color(1, "my orange", Some("#f5821e")),
            color(2, "orange", Some("#cc5500")),
        ];
        // Practically the same as "my orange": that's its name.
        let ideas = name_ideas([0xf5, 0x82, 0x1f], &palette, 3);
        assert_eq!(ideas[0], "my orange");
        // Built-in names the palette already uses (for another shade) are skipped.
        assert!(!ideas.contains(&"orange".to_string()), "{ideas:?}");
        assert_eq!(ideas.len(), 3);
        // Nothing like it in the palette: built-in names only.
        let ideas = name_ideas([0x01, 0x15, 0x3e], &palette, 2);
        assert_eq!(ideas[0], "navy");
    }
}
