//! Finding the main colors in a photo, and giving each one a name.
//!
//! Pure color math: it works on pixels, with no files, database or web code,
//! so it's tested with made-up pixels.
//!
//! How the main colors are found:
//! 1. **Bucket** every pixel by rounding its red, green and blue down to 16
//!    levels each, like sorting sweets into cups by color. Each bucket keeps
//!    a count and the average of the pixels in it.
//! 2. **Merge** buckets that look alike, biggest first: shading and fabric
//!    texture split one real color across several neighbouring buckets.
//! 3. **Keep** colors covering at least [`MIN_SHARE`] of the photo, largest first.
//!
//! "Look alike" is measured in CIE Lab, a color space designed so that the
//! distance between two colors matches how different they look to people
//! (in plain RGB, two greens can be "far apart" yet look nearly the same).
//! The same distance picks each color's name from [`NAMES`].
//!
//! Why not k-means clustering (the textbook method)? It starts from random
//! guesses, so the same photo could give different answers on different days.
//! Buckets are deterministic and easy to explain.

/// Colors covering less of the photo than this are ignored (stray highlights,
/// the edge of a table).
pub const MIN_SHARE: f32 = 0.04;

/// Buckets closer than this (in Lab "delta E") are treated as one color.
/// Around 2 is the smallest difference people notice; 15 merges shades of
/// one fabric color but keeps, say, teal and green apart.
const MERGE_DISTANCE: f32 = 15.0;

/// A main color of a photo.
#[derive(Debug, Clone, PartialEq)]
pub struct Swatch {
    pub rgb: [u8; 3],
    /// Fraction of the photo's pixels (0–1).
    pub share: f32,
    /// The nearest name from [`NAMES`].
    pub name: &'static str,
}

impl Swatch {
    /// "#3aa79b", the format the color picker uses.
    pub fn hex(&self) -> String {
        let [r, g, b] = self.rgb;
        format!("#{r:02x}{g:02x}{b:02x}")
    }
}

/// The main colors of an image, largest first, at most `max` of them.
/// Two colors that get the same name are reported once (the larger one),
/// since the name is what suggestions match on.
pub fn main_colors(pixels: impl IntoIterator<Item = [u8; 3]>, max: usize) -> Vec<Swatch> {
    // 1. Bucket: each channel rounded down to 16 levels gives 16×16×16 =
    //    4,096 buckets. A plain array indexed by those levels is much faster
    //    than a HashMap here: a photo has a few hundred thousand pixels.
    let mut buckets = vec![Group::default(); 4096];
    let mut total = 0usize;
    for pixel @ [r, g, b] in pixels {
        total += 1;
        let index = (r as usize >> 4) << 8 | (g as usize >> 4) << 4 | (b as usize >> 4);
        buckets[index].add(pixel);
    }
    if total == 0 {
        return Vec::new();
    }

    // 2. Merge, biggest bucket first, so each group is centred on a real
    //    common color rather than on a stray one. The sort is stable, so equal
    //    buckets stay in index order and the result never varies.
    buckets.retain(|b| b.count > 0);
    buckets.iter_mut().for_each(Group::refresh_lab);
    buckets.sort_by_key(|b| std::cmp::Reverse(b.count));
    let mut groups: Vec<Group> = Vec::new();
    for bucket in buckets {
        match groups
            .iter_mut()
            .find(|g| distance(g.lab, bucket.lab) < MERGE_DISTANCE)
        {
            Some(group) => group.merge(&bucket),
            None => groups.push(bucket),
        }
    }
    groups.sort_by_key(|g| std::cmp::Reverse(g.count));

    // 3. Keep the big ones, one per name.
    let mut swatches: Vec<Swatch> = Vec::new();
    for group in groups {
        let share = group.count as f32 / total as f32;
        if share < MIN_SHARE || swatches.len() == max {
            break;
        }
        let rgb = group.average();
        let name = nearest_name(rgb);
        if !swatches.iter().any(|s| s.name == name) {
            swatches.push(Swatch { rgb, share, name });
        }
    }
    swatches
}

/// Pixels added together, so their average can be taken.
#[derive(Debug, Clone, Default)]
struct Group {
    count: usize,
    sum: [u64; 3],
    /// The average in Lab, kept up to date by `refresh_lab`. Converting to
    /// Lab is slow, and merging compares each group many times, so it's
    /// worked out once per change rather than once per comparison.
    lab: Lab,
}

impl Group {
    fn add(&mut self, rgb: [u8; 3]) {
        self.count += 1;
        for (sum, c) in self.sum.iter_mut().zip(rgb) {
            *sum += c as u64;
        }
    }

    fn merge(&mut self, other: &Group) {
        self.count += other.count;
        for (sum, other) in self.sum.iter_mut().zip(other.sum) {
            *sum += other;
        }
        self.refresh_lab();
    }

    fn refresh_lab(&mut self) {
        self.lab = to_lab(self.average());
    }

    fn average(&self) -> [u8; 3] {
        self.sum
            .map(|s| (s as f64 / self.count as f64).round() as u8)
    }
}

// ---------------------------------------------------------------------------
// Color names
// ---------------------------------------------------------------------------

/// Names a sewer would use, with a typical shade of each. A photo color gets
/// the name whose shade is closest. Edit freely: suggestions match on names,
/// so names that also appear in event keywords ("orange", "gold") are useful.
///
/// A name can appear more than once. The simple distance formula used here
/// over-weights saturation, so a vivid shade and a dusty one of the same color
/// can be far apart; a second entry covers the other end (see "navy").
pub const NAMES: &[(&str, [u8; 3])] = &[
    ("white", [250, 250, 250]),
    ("cream", [245, 238, 215]),
    ("beige", [222, 205, 170]),
    ("tan", [200, 165, 120]),
    ("brown", [130, 85, 50]),
    ("chocolate", [80, 50, 30]),
    ("black", [20, 20, 20]),
    ("charcoal", [60, 62, 66]),
    ("grey", [128, 128, 128]),
    ("light grey", [200, 200, 200]),
    ("silver", [175, 180, 185]),
    ("red", [205, 30, 40]),
    ("burgundy", [120, 20, 40]),
    ("coral", [250, 125, 100]),
    ("peach", [255, 200, 165]),
    ("orange", [245, 130, 30]),
    ("burnt orange", [200, 85, 25]),
    ("gold", [215, 170, 50]),
    ("mustard", [205, 165, 30]),
    ("yellow", [250, 220, 40]),
    ("pale yellow", [255, 245, 160]),
    ("olive", [120, 120, 40]),
    ("lime", [170, 215, 50]),
    ("green", [50, 160, 70]),
    ("forest green", [30, 85, 45]),
    ("mint", [170, 230, 195]),
    ("sage", [160, 180, 140]),
    ("teal", [0, 128, 128]),
    ("teal green", [40, 170, 140]),
    ("turquoise", [60, 205, 200]),
    ("sky blue", [125, 195, 235]),
    ("baby blue", [175, 210, 240]),
    ("blue", [40, 100, 200]),
    ("royal blue", [40, 60, 170]),
    ("navy", [25, 35, 80]),
    // Dyed-fabric navy above; the vivid "CSS navy" here.
    ("navy", [0, 0, 128]),
    ("lavender", [200, 185, 235]),
    ("lilac", [190, 160, 210]),
    ("purple", [120, 50, 150]),
    ("plum", [100, 40, 80]),
    ("magenta", [210, 30, 140]),
    ("hot pink", [245, 70, 150]),
    ("pink", [245, 150, 185]),
    ("baby pink", [245, 200, 210]),
    ("rose", [200, 100, 120]),
    ("mauve", [175, 130, 150]),
];

/// The name in [`NAMES`] that looks most like `rgb`.
pub fn nearest_name(rgb: [u8; 3]) -> &'static str {
    let lab = to_lab(rgb);
    NAMES
        .iter()
        .min_by(|(_, a), (_, b)| distance(to_lab(*a), lab).total_cmp(&distance(to_lab(*b), lab)))
        .map(|(name, _)| *name)
        .unwrap_or("color")
}

/// Two shades too close to tell apart side by side (a Lab difference under 5;
/// around 2 is the smallest difference people notice at all).
pub fn look_alike(a: [u8; 3], b: [u8; 3]) -> bool {
    distance(to_lab(a), to_lab(b)) < 5.0
}

/// "#f4c2c2" → [244, 194, 194]. `None` for anything that isn't `#rrggbb`.
pub fn rgb_from_hex(hex: &str) -> Option<[u8; 3]> {
    let digits = hex.strip_prefix('#')?;
    if digits.len() != 6 || !digits.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let channel = |i: usize| u8::from_str_radix(&digits[i..i + 2], 16).ok();
    Some([channel(0)?, channel(2)?, channel(4)?])
}

/// Sort key that lays colors out like a rainbow: reds, oranges, yellows,
/// greens, blues, purples, pinks; then the greys, whites and blacks (which
/// have no real hue) from light to dark.
pub fn rainbow_order(rgb: [u8; 3]) -> (bool, u16, u16) {
    let [r, g, b] = rgb.map(|c| c as f32 / 255.0);
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    let lightness = (max + min) / 2.0;
    let chroma = max - min;
    // How colorful it is, relative to how colorful it could be at this
    // lightness (the "S" of HSL).
    let saturation = if chroma == 0.0 {
        0.0
    } else {
        chroma / (1.0 - (2.0 * lightness - 1.0).abs())
    };
    let darkness = ((1.0 - lightness) * 1000.0) as u16;
    // Too grey to have a meaningful hue: sorted after the colors.
    if saturation < 0.15 || chroma < 0.08 {
        return (true, 0, darkness);
    }
    // Hue is a circle: 0° and 360° are both red, so a slightly bluish red
    // (356°) would sort after pink (337°). Starting the circle 15° earlier
    // keeps all reds together at the front.
    let hue = (hue(rgb) + 15.0).rem_euclid(360.0);
    (false, (hue * 10.0) as u16, darkness)
}

/// Where a color sits on the color wheel, 0–360°: 0 red, 60 yellow,
/// 120 green, 240 blue, 300 magenta (the "H" of HSL). Meaningless for greys.
fn hue(rgb: [u8; 3]) -> f32 {
    let [r, g, b] = rgb.map(|c| c as f32 / 255.0);
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    let chroma = max - min;
    if chroma == 0.0 {
        0.0
    } else if max == r {
        60.0 * ((g - b) / chroma).rem_euclid(6.0)
    } else if max == g {
        60.0 * ((b - r) / chroma + 2.0)
    } else {
        60.0 * ((r - g) / chroma + 4.0)
    }
}

// ---------------------------------------------------------------------------
// Warm and cool
// ---------------------------------------------------------------------------

/// Warm or cool, the way sewers and painters sort colors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Tone {
    /// Reds, pinks, oranges, yellows (and the browns and tans made from them).
    Warm,
    /// Greens, blues, purples.
    Cool,
    /// Almost no color at all: white, cream, greys, black.
    Neutral,
}

/// Below this colorfulness (Lab chroma) a color counts as neutral. Cream is
/// about 12; the palest pastels (baby pink, baby blue) are about 17, so they
/// keep their tone.
const NEUTRAL_CHROMA: f32 = 15.0;

/// The tone of one shade. Colorfulness is measured in Lab, like the color
/// names: in plain RGB a pale cream looks "very saturated", because the
/// formula divides by how much room is left near white.
pub fn tone(rgb: [u8; 3]) -> Tone {
    let [_, a, b] = to_lab(rgb);
    if a.hypot(b) < NEUTRAL_CHROMA {
        return Tone::Neutral;
    }
    // Warm runs from pink (330°) through red and orange to yellow (75°);
    // past that, yellow turns green and the rest of the wheel is cool.
    let hue = hue(rgb);
    if !(75.0..330.0).contains(&hue) {
        Tone::Warm
    } else {
        Tone::Cool
    }
}

// ---------------------------------------------------------------------------
// CIE Lab
// ---------------------------------------------------------------------------

/// L (lightness 0–100), a (green ↔ red), b (blue ↔ yellow).
type Lab = [f32; 3];

/// Convert screen RGB (sRGB) to Lab, by way of CIE XYZ with a daylight (D65)
/// white point. The formulas are the standard ones; the steps are:
/// undo the screen's brightness curve, mix into XYZ, then compress like the eye.
fn to_lab(rgb: [u8; 3]) -> Lab {
    let linear = rgb.map(|c| {
        let c = c as f32 / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    });
    let [r, g, b] = linear;
    let x = (0.4124 * r + 0.3576 * g + 0.1805 * b) / 0.95047;
    let y = 0.2126 * r + 0.7152 * g + 0.0722 * b;
    let z = (0.0193 * r + 0.1192 * g + 0.9505 * b) / 1.08883;
    let f = |t: f32| {
        if t > 0.008856 {
            t.cbrt()
        } else {
            7.787 * t + 16.0 / 116.0
        }
    };
    let (fx, fy, fz) = (f(x), f(y), f(z));
    [116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)]
}

/// How different two colors look ("delta E", the 1976 formula: straight-line
/// distance in Lab).
fn distance(a: Lab, b: Lab) -> f32 {
    a.iter()
        .zip(b)
        .map(|(x, y)| (x - y).powi(2))
        .sum::<f32>()
        .sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tones_follow_the_color_wheel() {
        let tone_of = |name: &str| {
            let (_, rgb) = NAMES.iter().find(|(n, _)| *n == name).unwrap();
            tone(*rgb)
        };
        for warm in [
            "red",
            "coral",
            "orange",
            "gold",
            "yellow",
            "hot pink",
            "baby pink",
            "brown",
        ] {
            assert_eq!(tone_of(warm), Tone::Warm, "{warm}");
        }
        for cool in [
            "lime",
            "sage",
            "teal",
            "baby blue",
            "navy",
            "lavender",
            "purple",
            "magenta",
        ] {
            assert_eq!(tone_of(cool), Tone::Cool, "{cool}");
        }
        for neutral in ["white", "cream", "silver", "charcoal", "black"] {
            assert_eq!(tone_of(neutral), Tone::Neutral, "{neutral}");
        }
        // A bluish red is still red, across the 0°/360° seam.
        assert_eq!(tone([200, 20, 40]), Tone::Warm);
    }

    fn close(a: Lab, b: Lab) -> bool {
        a.iter().zip(b).all(|(x, y)| (x - y).abs() < 0.1)
    }

    #[test]
    fn lab_matches_published_values() {
        assert!(close(to_lab([255, 255, 255]), [100.0, 0.0, 0.0]));
        assert!(close(to_lab([0, 0, 0]), [0.0, 0.0, 0.0]));
        // Pure red, from any Lab reference table.
        assert!(close(to_lab([255, 0, 0]), [53.24, 80.09, 67.20]));
    }

    #[test]
    fn names_come_from_the_nearest_shade() {
        assert_eq!(nearest_name([255, 255, 255]), "white");
        assert_eq!(nearest_name([0, 0, 0]), "black");
        assert_eq!(nearest_name([255, 128, 0]), "orange");
        assert_eq!(nearest_name([244, 194, 194]), "baby pink");
        assert_eq!(nearest_name([0, 0, 128]), "navy");
        assert_eq!(nearest_name([30, 40, 75]), "navy", "dusty fabric navy too");
        assert_eq!(
            nearest_name([65, 105, 225]),
            "blue",
            "not every blue is navy"
        );
        assert_eq!(nearest_name([58, 167, 155]), "teal green");
    }

    /// `n` pixels of `rgb`, wobbling by a few levels like real fabric.
    fn patch(rgb: [u8; 3], n: usize) -> Vec<[u8; 3]> {
        (0..n)
            .map(|i| rgb.map(|c| c.saturating_add((i % 7) as u8 * 3)))
            .collect()
    }

    #[test]
    fn finds_the_main_colors_largest_first() {
        // 60% teal, 30% pink, 10% white.
        let pixels = [
            patch([40, 170, 140], 600),
            patch([245, 150, 185], 300),
            patch([250, 250, 250], 100),
        ]
        .concat();
        let colors = main_colors(pixels, 6);
        let names: Vec<&str> = colors.iter().map(|s| s.name).collect();
        assert_eq!(names, vec!["teal green", "pink", "white"]);
        assert!((colors[0].share - 0.6).abs() < 0.01);
        assert!(colors[0].hex().starts_with('#') && colors[0].hex().len() == 7);
    }

    #[test]
    fn shades_of_one_color_merge_into_one() {
        // Light and shadowed orange: one fabric, one color.
        let pixels = [patch([245, 130, 30], 500), patch([225, 115, 25], 500)].concat();
        let colors = main_colors(pixels, 6);
        assert_eq!(colors.len(), 1);
        assert_eq!(colors[0].name, "orange");
        assert!((colors[0].share - 1.0).abs() < 0.001);
    }

    #[test]
    fn small_specks_and_extra_colors_are_left_out() {
        let pixels = [
            patch([25, 35, 80], 970),  // navy
            patch([250, 220, 40], 30), // 3% yellow: below MIN_SHARE
        ]
        .concat();
        let names: Vec<&str> = main_colors(pixels, 6).iter().map(|s| s.name).collect();
        assert_eq!(names, vec!["navy"]);

        let rainbow = [
            patch([205, 30, 40], 200),
            patch([245, 130, 30], 200),
            patch([250, 220, 40], 200),
            patch([50, 160, 70], 200),
            patch([40, 100, 200], 200),
        ]
        .concat();
        assert_eq!(main_colors(rainbow, 3).len(), 3, "at most `max`");
    }

    #[test]
    fn the_same_photo_always_gives_the_same_answer() {
        let pixels = [patch([200, 100, 120], 50), patch([100, 40, 80], 50)].concat();
        let first = main_colors(pixels.clone(), 6);
        for _ in 0..20 {
            assert_eq!(main_colors(pixels.clone(), 6), first);
        }
    }

    #[test]
    fn an_empty_image_has_no_colors() {
        assert!(main_colors(Vec::new(), 6).is_empty());
    }

    #[test]
    fn hex_codes_become_rgb() {
        assert_eq!(rgb_from_hex("#f4c2c2"), Some([244, 194, 194]));
        assert_eq!(rgb_from_hex("#000000"), Some([0, 0, 0]));
        assert_eq!(rgb_from_hex("f4c2c2"), None);
        assert_eq!(rgb_from_hex("#f4c2"), None);
        assert_eq!(rgb_from_hex("#gggggg"), None);
    }

    #[test]
    fn colors_sort_like_a_rainbow_with_greys_last() {
        let mut colors = [
            ("grey", [128, 128, 128]),
            ("blue", [40, 100, 200]),
            ("white", [250, 250, 250]),
            ("red", [205, 30, 40]),
            ("pink", [245, 150, 185]),
            ("yellow", [250, 220, 40]),
            ("black", [20, 20, 20]),
            ("green", [50, 160, 70]),
        ];
        colors.sort_by_key(|(_, rgb)| rainbow_order(*rgb));
        let names: Vec<&str> = colors.iter().map(|(n, _)| *n).collect();
        assert_eq!(
            names,
            vec![
                "red", "yellow", "green", "blue", "pink", "white", "grey", "black"
            ]
        );
    }

    #[test]
    fn near_identical_shades_look_alike() {
        assert!(look_alike([64, 224, 208], [66, 226, 210]));
        assert!(!look_alike([64, 224, 208], [0, 128, 128]));
    }
}
