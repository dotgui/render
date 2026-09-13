//! Painting a scene at a pixel density other than 1.
//!
//! Paint works in absolute device pixels, with no transform threaded through
//! it, so a sharper image is not a matter of scaling the canvas. Instead the
//! scene itself is scaled: every length it carries is multiplied, and the
//! painter draws the larger scene exactly as it draws any other. Text is laid
//! out into lines by the painter, and font metrics scale linearly, so a scaled
//! text box breaks where the unscaled one did.
//!
//! Lengths that travel inside strings are scaled too: `blur()` in `filter`, and
//! the basic shapes of `clip-path`. Percentages, factors and angles are not
//! lengths and are left alone. A `path()` clip is SVG path data, which this
//! does not rewrite, so it clips at 1x geometry.

use crate::{
    scene::{Effect, ImageMask, Outline, PaintContent, Scene, SceneNode, TextSegment},
    LayoutRect,
};

impl Scene {
    /// This scene at `factor` device pixels per document pixel.
    pub fn scaled(&self, factor: f32) -> Scene {
        Scene {
            name: self.name.clone(),
            root: scale_node(&self.root, factor),
        }
    }
}

fn scale_node(node: &SceneNode, k: f32) -> SceneNode {
    let mut node = node.clone();
    scale_in_place(&mut node, k);
    node
}

fn scale_in_place(node: &mut SceneNode, k: f32) {
    node.bounds = scale_rect(node.bounds, k);
    node.radius = node.radius.map(|radius| radius * k);

    for border in &mut node.borders {
        border.width *= k;
        border.widths.top *= k;
        border.widths.right *= k;
        border.widths.bottom *= k;
        border.widths.left *= k;
    }
    if let Some(Outline { width, offset, .. }) = &mut node.outline {
        *width *= k;
        *offset *= k;
    }
    for Effect {
        x,
        y,
        radius,
        spread,
        ..
    } in &mut node.effects
    {
        *x *= k;
        *y *= k;
        *radius *= k;
        *spread *= k;
    }
    if let Some(ImageMask {
        x,
        y,
        width,
        height,
        ..
    }) = &mut node.image_mask
    {
        *x *= k;
        *y *= k;
        *width = width.map(|it| it * k);
        *height = height.map(|it| it * k);
    }
    if let Some(transform) = &mut node.transform {
        transform.origin_x *= k;
        transform.origin_y *= k;
    }
    node.filter = node.filter.as_deref().map(|filter| scale_filter(filter, k));
    node.clip_path = node
        .clip_path
        .as_deref()
        .map(|clip| scale_clip_path(clip, k));

    if let PaintContent::Text {
        segments,
        paragraph_indent,
        list_indent,
        ..
    } = &mut node.content
    {
        *paragraph_indent *= k;
        *list_indent *= k;
        segments
            .iter_mut()
            .for_each(|segment| scale_segment(segment, k));
    }

    node.children
        .iter_mut()
        .for_each(|child| scale_in_place(child, k));
}

fn scale_rect(rect: LayoutRect, k: f32) -> LayoutRect {
    LayoutRect {
        x: rect.x * k,
        y: rect.y * k,
        width: rect.width * k,
        height: rect.height * k,
    }
}

fn scale_segment(segment: &mut TextSegment, k: f32) {
    // `opsz` follows the font size, and a face drawn at twice the size is a
    // different optical cut: narrower, so lines would break elsewhere. Pin it
    // to the document's size by naming it, unless the document already did.
    let optical = segment.font_optical_sizing.as_deref() != Some("none");
    let named = segment
        .font_variation
        .as_deref()
        .is_some_and(|variation| variation.contains("opsz"));
    if optical && !named {
        let pin = format!("\"opsz\" {}", segment.font_size);
        segment.font_variation = Some(match segment.font_variation.take() {
            Some(variation) if !variation.trim().is_empty() => format!("{variation}, {pin}"),
            _ => pin,
        });
    }

    segment.font_size *= k;
    segment.line_height = segment.line_height.map(|it| it * k);
    segment.letter_spacing *= k;
    segment.word_spacing *= k;
    segment.baseline_shift *= k;
    if let Some(decoration) = &mut segment.decoration {
        decoration.thickness = decoration.thickness.map(|it| it * k);
        decoration.offset = decoration.offset.map(|it| it * k);
    }
}

/// Scales the argument of each `blur()`; the other filter functions take
/// factors, which a density does not change.
fn scale_filter(filter: &str, k: f32) -> String {
    let mut out = String::with_capacity(filter.len());
    let mut rest = filter;
    while let Some(open) = rest.find('(') {
        let Some(close) = rest[open..].find(')').map(|it| open + it) else {
            break;
        };
        let name = rest[..open].trim().to_ascii_lowercase();
        out.push_str(&rest[..=open]);
        let argument = &rest[open + 1..close];
        if name.ends_with("blur") {
            out.push_str(&scale_lengths(argument, k));
        } else {
            out.push_str(argument);
        }
        out.push(')');
        rest = &rest[close + 1..];
    }
    out.push_str(rest);
    out
}

/// Scales the lengths of a basic shape; `path()` data is left as it is.
fn scale_clip_path(clip: &str, k: f32) -> String {
    if clip.trim_start().starts_with("path") {
        clip.to_owned()
    } else {
        scale_lengths(clip, k)
    }
}

/// Multiplies every number in `value` that is not a percentage.
fn scale_lengths(value: &str, k: f32) -> String {
    let bytes = value.as_bytes();
    let mut out = String::with_capacity(value.len());
    let mut i = 0;
    while i < bytes.len() {
        let starts_number = bytes[i].is_ascii_digit()
            || (bytes[i] == b'.' && bytes.get(i + 1).is_some_and(u8::is_ascii_digit))
            || (bytes[i] == b'-'
                && bytes
                    .get(i + 1)
                    .is_some_and(|next| next.is_ascii_digit() || *next == b'.'));
        let inside_word = i > 0 && (bytes[i - 1].is_ascii_alphabetic() || bytes[i - 1] == b'-');
        if !starts_number || inside_word {
            out.push(bytes[i] as char);
            i += 1;
            continue;
        }

        let start = i;
        i += 1;
        while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'.') {
            i += 1;
        }
        let number = &value[start..i];
        match (bytes.get(i), number.parse::<f32>()) {
            (Some(b'%'), _) | (_, Err(_)) => out.push_str(number),
            (_, Ok(parsed)) => out.push_str(&(parsed * k).to_string()),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lengths_scale_and_percentages_do_not() {
        assert_eq!(
            scale_lengths("inset(10px 5% round 8px)", 2.0),
            "inset(20px 5% round 16px)"
        );
        assert_eq!(
            scale_lengths("circle(20px at 50% 0)", 1.5),
            "circle(30px at 50% 0)"
        );
        assert_eq!(
            scale_lengths("polygon(0 0, 10.5 -4, 100% 100%)", 2.0),
            "polygon(0 0, 21 -8, 100% 100%)"
        );
    }

    #[test]
    fn only_blur_arguments_are_lengths_in_a_filter() {
        assert_eq!(
            scale_filter("blur(4px) brightness(1.2) grayscale(50%)", 2.0),
            "blur(8px) brightness(1.2) grayscale(50%)"
        );
    }

    #[test]
    fn optical_size_stays_at_the_document_size() {
        let mut segment = TextSegment {
            value: "Title".to_owned(),
            font_family: Some("SF Pro".to_owned()),
            font_weight: None,
            font_style: None,
            font_size: 17.0,
            line_height: Some(22.0),
            letter_spacing: 0.5,
            color: None,
            font_stretch: None,
            font_optical_sizing: None,
            font_variation: Some("\"wght\" 600".to_owned()),
            font_smoothing: None,
            word_spacing: 0.0,
            baseline_shift: 0.0,
            decoration: None,
        };
        scale_segment(&mut segment, 2.0);
        assert_eq!(segment.font_size, 34.0);
        assert_eq!(segment.line_height, Some(44.0));
        assert_eq!(
            segment.font_variation.as_deref(),
            Some("\"wght\" 600, \"opsz\" 17")
        );

        let mut fixed = TextSegment {
            font_optical_sizing: Some("none".to_owned()),
            font_variation: None,
            ..segment
        };
        scale_segment(&mut fixed, 2.0);
        assert_eq!(fixed.font_variation, None);
    }

    #[test]
    fn path_clips_are_left_alone() {
        assert_eq!(
            scale_clip_path("path('M 0 0 L 10 10')", 2.0),
            "path('M 0 0 L 10 10')"
        );
    }
}
