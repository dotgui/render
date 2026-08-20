//! Grid translation, per RFC-0032.
//!
//! `<grid>` has three shapes, chosen by which attributes are present:
//!
//! - **unit grid** (`unit="8"`) — a snapped coordinate space of fixed square
//!   tracks, `w / unit` by `h / unit`. For freely placed, overlapping elements.
//! - **track grid** (`cols` / `rows`) — explicit track sizes.
//! - **auto flow** (`columns="N"`, the legacy alias) — N equal columns.
//!
//! Children place themselves with `gc` / `gr` (grid-column / grid-row) plus
//! `col-span` / `row-span`, in every mode.
//!
//! The reference HTML renderer (`@dotgui/kit`) lowers all of this to CSS grid,
//! so the job here is to reach the same grid geometry through Taffy rather than
//! to invent behaviour.

use crate::{text_style::resolve_token, GuiMetadata, GuiNode};
use taffy::prelude::*;

/// Which grid shape a `<grid>` element declares.
pub(crate) enum GridMode {
    /// `unit="8"` — a coordinate space of fixed square tracks.
    Unit(f32),
    /// `cols` / `rows` — explicit track templates.
    Track,
    /// `columns="N"` — N equal columns.
    AutoFlow,
}

/// Reads the grid mode of a node, or `None` if it is not a grid.
///
/// `unit` wins when both are present. RFC-0032 calls that combination a
/// validation error, and the reference renderer resolves it the same way, so
/// this warns rather than changing the geometry.
pub(crate) fn grid_mode(node: &GuiNode, metadata: &GuiMetadata) -> Option<GridMode> {
    if node.tag != "grid" {
        return None;
    }

    let unit = attr(node, metadata, "unit").and_then(|value| value.trim().parse::<f32>().ok());
    let has_tracks =
        attr(node, metadata, "cols").is_some() || attr(node, metadata, "rows").is_some();

    match (unit, has_tracks) {
        (Some(unit), true) if unit > 0.0 => {
            eprintln!(
                "warning: <grid> declares both `unit` and `cols`/`rows`; using unit grid (RFC-0032 \
                 treats this as a validation error)"
            );
            Some(GridMode::Unit(unit))
        }
        (Some(unit), false) if unit > 0.0 => Some(GridMode::Unit(unit)),
        (_, true) => Some(GridMode::Track),
        _ => Some(GridMode::AutoFlow),
    }
}

/// Applies the container half of the grid: track templates.
pub(crate) fn apply_container(
    style: &mut Style,
    node: &GuiNode,
    metadata: &GuiMetadata,
    mode: &GridMode,
) {
    match mode {
        GridMode::Unit(unit) => {
            // The canvas size divided by the unit gives the coordinate space.
            // Without a size there is nothing to divide, so no tracks are set
            // and children fall back to auto placement.
            if let Some(width) = number(node, metadata, "w") {
                style.grid_template_columns = fixed_tracks(width, *unit);
            }
            if let Some(height) = number(node, metadata, "h") {
                style.grid_template_rows = fixed_tracks(height, *unit);
            }
        }
        GridMode::Track => {
            if let Some(cols) = attr(node, metadata, "cols") {
                style.grid_template_columns = parse_track_template(&cols);
            }
            if let Some(rows) = attr(node, metadata, "rows") {
                style.grid_template_rows = parse_track_template(&rows);
            }
        }
        GridMode::AutoFlow => {
            // `columns` is the legacy spelling; `cols` is accepted as an alias
            // for it even without any other track attribute.
            if let Some(columns) = attr(node, metadata, "columns")
                .or_else(|| attr(node, metadata, "cols"))
                .and_then(|value| value.trim().parse::<u16>().ok())
            {
                style.grid_template_columns = evenly_sized_tracks(columns);
            }
        }
    }
}

/// `repeat(count, <size>px)` covering `total` at `unit` per track.
fn fixed_tracks(total: f32, unit: f32) -> Vec<GridTemplateComponent<String>> {
    let count = (total / unit).round().max(0.0) as u16;
    if count == 0 {
        return Vec::new();
    }
    vec![repeat(count, vec![length(unit)])]
}

/// Converts a `cols` / `rows` string to track sizes.
///
/// ```text
/// "3"         → repeat(3, 1fr)
/// "240 1fr"   → 240px 1fr
/// "auto 1fr"  → auto 1fr
/// "fill 200"  → repeat(auto-fill, minmax(200px, 1fr))
/// ```
///
/// A bare integer means different things by position: alone it is a track
/// *count*, inside a list it is a pixel *size*.
fn parse_track_template(value: &str) -> Vec<GridTemplateComponent<String>> {
    let trimmed = value.trim();

    if let Ok(count) = trimmed.parse::<u16>() {
        return evenly_sized_tracks(count);
    }

    if let Some(min_size) = trimmed.strip_prefix("fill ") {
        let min = parse_track_size(min_size.trim())
            .unwrap_or_else(|| length(200.0))
            .max;
        return vec![repeat(
            taffy::style::RepetitionCount::AutoFill,
            vec![minmax(min_to_min(min), fr(1.0))],
        )];
    }

    trimmed
        .split_whitespace()
        .filter_map(parse_track_size)
        .map(GridTemplateComponent::Single)
        .collect()
}

/// One track size: `240` (px), `1fr`, `auto`, `50%`.
fn parse_track_size(value: &str) -> Option<TrackSizingFunction> {
    if value == "auto" {
        return Some(auto());
    }
    if let Some(count) = value.strip_suffix("fr") {
        return count.trim().parse::<f32>().ok().map(fr);
    }
    if let Some(percentage) = value.strip_suffix('%') {
        return percentage
            .trim()
            .parse::<f32>()
            .ok()
            .map(|value| percent(value / 100.0));
    }
    value.trim_end_matches("px").parse::<f32>().ok().map(length)
}

/// Reuses a track's max sizing function as a minimum, for `minmax(x, 1fr)`.
fn min_to_min(max: MaxTrackSizingFunction) -> MinTrackSizingFunction {
    MinTrackSizingFunction::from(max)
}

/// Applies the child half of the grid: `gc` / `gr` / `col-span` / `row-span`.
///
/// Returns whether the child should fill its span, which happens when a range
/// is given and the matching `w` / `h` is absent.
pub(crate) fn apply_placement(style: &mut Style, node: &GuiNode, metadata: &GuiMetadata) {
    let gc = attr(node, metadata, "gc");
    let gr = attr(node, metadata, "gr");
    let col_span = attr(node, metadata, "col-span");
    let row_span = attr(node, metadata, "row-span");

    if let Some(placement) = placement_for(gc.as_deref(), col_span.as_deref()) {
        style.grid_column = placement;
    }
    if let Some(placement) = placement_for(gr.as_deref(), row_span.as_deref()) {
        style.grid_row = placement;
    }

    // Fill rule: a range sizes the child, unless an explicit pixel size wins.
    if is_range(gc.as_deref()) && !node.attributes.contains_key("w") {
        style.size.width = percent(1.0);
    }
    if is_range(gr.as_deref()) && !node.attributes.contains_key("h") {
        style.size.height = percent(1.0);
    }
}

fn is_range(value: Option<&str>) -> bool {
    value.is_some_and(|value| value.contains('/'))
}

fn placement_for(position: Option<&str>, span: Option<&str>) -> Option<Line<GridPlacement>> {
    match (position, span) {
        (Some(position), _) if position.contains('/') => Some(parse_inclusive_range(position)),
        (Some(position), Some(span)) => {
            let start = grid_line(position)?;
            Some(Line {
                start,
                end: span_end(span),
            })
        }
        (Some(position), None) => Some(Line {
            start: grid_line(position)?,
            end: GridPlacement::Auto,
        }),
        (None, Some(span)) => Some(match span.trim() {
            "all" => Line {
                start: line(1),
                end: line(-1),
            },
            span => Line {
                start: GridPlacement::Auto,
                end: span_end(span),
            },
        }),
        (None, None) => None,
    }
}

/// `col-span="all"` means "to the last line"; anything else is a track count.
///
/// The reference renderer emits `span -1` for `all` combined with a start
/// position, which is not valid CSS and is dropped by the browser. This follows
/// the RFC's stated intent instead.
fn span_end(value: &str) -> GridPlacement {
    match value.trim() {
        "all" => line(-1),
        count => count
            .parse::<u16>()
            .map(span)
            .unwrap_or(GridPlacement::Auto),
    }
}

/// Converts an inclusive `gc` / `gr` range to grid lines.
///
/// `"2/5"` is columns 2 through 5, which is CSS `2 / 6` — line numbers name the
/// gaps between tracks, so the end line is one past the last track. Negative
/// indices count back from the end and pass through unchanged: `-1` is already
/// the final line.
fn parse_inclusive_range(value: &str) -> Line<GridPlacement> {
    let Some((start, end)) = value.split_once('/') else {
        return Line {
            start: grid_line(value).unwrap_or(GridPlacement::Auto),
            end: GridPlacement::Auto,
        };
    };

    let start_line = grid_line(start).unwrap_or(GridPlacement::Auto);
    let end_line = match end.trim().parse::<i16>() {
        Ok(index) if index >= 0 => line(index + 1),
        Ok(index) => line(index),
        Err(_) => GridPlacement::Auto,
    };

    Line {
        start: start_line,
        end: end_line,
    }
}

fn grid_line(value: &str) -> Option<GridPlacement> {
    value.trim().parse::<i16>().ok().map(line)
}

fn attr(node: &GuiNode, metadata: &GuiMetadata, name: &str) -> Option<String> {
    node.attributes
        .get(name)
        .map(|value| resolve_token(value, metadata))
}

fn number(node: &GuiNode, metadata: &GuiMetadata, name: &str) -> Option<f32> {
    attr(node, metadata, name).and_then(|value| value.trim_end_matches("px").parse::<f32>().ok())
}
