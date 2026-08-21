//! CSS `clip-path` shapes, built as tiny-skia paths.
//!
//! The spec types `clip-path` as a string and kit hands it to `style.clipPath`,
//! so the values documents carry are CSS's basic shapes. `inset()`, `circle()`,
//! `ellipse()` and `polygon()` are built here; `path()` is handed to the SVG
//! parser instead, since that is what its argument already is.
//!
//! Percentages resolve against the node's own box, as CSS resolves them against
//! the reference box.

use crate::paint::{ellipse_path as ellipse_bounds_path, rounded_rect_path};
use tiny_skia::{PathBuilder, Rect};

/// The box a clip path's percentages and offsets resolve against.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ClipBox {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// Builds a `clip-path` value as a path, or `None` when the value is not a
/// shape this understands.
///
/// A `path()` value returns `None` here; the caller renders it through the SVG
/// parser, which already knows the `d` grammar.
pub(crate) fn clip_path(value: &str, area: ClipBox) -> Option<tiny_skia::Path> {
    let (name, argument) = split_function(value)?;

    match name.as_str() {
        "inset" => inset_path(&argument, area),
        "circle" => circle_path(&argument, area),
        "ellipse" => ellipse_path(&argument, area),
        "polygon" => polygon_path(&argument, area),
        _ => None,
    }
}

/// The `d` of a `path()` value, for the caller to hand to the SVG parser.
pub(crate) fn svg_path_data(value: &str) -> Option<String> {
    let (name, argument) = split_function(value)?;
    if name != "path" {
        return None;
    }

    // `path("M0 0 L10 0 Z")` — the argument is quoted, and may carry a
    // fill-rule before it, which the caller applies separately.
    let argument = argument
        .trim()
        .trim_start_matches("nonzero")
        .trim_start_matches("evenodd")
        .trim()
        .trim_start_matches(',')
        .trim();

    let unquoted = argument
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
        .or_else(|| {
            argument
                .strip_prefix('\'')
                .and_then(|rest| rest.strip_suffix('\''))
        })
        .unwrap_or(argument);

    (!unquoted.is_empty()).then(|| unquoted.to_owned())
}

fn split_function(value: &str) -> Option<(String, String)> {
    let value = value.trim();
    let open = value.find('(')?;
    let close = value.rfind(')')?;
    if close < open {
        return None;
    }

    Some((
        value[..open].trim().to_lowercase(),
        value[open + 1..close].to_owned(),
    ))
}

/// `inset(top right bottom left round <radius>)`, with CSS's 1-to-4 shorthand.
fn inset_path(argument: &str, area: ClipBox) -> Option<tiny_skia::Path> {
    let (offsets, radius) = match argument.split_once("round") {
        Some((offsets, radius)) => (offsets, radius.trim()),
        None => (argument, ""),
    };

    // CSS reads one to four offsets as top/right/bottom/left. The shorthand is
    // resolved before the percentages are, because which extent a percentage
    // resolves against depends on which side it lands on: the vertical ones
    // take the height and the horizontal ones the width.
    let sides: Vec<&str> = offsets.split_whitespace().collect();
    let (top, right, bottom, left) = match sides[..] {
        [] => return None,
        [all] => (all, all, all, all),
        [vertical, horizontal] => (vertical, horizontal, vertical, horizontal),
        [top, horizontal, bottom] => (top, horizontal, bottom, horizontal),
        [top, right, bottom, left, ..] => (top, right, bottom, left),
    };

    let top = length(top, area.height)?;
    let bottom = length(bottom, area.height)?;
    let left = length(left, area.width)?;
    let right = length(right, area.width)?;

    let rect = Rect::from_xywh(
        area.x + left,
        area.y + top,
        (area.width - left - right).max(0.0),
        (area.height - top - bottom).max(0.0),
    )?;

    let corner = length(radius, area.width.min(area.height)).unwrap_or(0.0);
    rounded_rect_path(rect.x(), rect.y(), rect.width(), rect.height(), corner)
}

/// `circle(<radius> at <x> <y>)`.
fn circle_path(argument: &str, area: ClipBox) -> Option<tiny_skia::Path> {
    let (radius, centre) = split_at_position(argument);
    let (cx, cy) = position(centre, area);

    // CSS resolves a circle's percentage radius against the diagonal, so a
    // 50% circle in a square box touches the edges rather than the corners.
    let diagonal = (area.width.powi(2) + area.height.powi(2)).sqrt() / 2.0_f32.sqrt();
    let r = match radius.trim() {
        "" | "closest-side" => (area.width / 2.0).min(area.height / 2.0),
        "farthest-side" => (area.width / 2.0).max(area.height / 2.0),
        value => length(value, diagonal)?,
    };

    oval(cx, cy, r, r)
}

/// `ellipse(<rx> <ry> at <x> <y>)`.
fn ellipse_path(argument: &str, area: ClipBox) -> Option<tiny_skia::Path> {
    let (radii, centre) = split_at_position(argument);
    let (cx, cy) = position(centre, area);

    let mut parts = radii.split_whitespace();
    let rx = match parts.next() {
        None | Some("closest-side") => area.width / 2.0,
        Some("farthest-side") => area.width / 2.0,
        Some(value) => length(value, area.width)?,
    };
    let ry = match parts.next() {
        None | Some("closest-side") => area.height / 2.0,
        Some("farthest-side") => area.height / 2.0,
        Some(value) => length(value, area.height)?,
    };

    oval(cx, cy, rx, ry)
}

/// `polygon(<fill-rule>?, x y, x y, ...)`.
fn polygon_path(argument: &str, area: ClipBox) -> Option<tiny_skia::Path> {
    let mut points = Vec::new();
    for pair in argument.split(',') {
        let pair = pair.trim();
        // A leading fill-rule is a comma-separated item of its own.
        if pair == "nonzero" || pair == "evenodd" || pair.is_empty() {
            continue;
        }

        let mut parts = pair.split_whitespace();
        let x = length(parts.next()?, area.width)?;
        let y = length(parts.next()?, area.height)?;
        points.push((area.x + x, area.y + y));
    }

    if points.len() < 3 {
        return None;
    }

    let mut builder = PathBuilder::new();
    builder.move_to(points[0].0, points[0].1);
    for (x, y) in &points[1..] {
        builder.line_to(*x, *y);
    }
    builder.close();
    builder.finish()
}

/// A centre-and-radii ellipse, as the painter's bounding-box one.
fn oval(cx: f32, cy: f32, rx: f32, ry: f32) -> Option<tiny_skia::Path> {
    ellipse_bounds_path(cx - rx, cy - ry, rx * 2.0, ry * 2.0)
}

/// Splits `40% at 50% 50%` into the shape's size and its centre.
fn split_at_position(argument: &str) -> (&str, &str) {
    match argument.split_once(" at ") {
        Some((size, centre)) => (size, centre),
        None => (argument, ""),
    }
}

/// A `<position>`, defaulting to the box's centre as CSS does.
fn position(value: &str, area: ClipBox) -> (f32, f32) {
    let mut parts = value.split_whitespace();
    let x = parts
        .next()
        .and_then(|part| keyword_or_length(part, area.width))
        .unwrap_or(area.width / 2.0);
    let y = parts
        .next()
        .and_then(|part| keyword_or_length(part, area.height))
        .unwrap_or(area.height / 2.0);

    (area.x + x, area.y + y)
}

fn keyword_or_length(value: &str, extent: f32) -> Option<f32> {
    match value.trim() {
        "left" | "top" => Some(0.0),
        "center" => Some(extent / 2.0),
        "right" | "bottom" => Some(extent),
        value => length(value, extent),
    }
}

/// A length in pixels or a percentage of `extent`.
fn length(value: &str, extent: f32) -> Option<f32> {
    let value = value.trim();
    match value.strip_suffix('%') {
        Some(percentage) => percentage
            .trim()
            .parse::<f32>()
            .ok()
            .map(|it| it / 100.0 * extent),
        None => value.trim_end_matches("px").trim().parse::<f32>().ok(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AREA: ClipBox = ClipBox {
        x: 10.0,
        y: 20.0,
        width: 100.0,
        height: 50.0,
    };

    fn bounds(value: &str) -> (f32, f32, f32, f32) {
        let path = clip_path(value, AREA).expect("shape builds");
        let rect = path.bounds();
        (rect.left(), rect.top(), rect.width(), rect.height())
    }

    #[test]
    fn inset_takes_offsets_off_each_side() {
        assert_eq!(bounds("inset(10px 20px)"), (30.0, 30.0, 60.0, 30.0));
    }

    #[test]
    fn inset_offsets_can_be_percentages_of_the_box() {
        // 10% of the width is 10, 20% of the height is 10.
        assert_eq!(bounds("inset(20% 10%)"), (20.0, 30.0, 80.0, 30.0));
    }

    #[test]
    fn a_single_inset_offset_applies_to_every_side() {
        assert_eq!(bounds("inset(5px)"), (15.0, 25.0, 90.0, 40.0));
    }

    #[test]
    fn circle_centres_on_the_box_by_default() {
        let (left, top, width, height) = bounds("circle(20px)");
        assert_eq!((width, height), (40.0, 40.0));
        assert_eq!((left, top), (40.0, 25.0));
    }

    #[test]
    fn circle_takes_an_explicit_centre() {
        let (left, top, ..) = bounds("circle(10px at 0 0)");
        assert_eq!((left, top), (0.0, 10.0));
    }

    #[test]
    fn ellipse_takes_a_radius_per_axis() {
        let (_, _, width, height) = bounds("ellipse(30px 10px at 50% 50%)");
        assert_eq!((width, height), (60.0, 20.0));
    }

    #[test]
    fn polygon_spans_its_points() {
        assert_eq!(
            bounds("polygon(0% 0%, 100% 0%, 50% 100%)"),
            (10.0, 20.0, 100.0, 50.0)
        );
    }

    #[test]
    fn polygon_ignores_a_leading_fill_rule() {
        assert_eq!(
            bounds("polygon(evenodd, 0 0, 100 0, 50 50)"),
            bounds("polygon(0 0, 100 0, 50 50)")
        );
    }

    #[test]
    fn a_polygon_needs_three_points_to_enclose_anything() {
        assert!(clip_path("polygon(0 0, 10 10)", AREA).is_none());
    }

    #[test]
    fn path_data_is_handed_on_rather_than_built_here() {
        assert!(clip_path("path(\"M0 0 L10 0 Z\")", AREA).is_none());
        assert_eq!(
            svg_path_data("path(\"M0 0 L10 0 Z\")").as_deref(),
            Some("M0 0 L10 0 Z")
        );
        assert_eq!(
            svg_path_data("path(evenodd, \"M0 0 Z\")").as_deref(),
            Some("M0 0 Z")
        );
    }

    #[test]
    fn an_unknown_shape_builds_nothing() {
        assert!(clip_path("frobnicate(10px)", AREA).is_none());
        assert!(clip_path("nonsense", AREA).is_none());
    }
}
