//! Color helpers for the fabric and accessory forms:
//! - [`name_for_hex`]: the page asks what to call a shade that was just picked,
//!   so naming uses the same code (and list of names) as the photo colors;
//! - [`your_shades`]: every shade already picked somewhere in the stash, to
//!   reuse with one click instead of picking it again.

use std::collections::HashMap;

use askama::Template;
use axum::extract::{Query, State};
use axum::http::header;
use axum::response::{IntoResponse, Response};

use super::forms::read_colors;
use super::photos::{self, ColorIdea, FormWithPhotos};
use super::{AppError, AppState, render};
use crate::models::{Accessory, Color, Fabric};
use crate::palette;

/// Most names to offer for one shade.
const MAX_NAMES: usize = 5;

/// `GET /colors/name?hex=%23f4c2c2` → `baby pink` (plain text). With
/// `&count=3`, the best name and alternatives, one per line.
pub async fn name_for_hex(
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
    Ok((
        [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
        palette::nearest_names(rgb, count).join("\n"),
    ))
}

/// The chips for photos chosen in a form but not saved yet: a piece of HTML
/// that colors.js puts on the page as it is.
#[derive(Template)]
#[template(path = "photo_color_ideas.html")]
struct ChosenPhotoIdeas {
    ideas: Vec<ColorIdea>,
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

/// Every named shade in the stash (fabrics' colors and accessories' own),
/// once each, in rainbow order. Colors without a swatch are left out: there's
/// no shade to reuse. Shades with the same name that look identical count
/// once; same-named shades that really differ are kept apart.
pub fn your_shades(fabrics: &[Fabric], accessories: &[Accessory]) -> Vec<ColorIdea> {
    let all = fabrics
        .iter()
        .flat_map(|f| &f.colors)
        .chain(accessories.iter().flat_map(|a| &a.colors));
    let mut shades: Vec<(ColorIdea, [u8; 3])> = Vec::new();
    for Color { name, hex } in all {
        let Some(hex) = hex else { continue };
        let Some(rgb) = palette::rgb_from_hex(hex) else {
            continue;
        };
        let idea = ColorIdea {
            name: name.clone(),
            hex: hex.clone(),
        };
        let repeat = shades
            .iter()
            .any(|(seen, seen_rgb)| seen.name == idea.name && palette::look_alike(*seen_rgb, rgb));
        if !repeat {
            shades.push((idea, rgb));
        }
    }
    shades.sort_by(|(a, rgb_a), (b, rgb_b)| {
        palette::rainbow_order(*rgb_a)
            .cmp(&palette::rainbow_order(*rgb_b))
            .then(a.name.cmp(&b.name))
    });
    shades.into_iter().map(|(idea, _)| idea).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::FabricId;

    fn fabric(colors: Vec<Color>) -> Fabric {
        Fabric {
            id: FabricId(1),
            name: "Any".into(),
            description: String::new(),
            colors,
            keywords: vec![],
            pairings: vec![],
            in_stock: true,
        }
    }

    #[test]
    fn your_shades_are_each_picked_shade_once_in_rainbow_order() {
        let fabrics = [
            fabric(vec![
                Color::with_hex("baby blue", "#afd2f0"),
                Color::with_hex("orange", "#f5821e"),
                "yellow".into(), // no swatch: nothing to reuse
            ]),
            fabric(vec![
                Color::with_hex("orange", "#f5821e"), // the same shade again
                Color::with_hex("orange", "#f6831f"), // one you couldn't tell apart
                Color::with_hex("orange", "#e06010"), // same name, another shade
                Color::with_hex("cream", "#f5eed7"),
            ]),
        ];
        let shades: Vec<(String, String)> = your_shades(&fabrics, &[])
            .into_iter()
            .map(|c| (c.name, c.hex))
            .collect();
        assert_eq!(
            shades,
            vec![
                ("orange".into(), "#e06010".into()),
                ("orange".into(), "#f5821e".into()),
                // Cream is a pale, warm yellow: it sits after the oranges.
                ("cream".into(), "#f5eed7".into()),
                ("baby blue".into(), "#afd2f0".into()),
            ]
        );
    }
}
