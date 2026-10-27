//! Color helpers for the fabric and accessory forms:
//! - [`name_for_hex`]: the page asks what to call a shade that was just picked,
//!   so naming uses the same code (and list of names) as the photo colors;
//! - [`your_shades`]: every shade already picked somewhere in the stash, to
//!   reuse with one click instead of picking it again.

use std::collections::HashMap;

use axum::extract::Query;
use axum::http::header;
use axum::response::IntoResponse;

use super::AppError;
use super::photos::ColorIdea;
use crate::models::{Accessory, Color, Fabric};
use crate::palette;

/// `GET /colors/name?hex=%23f4c2c2` → `baby pink` (plain text).
pub async fn name_for_hex(
    Query(query): Query<HashMap<String, String>>,
) -> Result<impl IntoResponse, AppError> {
    let rgb = query
        .get("hex")
        .and_then(|hex| palette::rgb_from_hex(&hex.to_ascii_lowercase()))
        .ok_or(AppError::BadRequest)?;
    Ok((
        [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
        palette::nearest_name(rgb),
    ))
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
