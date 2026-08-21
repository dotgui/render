//! Geometry produced by layout, plus the text-measurement seam.
//!
//! The layout algorithm itself lives in `crate::taffy_layout`; this module
//! only owns the types both layout and painting agree on.

use crate::fonts::FontAxes;

/// Where a Latin capital lands, as a fraction of the font size, for faces that
/// do not declare a `capHeight`.
pub(crate) const CAP_HEIGHT_RATIO: f32 = 0.72;

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LayoutRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LayoutBox {
    pub tag: String,
    pub attributes: BTreeMap<String, String>,
    /// Element body text, as distinct from a `value` attribute.
    pub text: Option<String>,
    pub rect: LayoutRect,
    pub children: Vec<LayoutBox>,
}

/// Measures the painted width of a string.
///
/// Implemented by [`ApproxTextMeasurer`] for callers with no fonts loaded and
/// by [`FontStore`](crate::FontStore) for real font metrics.
pub trait TextMeasurer {
    fn text_width(
        &self,
        value: &str,
        font_family: Option<&str>,
        font_weight: Option<&str>,
        font_style: Option<&str>,
        font_size: f32,
        axes: FontAxes,
    ) -> f32;

    /// How much `leading-trim` takes off the top of a block: the distance
    /// from the line box's top edge down to the cap height.
    ///
    /// This is one number rather than the ascender and cap height separately,
    /// so layout and painting cannot combine the same parts differently and
    /// size a box to an edge they then draw to somewhere else.
    fn leading_trim(
        &self,
        font_family: Option<&str>,
        font_weight: Option<&str>,
        font_style: Option<&str>,
        font_size: f32,
        line_height: f32,
    ) -> f32;
}

/// Estimates width as a fixed fraction of the font size per character.
///
/// Good enough for structural tests; it will not match painted output.
#[derive(Debug, Clone, Copy, Default)]
pub struct ApproxTextMeasurer;

impl TextMeasurer for ApproxTextMeasurer {
    fn text_width(
        &self,
        value: &str,
        _font_family: Option<&str>,
        _font_weight: Option<&str>,
        _font_style: Option<&str>,
        font_size: f32,
        _axes: FontAxes,
    ) -> f32 {
        value.chars().count() as f32 * font_size * 0.55
    }

    fn leading_trim(
        &self,
        _font_family: Option<&str>,
        _font_weight: Option<&str>,
        _font_style: Option<&str>,
        font_size: f32,
        line_height: f32,
    ) -> f32 {
        ((line_height - font_size) / 2.0 + font_size - font_size * CAP_HEIGHT_RATIO).max(0.0)
    }
}
