//! CSS `filter` functions, applied to a finished layer.
//!
//! The spec types `filter` as a CSS filter string, and kit hands it straight to
//! `style.filter`, so the values documents carry are CSS's. The colour matrices
//! here are the ones the Filter Effects spec defines, so `brightness(1.2)` means
//! the same thing it does in a browser.
//!
//! Functions are applied left to right, as CSS applies them.

use crate::blur;
use tiny_skia::Pixmap;

/// Applies a CSS `filter` list to `pixmap` in place.
///
/// An unrecognised or malformed function is skipped rather than failing the
/// paint: a document that asks for something not implemented yet should come
/// out unfiltered, not blank.
pub(crate) fn apply_filter(pixmap: &mut Pixmap, filter: &str) {
    for (name, argument) in parse_functions(filter) {
        apply_one(pixmap, &name, argument.as_deref());
    }
}

fn apply_one(pixmap: &mut Pixmap, name: &str, argument: Option<&str>) {
    match name {
        // `blur()` takes a length, and CSS calls it a standard deviation here,
        // unlike `box-shadow`'s radius.
        "blur" => blur::blur(pixmap, argument.and_then(parse_length).unwrap_or(0.0)),
        "opacity" => {
            let amount = argument.and_then(parse_amount).unwrap_or(1.0).max(0.0);
            scale_alpha(pixmap, amount);
        }
        "brightness" => {
            let amount = argument.and_then(parse_amount).unwrap_or(1.0).max(0.0);
            map_channels(pixmap, |channel| channel * amount);
        }
        "contrast" => {
            let amount = argument.and_then(parse_amount).unwrap_or(1.0).max(0.0);
            let intercept = 0.5 - amount / 2.0;
            map_channels(pixmap, |channel| channel * amount + intercept);
        }
        "invert" => {
            let amount = argument
                .and_then(parse_amount)
                .unwrap_or(1.0)
                .clamp(0.0, 1.0);
            map_channels(pixmap, |channel| {
                channel * (1.0 - amount) + (1.0 - channel) * amount
            });
        }
        // The remaining three are colour matrices interpolated against the
        // identity by `amount`, exactly as the Filter Effects spec writes them.
        "grayscale" => {
            let amount = argument
                .and_then(parse_amount)
                .unwrap_or(1.0)
                .clamp(0.0, 1.0);
            apply_matrix(pixmap, &lerp_matrix(GRAYSCALE, amount));
        }
        "sepia" => {
            let amount = argument
                .and_then(parse_amount)
                .unwrap_or(1.0)
                .clamp(0.0, 1.0);
            apply_matrix(pixmap, &lerp_matrix(SEPIA, amount));
        }
        "saturate" => {
            let amount = argument.and_then(parse_amount).unwrap_or(1.0).max(0.0);
            apply_matrix(pixmap, &saturate_matrix(amount));
        }
        _ => {}
    }
}

/// Splits `brightness(1.2) contrast(0.9)` into its functions.
fn parse_functions(filter: &str) -> Vec<(String, Option<String>)> {
    let mut functions = Vec::new();
    let mut rest = filter.trim();

    while let Some(open) = rest.find('(') {
        let name = rest[..open].trim().to_lowercase();
        let Some(close) = rest[open..].find(')') else {
            break;
        };
        let argument = rest[open + 1..open + close].trim();
        if !name.is_empty() {
            functions.push((name, (!argument.is_empty()).then(|| argument.to_owned())));
        }
        rest = &rest[open + close + 1..];
    }

    functions
}

/// A filter amount, as a factor or a percentage: `1.2` or `120%`.
fn parse_amount(value: &str) -> Option<f32> {
    match value.trim().strip_suffix('%') {
        Some(percentage) => percentage.trim().parse::<f32>().ok().map(|it| it / 100.0),
        None => value.trim().parse::<f32>().ok(),
    }
}

/// A length in pixels; `px` is the only unit a `.gui` document uses.
fn parse_length(value: &str) -> Option<f32> {
    value
        .trim()
        .trim_end_matches("px")
        .trim()
        .parse::<f32>()
        .ok()
}

/// Runs a per-channel function over the pixmap's colours.
///
/// The buffer is premultiplied, so each pixel is divided by its alpha, mapped,
/// and multiplied back — mapping premultiplied bytes directly would darken
/// translucent pixels twice.
fn map_channels(pixmap: &mut Pixmap, map: impl Fn(f32) -> f32) {
    for pixel in pixmap.data_mut().chunks_exact_mut(4) {
        let alpha = pixel[3] as f32 / 255.0;
        if alpha <= 0.0 {
            continue;
        }

        for channel in &mut pixel[..3] {
            let straight = (*channel as f32 / 255.0) / alpha;
            *channel = to_byte(map(straight) * alpha);
        }
    }
}

fn scale_alpha(pixmap: &mut Pixmap, amount: f32) {
    for pixel in pixmap.data_mut().chunks_exact_mut(4) {
        // Premultiplied, so every channel scales with the alpha.
        for channel in pixel {
            *channel = to_byte(*channel as f32 / 255.0 * amount);
        }
    }
}

/// A 3x3 colour matrix, row-major: red, green, blue.
type ColorMatrix = [[f32; 3]; 3];

const IDENTITY: ColorMatrix = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

/// Rec. 709 luminance, which is what the spec's grayscale matrix is made of.
const LUMA: [f32; 3] = [0.2126, 0.7152, 0.0722];

const GRAYSCALE: ColorMatrix = [LUMA, LUMA, LUMA];

const SEPIA: ColorMatrix = [
    [0.393, 0.769, 0.189],
    [0.349, 0.686, 0.168],
    [0.272, 0.534, 0.131],
];

/// Interpolates a matrix against the identity, which is how CSS turns a full
/// effect into a partial one.
fn lerp_matrix(target: ColorMatrix, amount: f32) -> ColorMatrix {
    let mut result = IDENTITY;
    for row in 0..3 {
        for column in 0..3 {
            result[row][column] =
                IDENTITY[row][column] * (1.0 - amount) + target[row][column] * amount;
        }
    }
    result
}

/// `saturate(0)` is grayscale and `saturate(1)` the identity, so the matrix is
/// the same interpolation extended past 1 rather than clamped at it.
fn saturate_matrix(amount: f32) -> ColorMatrix {
    let mut result = IDENTITY;
    for (row, identity_row) in IDENTITY.iter().enumerate() {
        for (column, luma) in LUMA.iter().enumerate() {
            result[row][column] = luma + (identity_row[column] - luma) * amount;
        }
    }
    result
}

fn apply_matrix(pixmap: &mut Pixmap, matrix: &ColorMatrix) {
    for pixel in pixmap.data_mut().chunks_exact_mut(4) {
        let alpha = pixel[3] as f32 / 255.0;
        if alpha <= 0.0 {
            continue;
        }

        let straight = [
            (pixel[0] as f32 / 255.0) / alpha,
            (pixel[1] as f32 / 255.0) / alpha,
            (pixel[2] as f32 / 255.0) / alpha,
        ];
        for (channel, row) in pixel[..3].iter_mut().zip(matrix) {
            let value = row[0] * straight[0] + row[1] * straight[1] + row[2] * straight[2];
            *channel = to_byte(value * alpha);
        }
    }
}

fn to_byte(value: f32) -> u8 {
    (value.clamp(0.0, 1.0) * 255.0).round() as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(r: u8, g: u8, b: u8) -> Pixmap {
        let mut pixmap = Pixmap::new(2, 2).expect("pixmap allocates");
        pixmap.fill(tiny_skia::Color::from_rgba8(r, g, b, 255));
        pixmap
    }

    fn first(pixmap: &Pixmap) -> [u8; 4] {
        let data = pixmap.data();
        [data[0], data[1], data[2], data[3]]
    }

    #[test]
    fn splits_a_filter_list_into_its_functions() {
        assert_eq!(
            parse_functions("brightness(1.2) contrast( 90% ) grayscale()"),
            vec![
                ("brightness".to_owned(), Some("1.2".to_owned())),
                ("contrast".to_owned(), Some("90%".to_owned())),
                ("grayscale".to_owned(), None),
            ]
        );
    }

    #[test]
    fn brightness_scales_channels() {
        let mut pixmap = solid(100, 100, 100);
        apply_filter(&mut pixmap, "brightness(1.5)");
        assert_eq!(first(&pixmap), [150, 150, 150, 255]);
    }

    #[test]
    fn grayscale_collapses_to_luminance() {
        let mut pixmap = solid(255, 0, 0);
        apply_filter(&mut pixmap, "grayscale(1)");
        // Rec. 709 luminance of pure red is 0.2126.
        let [r, g, b, _] = first(&pixmap);
        assert_eq!((r, g, b), (54, 54, 54));
    }

    #[test]
    fn a_percentage_amount_means_the_same_as_a_factor() {
        let mut percent = solid(100, 100, 100);
        let mut factor = solid(100, 100, 100);
        apply_filter(&mut percent, "brightness(150%)");
        apply_filter(&mut factor, "brightness(1.5)");
        assert_eq!(first(&percent), first(&factor));
    }

    #[test]
    fn saturate_zero_matches_grayscale() {
        let mut saturated = solid(200, 60, 30);
        let mut gray = solid(200, 60, 30);
        apply_filter(&mut saturated, "saturate(0)");
        apply_filter(&mut gray, "grayscale(1)");
        assert_eq!(first(&saturated), first(&gray));
    }

    #[test]
    fn functions_apply_left_to_right() {
        let mut once = solid(100, 100, 100);
        apply_filter(&mut once, "brightness(2) brightness(0.5)");
        assert_eq!(first(&once), [100, 100, 100, 255]);
    }

    #[test]
    fn opacity_scales_alpha() {
        let mut pixmap = solid(255, 255, 255);
        apply_filter(&mut pixmap, "opacity(0.5)");
        assert_eq!(first(&pixmap)[3], 128);
    }

    #[test]
    fn an_unknown_function_is_skipped_rather_than_failing() {
        let mut pixmap = solid(100, 100, 100);
        apply_filter(&mut pixmap, "hue-rotate(90deg) brightness(1.5)");
        assert_eq!(first(&pixmap), [150, 150, 150, 255]);
    }

    #[test]
    fn a_translucent_pixel_keeps_its_colour_through_a_no_op_filter() {
        let mut pixmap = Pixmap::new(2, 2).expect("pixmap allocates");
        pixmap.fill(tiny_skia::Color::from_rgba8(200, 100, 50, 128));
        let before = first(&pixmap);
        apply_filter(&mut pixmap, "brightness(1)");
        assert_eq!(first(&pixmap), before);
    }
}
