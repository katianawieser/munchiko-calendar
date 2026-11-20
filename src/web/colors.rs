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
use super::photos::{self, ColorIdea, FormWithPhotos, IdeaGroup, IdeaKind};
use super::{AppError, AppState, render};
use crate::models::{Accessory, Fabric};
use crate::palette;
use crate::shades::{self, Closeness};

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

/// The stash's shades to reuse with one click: one per group of look-alikes
/// (at the Shades page's default closeness), shown as that group's most used
/// shade under its most used name, in rainbow order. Colors without a swatch
/// have no shade to reuse. Merging look-alikes on the Shades page makes the
/// stash itself match.
pub fn your_shades(fabrics: &[Fabric], accessories: &[Accessory]) -> Vec<ColorIdea> {
    let all = shades::collect(fabrics, accessories);
    shades::look_alike_groups(&all, Closeness::Medium)
        .iter()
        .map(|group| ColorIdea {
            name: group.leader().name().to_string(),
            hex: group.leader().hex.clone(),
            kind: IdeaKind::Yours,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{Color, FabricId};

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
    fn your_shades_are_one_per_look_alike_group_in_rainbow_order() {
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
                // Looks like the orange, another name: joins its group,
                // shown under the more used name.
                Color::with_hex("pumpkin", "#f4801c"),
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
