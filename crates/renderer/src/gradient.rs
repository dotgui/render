//! CSS gradient values, built as tiny-skia shaders.
//!
//! The spec's Fill Values page says gradient syntax mirrors CSS —
//! `linear-gradient()`, `radial-gradient()`, `conic-gradient()` — and kit hands
//! the value straight to a browser, so what documents carry is CSS's grammar.
//!
//! Angles follow CSS rather than trigonometry: `0deg` points up and they turn
//! clockwise, so `180deg` is the default top-to-bottom.

use tiny_skia::{
    GradientStop, LinearGradient, Point, RadialGradient, Shader, SpreadMode, SweepGradient,
    Transform,
};

/// The box a gradient's percentages and default extent resolve against.
#[derive(Debug, Clone, Copy)]
pub(crate) struct GradientBox {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl GradientBox {
    fn centre(&self) -> (f32, f32) {
        (self.x + self.width / 2.0, self.y + self.height / 2.0)
    }
}

/// Builds a gradient value as a shader, or `None` when the value is not a
/// gradient this understands.
///
/// `parse_color` is passed in so a stop resolves colours exactly as every other
/// paint does, rather than this module growing a second colour parser that can
/// drift from it.
pub(crate) fn gradient_shader(
    value: &str,
    area: GradientBox,
    opacity: f32,
    parse_color: &dyn Fn(&str, f32) -> Option<tiny_skia::Color>,
) -> Option<Shader<'static>> {
    let (name, argument) = split_function(value)?;
    let parts = split_top_level(&argument);
    if parts.is_empty() {
        return None;
    }

    match name.as_str() {
        "linear-gradient" => linear(&parts, area, opacity, parse_color),
        "radial-gradient" => radial(&parts, area, opacity, parse_color),
        // The spec calls it an angular gradient and writes it `conic-gradient`,
        // as CSS does; both spellings reach the same shader.
        "conic-gradient" | "angular-gradient" => sweep(&parts, area, opacity, parse_color),
        _ => None,
    }
}

/// Whether a value looks like a gradient at all, for callers deciding between
/// this and a solid colour.
pub(crate) fn is_gradient(value: &str) -> bool {
    matches!(
        split_function(value)
            .as_ref()
            .map(|(name, _)| name.as_str()),
        Some("linear-gradient" | "radial-gradient" | "conic-gradient" | "angular-gradient")
    )
}

fn linear(
    parts: &[String],
    area: GradientBox,
    opacity: f32,
    parse_color: &dyn Fn(&str, f32) -> Option<tiny_skia::Color>,
) -> Option<Shader<'static>> {
    // A leading angle or `to <side>` is optional; without one CSS runs the
    // gradient down the box.
    let (angle, stop_parts) = match direction_angle(&parts[0]) {
        Some(angle) => (angle, &parts[1..]),
        None => (180.0, parts),
    };

    let stops = shader_stops(gradient_stops(stop_parts, opacity, parse_color)?);
    let (cx, cy) = area.centre();
    let radians = angle.to_radians();
    let (dx, dy) = (radians.sin(), -radians.cos());

    // The gradient line is long enough that its ends sit on the box's edges,
    // which is what makes `0%` and `100%` land where CSS puts them.
    let length = (area.width * dx).abs() + (area.height * dy).abs();
    let half = length / 2.0;

    LinearGradient::new(
        Point::from_xy(cx - dx * half, cy - dy * half),
        Point::from_xy(cx + dx * half, cy + dy * half),
        stops,
        SpreadMode::Pad,
        Transform::identity(),
    )
}

fn radial(
    parts: &[String],
    area: GradientBox,
    opacity: f32,
    parse_color: &dyn Fn(&str, f32) -> Option<tiny_skia::Color>,
) -> Option<Shader<'static>> {
    let (centre, stop_parts) = match position_of(&parts[0], area) {
        Some(centre) => (centre, &parts[1..]),
        None => (area.centre(), parts),
    };

    let stops = shader_stops(gradient_stops(stop_parts, opacity, parse_color)?);

    // CSS defaults to `farthest-corner`: the gradient ends at whichever corner
    // is furthest from the centre.
    let (cx, cy) = centre;
    let radius = [
        (area.x, area.y),
        (area.x + area.width, area.y),
        (area.x, area.y + area.height),
        (area.x + area.width, area.y + area.height),
    ]
    .into_iter()
    .map(|(x, y)| ((x - cx).powi(2) + (y - cy).powi(2)).sqrt())
    .fold(0.0_f32, f32::max);

    RadialGradient::new(
        Point::from_xy(cx, cy),
        0.0,
        Point::from_xy(cx, cy),
        radius.max(0.01),
        stops,
        SpreadMode::Pad,
        Transform::identity(),
    )
}

fn sweep(
    parts: &[String],
    area: GradientBox,
    opacity: f32,
    parse_color: &dyn Fn(&str, f32) -> Option<tiny_skia::Color>,
) -> Option<Shader<'static>> {
    // `conic-gradient(from 45deg at 50% 50%, ...)` — either prelude may be
    // absent, and both live in the first comma-separated part.
    let mut start_angle = 0.0;
    let mut centre = area.centre();
    let mut stop_parts = parts;

    let head = &parts[0];
    if head.starts_with("from ") || head.starts_with("at ") || head.contains(" at ") {
        if let Some(from) = head.split("at").next() {
            if let Some(angle) = from.trim().strip_prefix("from") {
                start_angle = parse_angle(angle.trim()).unwrap_or(0.0);
            }
        }
        if let Some(at) = head
            .split_once(" at ")
            .map(|(_, at)| at)
            .or_else(|| head.strip_prefix("at "))
        {
            centre = position_of(&format!("at {at}"), area).unwrap_or(centre);
        }
        stop_parts = &parts[1..];
    }

    let stops = shader_stops(gradient_stops(stop_parts, opacity, parse_color)?);

    // CSS measures a conic gradient's angle from 12 o'clock going clockwise;
    // the shader measures from 3 o'clock, so the start turns back a quarter.
    SweepGradient::new(
        Point::from_xy(centre.0, centre.1),
        0.0,
        360.0,
        stops,
        SpreadMode::Pad,
        Transform::from_rotate_at(start_angle - 90.0, centre.0, centre.1),
    )
}

/// Reads `45deg` or `to bottom right` as a CSS angle.
fn direction_angle(part: &str) -> Option<f32> {
    let part = part.trim();
    if let Some(sides) = part.strip_prefix("to ") {
        let (mut up, mut down, mut left, mut right) = (false, false, false, false);
        for side in sides.split_whitespace() {
            match side {
                "top" => up = true,
                "bottom" => down = true,
                "left" => left = true,
                "right" => right = true,
                _ => return None,
            }
        }

        return match (up, down, left, right) {
            (true, false, false, false) => Some(0.0),
            (false, false, false, true) => Some(90.0),
            (false, true, false, false) => Some(180.0),
            (false, false, true, false) => Some(270.0),
            // A corner points at the corner, which on a square box is the
            // diagonal. CSS tilts it by the box's aspect ratio; this does not,
            // which shows only on a box far from square.
            (true, false, false, true) => Some(45.0),
            (false, true, false, true) => Some(135.0),
            (false, true, true, false) => Some(225.0),
            (true, false, true, false) => Some(315.0),
            // `to top bottom` and friends are not directions.
            _ => None,
        };
    }

    parse_angle(part)
}

fn parse_angle(value: &str) -> Option<f32> {
    let value = value.trim();
    let (number, scale) = if let Some(rest) = value.strip_suffix("deg") {
        (rest, 1.0)
    } else if let Some(rest) = value.strip_suffix("turn") {
        (rest, 360.0)
    } else if let Some(rest) = value.strip_suffix("rad") {
        (rest, 180.0 / std::f32::consts::PI)
    } else {
        return None;
    };

    number.trim().parse::<f32>().ok().map(|it| it * scale)
}

/// Reads `at 50% 30%` as an absolute point inside the box.
fn position_of(part: &str, area: GradientBox) -> Option<(f32, f32)> {
    let coords = part.trim().split_once("at ")?.1;
    let mut parts = coords.split_whitespace();
    let x = fraction(parts.next()?)?;
    // CSS lets the second coordinate be left out, and centres on that axis.
    let y = parts.next().map_or(Some(0.5), fraction)?;

    Some((area.x + area.width * x, area.y + area.height * y))
}

fn fraction(value: &str) -> Option<f32> {
    match value.trim() {
        "left" | "top" => Some(0.0),
        "center" => Some(0.5),
        "right" | "bottom" => Some(1.0),
        value => value
            .strip_suffix('%')
            .and_then(|percentage| percentage.trim().parse::<f32>().ok())
            .map(|percentage| percentage / 100.0),
    }
}

/// Reads the colour stops, spacing any that declare no position evenly.
fn gradient_stops(
    parts: &[String],
    opacity: f32,
    parse_color: &dyn Fn(&str, f32) -> Option<tiny_skia::Color>,
) -> Option<Vec<(f32, tiny_skia::Color)>> {
    let mut parsed: Vec<(Option<f32>, tiny_skia::Color)> = Vec::new();

    for part in parts {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }

        // `#1C1C1E 40%` — the position trails the colour when it is given.
        let (color_text, position) = match part.rsplit_once(char::is_whitespace) {
            Some((color, tail)) if fraction(tail).is_some() => (color.trim(), fraction(tail)),
            _ => (part, None),
        };

        parsed.push((position, parse_color(color_text, opacity)?));
    }

    if parsed.is_empty() {
        return None;
    }

    // A stop without a position sits evenly between its neighbours, which for
    // the common `#a 0%, #b 100%` case changes nothing.
    let last = parsed.len().saturating_sub(1).max(1) as f32;
    Some(
        parsed
            .into_iter()
            .enumerate()
            .map(|(index, (position, color))| (position.unwrap_or(index as f32 / last), color))
            .collect(),
    )
}

/// The stops as tiny-skia wants them.
///
/// Kept apart from [`gradient_stops`] because a `GradientStop`'s position is
/// private, so the placement rules are only testable before this step.
fn shader_stops(stops: Vec<(f32, tiny_skia::Color)>) -> Vec<GradientStop> {
    stops
        .into_iter()
        .map(|(position, color)| GradientStop::new(position, color))
        .collect()
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

/// Splits on commas without breaking `rgba(0, 0, 0, 0.2)` apart.
fn split_top_level(value: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut depth = 0usize;

    for character in value.chars() {
        match character {
            '(' => {
                depth += 1;
                current.push(character);
            }
            ')' => {
                depth = depth.saturating_sub(1);
                current.push(character);
            }
            ',' if depth == 0 => parts.push(std::mem::take(&mut current)),
            _ => current.push(character),
        }
    }
    if !current.trim().is_empty() {
        parts.push(current);
    }

    parts
        .into_iter()
        .map(|part| part.trim().to_owned())
        .filter(|part| !part.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tiny_skia::Color;

    const AREA: GradientBox = GradientBox {
        x: 0.0,
        y: 0.0,
        width: 100.0,
        height: 50.0,
    };

    /// Hex only, which is all the corpus carries and enough to test the shape
    /// of the parse.
    fn colour(value: &str, opacity: f32) -> Option<Color> {
        let hex = value.trim().strip_prefix('#')?;
        let byte = |range: std::ops::Range<usize>| u8::from_str_radix(&hex[range], 16).ok();
        let alpha = if hex.len() == 8 { byte(6..8)? } else { 255 };
        Some(Color::from_rgba8(
            byte(0..2)?,
            byte(2..4)?,
            byte(4..6)?,
            (alpha as f32 * opacity) as u8,
        ))
    }

    fn shader(value: &str) -> Option<Shader<'static>> {
        gradient_shader(value, AREA, 1.0, &colour)
    }

    #[test]
    fn recognises_every_gradient_spelling() {
        assert!(is_gradient("linear-gradient(180deg, #000 0%, #fff 100%)"));
        assert!(is_gradient("radial-gradient(#000, #fff)"));
        assert!(is_gradient("conic-gradient(#000, #fff)"));
        assert!(is_gradient("angular-gradient(#000, #fff)"));
        assert!(!is_gradient("#1C1C1E"));
        assert!(!is_gradient("rgba(0, 0, 0, 0.2)"));
        assert!(!is_gradient("not-a-gradient(1)"));
    }

    #[test]
    fn builds_the_gradients_the_corpus_carries() {
        assert!(shader("linear-gradient(180deg, #2A0F0500 0%, #2A0F05E6 100%)").is_some());
        assert!(shader("linear-gradient(90deg, #5BD98A 0%, #34383E 50%, #E06BD9 100%)").is_some());
    }

    #[test]
    fn builds_radial_and_conic_gradients() {
        assert!(shader("radial-gradient(circle at 50% 30%, #FFFFFF 0%, #000000 100%)").is_some());
        assert!(shader("conic-gradient(from 0deg at 50% 50%, #FF0000 0%, #0000FF 100%)").is_some());
    }

    #[test]
    fn angles_follow_css_with_zero_pointing_up() {
        // 0deg runs bottom-to-top, so the start sits below the centre.
        assert_eq!(direction_angle("0deg"), Some(0.0));
        assert_eq!(direction_angle("to top"), Some(0.0));
        assert_eq!(direction_angle("to right"), Some(90.0));
        assert_eq!(direction_angle("to bottom"), Some(180.0));
        assert_eq!(direction_angle("to left"), Some(270.0));
        assert_eq!(direction_angle("to bottom right"), Some(135.0));
        assert_eq!(direction_angle("0.5turn"), Some(180.0));
        assert_eq!(
            direction_angle("#ff0000"),
            None,
            "a colour is not a direction"
        );
        assert_eq!(direction_angle("to top bottom"), None);
    }

    #[test]
    fn a_gradient_without_a_direction_runs_down_the_box() {
        // No leading angle, so the first part is already a stop.
        assert!(shader("linear-gradient(#000000, #ffffff)").is_some());
    }

    #[test]
    fn stops_without_positions_are_spaced_evenly() {
        let stops = gradient_stops(
            &[
                "#000000".to_owned(),
                "#888888".to_owned(),
                "#ffffff".to_owned(),
            ],
            1.0,
            &colour,
        )
        .expect("stops parse");

        let positions: Vec<f32> = stops.iter().map(|(position, _)| *position).collect();
        assert_eq!(positions, vec![0.0, 0.5, 1.0]);
    }

    #[test]
    fn a_declared_position_wins_over_even_spacing() {
        let stops = gradient_stops(
            &["#000000 0%".to_owned(), "#ffffff 25%".to_owned()],
            1.0,
            &colour,
        )
        .expect("stops parse");

        let positions: Vec<f32> = stops.iter().map(|(position, _)| *position).collect();
        assert_eq!(positions, vec![0.0, 0.25]);
    }

    #[test]
    fn an_unreadable_stop_gives_up_rather_than_guessing() {
        assert!(shader("linear-gradient(180deg, notacolour, #ffffff)").is_none());
        assert!(shader("linear-gradient()").is_none());
    }

    #[test]
    fn a_position_resolves_against_the_box() {
        assert_eq!(position_of("at 50% 50%", AREA), Some((50.0, 25.0)));
        assert_eq!(position_of("at 0% 100%", AREA), Some((0.0, 50.0)));
        assert_eq!(
            position_of("circle at right bottom", AREA),
            Some((100.0, 50.0))
        );
        // CSS lets the second coordinate be left out.
        assert_eq!(position_of("at 25%", AREA), Some((25.0, 25.0)));
    }
}
