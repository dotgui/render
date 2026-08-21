use crate::{
    text_style::{resolve_text_runs, resolve_token},
    GuiDocument, GuiMetadata, LayoutBox, LayoutRect,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Scene {
    pub name: Option<String>,
    pub root: SceneNode,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SceneNode {
    pub tag: String,
    pub bounds: LayoutRect,
    /// The node's fill stack, in document order: the first entry is painted
    /// first and the last one ends up on top.
    ///
    /// A node with a `fill` attribute and no `<appearance>` fills has exactly
    /// one entry.
    pub fills: Vec<Fill>,
    /// The node's border stack, in document order.
    ///
    /// Per the spec, an `<appearance>` carrying at least one `<border>` makes
    /// the `border` shorthand attribute on the node ignored.
    pub borders: Vec<Border>,
    /// A stroke drawn outside the node's box, which never affects layout.
    pub outline: Option<Outline>,
    pub radius: Option<f32>,
    /// Squircle factor for the node's corners, 0 (a circular arc) to 1.
    pub corner_smoothing: f32,
    pub opacity: f32,
    pub clip: bool,
    /// Visual effects from `<appearance>`, in document order.
    pub effects: Vec<Effect>,
    pub content: PaintContent,
    pub children: Vec<SceneNode>,
}

impl SceneNode {
    /// The colour the node paints with, for consumers that need one value:
    /// text runs inheriting their colour, and placeholder shapes.
    ///
    /// The topmost solid fill wins, because that is the one a viewer sees.
    pub fn fill_color(&self) -> Option<&str> {
        self.fills
            .iter()
            .rev()
            .find(|fill| fill.kind == "color")
            .and_then(|fill| fill.value.as_deref())
    }
}

/// A stroke drawn outside the node's box, as in CSS `outline`.
///
/// Unlike a border, an outline sits outside the box entirely and does not take
/// part in layout, so it can overlap its neighbours.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Outline {
    pub width: f32,
    pub color: String,
    pub style: String,
    /// Gap between the node's edge and the outline, as in CSS
    /// `outline-offset`. Negative values pull the outline inwards.
    pub offset: f32,
}

/// One entry of a node's fill stack.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Fill {
    /// `color`, `linear-gradient`, `radial-gradient`, `angular-gradient` or
    /// `image`. Only `color` is painted today; the rest are carried so a
    /// consumer can see them and so the stack keeps its order.
    pub kind: String,
    /// The paint itself: a colour for `type="color"`, the gradient function
    /// for a gradient, unset for an image.
    pub value: Option<String>,
    /// `src` and `fit`, for `type="image"`.
    pub src: Option<String>,
    pub fit: Option<String>,
}

/// One entry of a node's effect stack (RFC-0027).
///
/// The fields mirror CSS `box-shadow` and `backdrop-filter`, because that is
/// what the format's values mean and what the reference renderer emits.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Effect {
    /// `drop-shadow`, `inner-shadow`, `layer-blur`, `background-blur`, `glass`.
    pub kind: String,
    pub x: f32,
    pub y: f32,
    /// Blur radius. Twice the Gaussian sigma, as in CSS.
    pub radius: f32,
    /// Grows the shadow shape before blurring; negative values shrink it.
    pub spread: f32,
    pub color: Option<String>,
    /// Multiplied into the colour's alpha.
    pub opacity: f32,
    /// Backdrop saturation percentage, for `glass`.
    pub saturation: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Border {
    pub width: f32,
    pub widths: BorderWidths,
    pub color: String,
    pub style: String,
    pub align: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct BorderWidths {
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
    pub left: f32,
}

impl BorderWidths {
    pub fn uniform(width: f32) -> Self {
        Self {
            top: width,
            right: width,
            bottom: width,
            left: width,
        }
    }

    pub fn is_uniform(self) -> bool {
        (self.top - self.right).abs() < f32::EPSILON
            && (self.right - self.bottom).abs() < f32::EPSILON
            && (self.bottom - self.left).abs() < f32::EPSILON
    }
}

/// One run of a `<text>` node, with every font property already resolved.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TextSegment {
    pub value: String,
    pub font_family: Option<String>,
    pub font_weight: Option<String>,
    pub font_style: Option<String>,
    pub font_size: f32,
    pub line_height: f32,
    pub letter_spacing: f32,
    pub color: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum PaintContent {
    None,
    Text {
        /// The full string with styling flattened away, for consumers that
        /// only need the words.
        value: String,
        /// The styled runs this text is drawn as. Always at least one; a
        /// `<text>` without `<segment>` children has exactly one.
        segments: Vec<TextSegment>,
        max_lines: Option<usize>,
        truncate: bool,
        text_align: Option<String>,
    },
    Image {
        src: String,
        fit: Option<String>,
    },
}

pub fn build_scene(document: &GuiDocument, layout: &LayoutBox) -> Scene {
    Scene {
        name: document.name.clone(),
        root: build_scene_node(layout, &document.metadata),
    }
}

fn build_scene_node(layout: &LayoutBox, metadata: &GuiMetadata) -> SceneNode {
    SceneNode {
        tag: layout.tag.clone(),
        bounds: layout.rect,
        fills: fills_for(layout, metadata),
        borders: borders_for(layout, metadata),
        outline: outline_for(layout, metadata),
        radius: attr(layout, "radius")
            .map(|value| resolve_token(value, metadata))
            .and_then(|value| parse_number(&value)),
        corner_smoothing: corner_smoothing_for(layout, metadata),
        opacity: attr(layout, "opacity")
            .and_then(parse_number)
            .unwrap_or(1.0),
        clip: attr(layout, "clip").is_some_and(|value| value != "false"),
        effects: effects_for(layout, metadata),
        content: content_for(layout, metadata),
        children: layout
            .children
            .iter()
            .filter(|child| child.tag != "segment" && child.tag != "appearance")
            .map(|child| build_scene_node(child, metadata))
            .collect(),
    }
}

/// The children of a node's `<appearance>` blocks, in document order.
///
/// `visible="false"` keeps an entry in the document without drawing it, so it
/// is dropped here rather than carried into the scene.
fn appearance_children<'a>(
    layout: &'a LayoutBox,
    tag: &'a str,
) -> impl Iterator<Item = &'a LayoutBox> {
    layout
        .children
        .iter()
        .filter(|child| child.tag == "appearance")
        .flat_map(|appearance| appearance.children.iter())
        .filter(move |child| child.tag == tag)
        .filter(|child| attr(child, "visible") != Some("false"))
}

/// Reads the ordered fill stack out of a node's `<appearance>` block, falling
/// back to the `fill` shorthand attribute.
fn fills_for(layout: &LayoutBox, metadata: &GuiMetadata) -> Vec<Fill> {
    let fills: Vec<Fill> = appearance_children(layout, "fill")
        .map(|fill| Fill {
            // The corpus writes `type="color"`; a `<fill>` with a bare `value`
            // and no type is a colour too.
            kind: attr(fill, "type").unwrap_or("color").to_owned(),
            value: attr(fill, "value").map(|value| resolve_token(value, metadata)),
            src: attr(fill, "src").map(ToOwned::to_owned),
            fit: attr(fill, "fit").map(ToOwned::to_owned),
        })
        .collect();

    if !fills.is_empty() {
        return fills;
    }

    attr(layout, "fill")
        .or_else(|| attr(layout, "color"))
        .map(|value| Fill {
            kind: "color".to_owned(),
            value: Some(resolve_token(value, metadata)),
            src: None,
            fit: None,
        })
        .into_iter()
        .collect()
}

/// Reads the ordered border stack out of a node's `<appearance>` block.
///
/// Per the spec, one `<border>` child is enough to make the node's `border`
/// shorthand attribute ignored.
fn borders_for(layout: &LayoutBox, metadata: &GuiMetadata) -> Vec<Border> {
    let borders: Vec<Border> = appearance_children(layout, "border")
        .filter_map(|border| {
            let color = attr(border, "color").map(|value| resolve_token(value, metadata))?;
            let widths = parse_border_widths(
                &attr(border, "w")
                    .map(|value| resolve_token(value, metadata))
                    .unwrap_or_else(|| "1".to_owned())
                    .split_whitespace()
                    .filter_map(parse_number)
                    .collect::<Vec<_>>(),
            )?;

            Some(Border {
                width: widths
                    .top
                    .max(widths.right)
                    .max(widths.bottom)
                    .max(widths.left),
                widths,
                color,
                style: attr(border, "style").unwrap_or("solid").to_owned(),
                align: attr(border, "align").unwrap_or("center").to_owned(),
            })
        })
        .collect();

    if !borders.is_empty() {
        return borders;
    }

    attr(layout, "border")
        .and_then(|value| parse_border(&resolve_token(value, metadata)))
        .into_iter()
        .collect()
}

/// Reads `outline` and `outline-offset`.
///
/// The value grammar is the `border` shorthand's, so it is parsed with the
/// same reader. CSS outlines are uniform, so a sided value collapses to its
/// widest side rather than drawing four different strokes.
fn outline_for(layout: &LayoutBox, metadata: &GuiMetadata) -> Option<Outline> {
    let border = parse_border(&resolve_token(attr(layout, "outline")?, metadata))?;

    Some(Outline {
        width: border.width,
        color: border.color,
        style: border.style,
        offset: attr(layout, "outline-offset")
            .map(|value| resolve_token(value, metadata))
            .and_then(|value| parse_number(&value))
            .unwrap_or(0.0),
    })
}

/// Reads `corner-smoothing`, accepting both `0.6` and `60%`.
///
/// Figma's control is a percentage and the spec types it as a number, so both
/// spellings show up in the wild. Out-of-range values clamp rather than
/// producing a corner that escapes its own box.
fn corner_smoothing_for(layout: &LayoutBox, metadata: &GuiMetadata) -> f32 {
    let Some(raw) = attr(layout, "corner-smoothing").map(|value| resolve_token(value, metadata))
    else {
        return 0.0;
    };

    let value = match raw.trim().strip_suffix('%') {
        Some(percentage) => parse_number(percentage).map(|value| value / 100.0),
        None => parse_number(&raw),
    };

    value.unwrap_or(0.0).clamp(0.0, 1.0)
}

/// The single-shadow shorthand, read as one entry of the effect stack.
///
/// The value is CSS `box-shadow`: `x y blur [spread] color`, with `inset`
/// turning it into an inner shadow. `<appearance>` supersedes it, so the
/// shorthand only applies to a node that declares no `<effect>` of its own.
fn shadow_shorthand(layout: &LayoutBox, metadata: &GuiMetadata) -> Option<Effect> {
    let raw = resolve_token(attr(layout, "shadow")?, metadata);
    if raw.trim().is_empty() || raw.trim() == "none" {
        return None;
    }

    let mut numbers = Vec::new();
    let mut color = None;
    let mut inset = false;
    for part in split_shadow_parts(&raw) {
        match part.as_str() {
            "inset" => inset = true,
            _ if color_like(&part) => color = Some(part),
            _ => {
                if let Some(number) = parse_number(&part) {
                    numbers.push(number);
                }
            }
        }
    }

    // `x` and `y` are the minimum that makes a shadow mean anything; a blur
    // radius and a spread are optional, as in CSS.
    let [x, y, ..] = numbers[..] else {
        return None;
    };

    Some(Effect {
        kind: if inset { "inner-shadow" } else { "drop-shadow" }.to_owned(),
        x,
        y,
        radius: numbers.get(2).copied().unwrap_or(0.0),
        spread: numbers.get(3).copied().unwrap_or(0.0),
        color,
        opacity: 1.0,
        saturation: 180.0,
    })
}

/// Splits a shadow value on whitespace without breaking `rgba(0, 0, 0, 0.2)`
/// apart at its commas and spaces.
fn split_shadow_parts(value: &str) -> Vec<String> {
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
            _ if character.is_whitespace() && depth == 0 => {
                if !current.is_empty() {
                    parts.push(std::mem::take(&mut current));
                }
            }
            _ => current.push(character),
        }
    }
    if !current.is_empty() {
        parts.push(current);
    }

    parts
}

/// Reads the ordered effect stack out of a node's `<appearance>` block,
/// falling back to the `shadow` shorthand.
fn effects_for(layout: &LayoutBox, metadata: &GuiMetadata) -> Vec<Effect> {
    let effects: Vec<Effect> = appearance_effects(layout, metadata);

    if !effects.is_empty() {
        return effects;
    }

    shadow_shorthand(layout, metadata).into_iter().collect()
}

fn appearance_effects(layout: &LayoutBox, metadata: &GuiMetadata) -> Vec<Effect> {
    appearance_children(layout, "effect")
        .filter_map(|effect| {
            let number = |name: &str, fallback: f32| {
                attr(effect, name)
                    .map(|value| resolve_token(value, metadata))
                    .and_then(|value| parse_number(&value))
                    .unwrap_or(fallback)
            };

            Some(Effect {
                kind: attr(effect, "type")?.to_owned(),
                x: number("x", 0.0),
                y: number("y", 0.0),
                radius: number("radius", 0.0),
                spread: number("spread", 0.0),
                color: attr(effect, "color").map(|value| resolve_token(value, metadata)),
                opacity: number("opacity", 1.0),
                saturation: number("saturation", 180.0),
            })
        })
        .collect()
}

fn content_for(layout: &LayoutBox, metadata: &GuiMetadata) -> PaintContent {
    match layout.tag.as_str() {
        "text" => {
            let runs = resolve_text_runs(layout, metadata);
            let value: String = runs.iter().map(|run| run.value.as_str()).collect();
            let segments = runs
                .into_iter()
                .map(|run| TextSegment {
                    value: run.value,
                    font_family: run.style.font_family,
                    font_weight: run.style.font_weight,
                    font_style: run.style.font_style,
                    font_size: run.style.font_size,
                    line_height: run.style.line_height,
                    letter_spacing: run.style.letter_spacing,
                    color: run.style.color,
                })
                .collect();

            PaintContent::Text {
                value,
                segments,
                max_lines: max_text_lines(layout),
                truncate: truncates(layout),
                text_align: attr(layout, "align").map(ToOwned::to_owned),
            }
        }
        "img" => layout
            .attributes
            .get("src")
            .cloned()
            .map(|src| PaintContent::Image {
                src,
                fit: layout.attributes.get("fit").cloned(),
            })
            .unwrap_or(PaintContent::None),
        _ => PaintContent::None,
    }
}

fn parse_border(value: &str) -> Option<Border> {
    let mut numbers = Vec::new();
    let mut color = None;
    let mut style = "solid".to_owned();
    let mut align = "center".to_owned();

    for part in value.split_whitespace() {
        match part {
            "inside" | "outside" | "center" => align = part.to_owned(),
            "solid" | "dashed" | "dotted" => style = part.to_owned(),
            _ if color_like(part) => color = Some(part.to_owned()),
            _ => {
                if let Some(number) = parse_number(part) {
                    numbers.push(number);
                }
            }
        }
    }

    let widths = parse_border_widths(&numbers)?;
    let width = widths
        .top
        .max(widths.right)
        .max(widths.bottom)
        .max(widths.left);

    Some(Border {
        width,
        widths,
        color: color?,
        style,
        align,
    })
}

fn parse_border_widths(numbers: &[f32]) -> Option<BorderWidths> {
    match numbers {
        [] => None,
        [all] => Some(BorderWidths::uniform(*all)),
        [vertical, horizontal] => Some(BorderWidths {
            top: *vertical,
            right: *horizontal,
            bottom: *vertical,
            left: *horizontal,
        }),
        [top, horizontal, bottom] => Some(BorderWidths {
            top: *top,
            right: *horizontal,
            bottom: *bottom,
            left: *horizontal,
        }),
        [top, right, bottom, left, ..] => Some(BorderWidths {
            top: *top,
            right: *right,
            bottom: *bottom,
            left: *left,
        }),
    }
}

fn color_like(value: &str) -> bool {
    value.starts_with('#')
        || value.starts_with("rgb(")
        || value.starts_with("rgba(")
        || value.starts_with("oklch(")
        || value == "none"
}

fn attr<'a>(layout: &'a LayoutBox, name: &str) -> Option<&'a str> {
    layout.attributes.get(name).map(String::as_str)
}

fn parse_number(value: &str) -> Option<f32> {
    value.trim().trim_end_matches("px").parse::<f32>().ok()
}

/// Resolves how many lines a `<text>` node may occupy.
///
/// Kept in step with `crate::taffy_layout`, which reserves height for exactly
/// this many lines: an explicit `max-lines` wins over `truncate`, which on its
/// own means a single ellipsized line.
fn max_text_lines(layout: &LayoutBox) -> Option<usize> {
    let explicit = attr(layout, "max-lines")
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|lines| *lines > 0);
    if explicit.is_some() {
        return explicit;
    }

    truncates(layout).then_some(1)
}

fn truncates(layout: &LayoutBox) -> bool {
    attr(layout, "truncate").is_some_and(|value| value != "false")
        || attr(layout, "overflow").is_some_and(|value| value == "ellipsis")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{compute_taffy_layout, parse_gui_xml};

    fn scene_of(xml: &str) -> Scene {
        let document = parse_gui_xml(xml).expect("valid gui");
        let layout = compute_taffy_layout(&document).expect("layout computes");
        build_scene(&document, &layout)
    }

    #[test]
    fn builds_paintable_scene_from_layout() {
        let document = parse_gui_xml(
            r##"
            <gui version="0.2" name="Scene Smoke">
              <tokens>
                <color name="surface" value="#ffffff" />
                <color name="rule" value="#dddddd" />
              </tokens>
              <col w="200" fill="$surface" border="1 $rule" radius="8" clip>
                <text value="Hello" />
                <img src="assets/icon.svg" w="16" h="16" />
              </col>
            </gui>
            "##,
        )
        .expect("valid gui");
        let layout = compute_taffy_layout(&document).expect("layout computes");
        let scene = build_scene(&document, &layout);

        assert_eq!(scene.name.as_deref(), Some("Scene Smoke"));
        assert_eq!(scene.root.fill_color(), Some("#ffffff"));
        assert_eq!(scene.root.borders[0].color, "#dddddd");
        assert_eq!(scene.root.borders[0].widths, BorderWidths::uniform(1.0));
        assert_eq!(scene.root.radius, Some(8.0));
        assert!(scene.root.clip);
        assert_eq!(
            scene.root.children[0].content,
            PaintContent::Text {
                value: "Hello".to_owned(),
                segments: vec![TextSegment {
                    value: "Hello".to_owned(),
                    font_family: None,
                    font_weight: None,
                    font_style: None,
                    font_size: 16.0,
                    line_height: 19.2,
                    letter_spacing: 0.0,
                    color: None,
                }],
                max_lines: None,
                truncate: false,
                text_align: None,
            }
        );
        assert_eq!(
            scene.root.children[1].content,
            PaintContent::Image {
                src: "assets/icon.svg".to_owned(),
                fit: None,
            }
        );
    }

    #[test]
    fn scene_text_preserves_font_resolution_hints() {
        let document = parse_gui_xml(
            r#"
            <gui version="0.2" name="Font Scene">
              <fonts>
                <font family="Roboto" source="google" weights="400 500 700" styles="normal" />
              </fonts>
              <styles>
                <text-style name="Title" font-family="Roboto" font-size="22" font-weight="500" line-height="28" />
              </styles>
              <col w="200">
                <text text-style="Title" value="Roboto title" />
              </col>
            </gui>
            "#,
        )
        .expect("valid gui");
        let layout = compute_taffy_layout(&document).expect("layout computes");
        let scene = build_scene(&document, &layout);

        assert_eq!(
            scene.root.children[0].content,
            PaintContent::Text {
                value: "Roboto title".to_owned(),
                segments: vec![TextSegment {
                    value: "Roboto title".to_owned(),
                    font_family: Some("Roboto".to_owned()),
                    font_weight: Some("500".to_owned()),
                    font_style: None,
                    font_size: 22.0,
                    line_height: 28.0,
                    letter_spacing: 0.0,
                    color: None,
                }],
                max_lines: None,
                truncate: false,
                text_align: None,
            }
        );
    }

    #[test]
    fn scene_text_preserves_truncation_hints() {
        let document = parse_gui_xml(
            r#"
            <gui version="0.2" name="Truncate Scene">
              <styles>
                <text-style name="Body" font-size="14" line-height="20" />
              </styles>
              <col w="120">
                <text text-style="Body" value="Long message preview" w="120" max-lines="1" truncate align="right" />
              </col>
            </gui>
            "#,
        )
        .expect("valid gui");
        let layout = compute_taffy_layout(&document).expect("layout computes");
        let scene = build_scene(&document, &layout);

        assert_eq!(
            scene.root.children[0].content,
            PaintContent::Text {
                value: "Long message preview".to_owned(),
                segments: vec![TextSegment {
                    value: "Long message preview".to_owned(),
                    font_family: None,
                    font_weight: None,
                    font_style: None,
                    font_size: 14.0,
                    line_height: 20.0,
                    letter_spacing: 0.0,
                    color: None,
                }],
                max_lines: Some(1),
                truncate: true,
                text_align: Some("right".to_owned()),
            }
        );
    }

    #[test]
    fn scene_carries_segments_without_making_them_nodes() {
        let document = parse_gui_xml(
            r##"
            <gui version="0.2">
              <tokens><color name="accent" value="#7ee2b8" /></tokens>
              <col w="200">
                <text font-size="14" fill="#111111">
                  <segment value="Total " />
                  <segment value="$12" font-weight="700" fill="$accent" />
                </text>
              </col>
            </gui>
            "##,
        )
        .expect("valid gui");
        let layout = compute_taffy_layout(&document).expect("layout computes");
        let scene = build_scene(&document, &layout);

        let text = &scene.root.children[0];
        assert!(
            text.children.is_empty(),
            "segments are text content, not paintable nodes"
        );

        let PaintContent::Text {
            value, segments, ..
        } = &text.content
        else {
            panic!("expected text content, got {:?}", text.content);
        };

        assert_eq!(value, "Total $12");
        assert_eq!(segments.len(), 2);
        assert_eq!(segments[0].color.as_deref(), Some("#111111"));
        assert_eq!(segments[0].font_weight, None);
        assert_eq!(segments[1].color.as_deref(), Some("#7ee2b8"));
        assert_eq!(segments[1].font_weight.as_deref(), Some("700"));
        // Inherited from the parent rather than defaulted.
        assert_eq!(segments[1].font_size, 14.0);
    }

    #[test]
    fn scene_uses_color_as_fill_alias() {
        let document = parse_gui_xml(
            r##"
            <gui version="0.2">
              <tokens>
                <color name="ink" value="#17211B" />
              </tokens>
              <col w="120">
                <text color="$ink" value="Visible text" />
              </col>
            </gui>
            "##,
        )
        .expect("valid gui");
        let layout = compute_taffy_layout(&document).expect("layout computes");
        let scene = build_scene(&document, &layout);

        assert_eq!(scene.root.children[0].fill_color(), Some("#17211B"));
    }

    #[test]
    fn appearance_fill_and_border_reach_the_scene() {
        let scene = scene_of(
            r##"
            <gui version="0.2">
              <col w="100" h="50" radius="12">
                <appearance>
                  <fill type="color" value="#17211B" />
                  <border color="#E0CFC4" w="1" align="inside" />
                </appearance>
              </col>
            </gui>
            "##,
        );

        assert_eq!(scene.root.fill_color(), Some("#17211B"));
        let border = &scene.root.borders[0];
        assert_eq!(border.color, "#E0CFC4");
        assert_eq!(border.widths, BorderWidths::uniform(1.0));
        assert_eq!(border.align, "inside");
    }

    #[test]
    fn appearance_fills_and_borders_stack_in_document_order() {
        let scene = scene_of(
            r##"
            <gui version="0.2">
              <col w="100" h="50">
                <appearance>
                  <fill type="color" value="#000000" />
                  <fill type="linear-gradient" value="linear-gradient(180deg, #FFF 0%, #000 100%)" />
                  <border color="#111111" w="1" />
                  <border color="#222222" w="4" align="outside" />
                </appearance>
              </col>
            </gui>
            "##,
        );

        let values: Vec<_> = scene
            .root
            .fills
            .iter()
            .map(|fill| (fill.kind.as_str(), fill.value.as_deref()))
            .collect();
        assert_eq!(
            values,
            vec![
                ("color", Some("#000000")),
                (
                    "linear-gradient",
                    Some("linear-gradient(180deg, #FFF 0%, #000 100%)")
                ),
            ]
        );

        let colors: Vec<_> = scene
            .root
            .borders
            .iter()
            .map(|border| border.color.as_str())
            .collect();
        assert_eq!(colors, vec!["#111111", "#222222"]);
    }

    #[test]
    fn an_appearance_border_beats_the_border_shorthand() {
        let scene = scene_of(
            r##"
            <gui version="0.2">
              <col w="100" h="50" fill="#ffffff" border="2 #dddddd">
                <appearance>
                  <border color="#E0CFC4" w="1" align="inside" />
                </appearance>
              </col>
            </gui>
            "##,
        );

        assert_eq!(scene.root.borders.len(), 1);
        assert_eq!(scene.root.borders[0].color, "#E0CFC4");
        // The node keeps its `fill` shorthand: only borders are overridden.
        assert_eq!(scene.root.fill_color(), Some("#ffffff"));
    }

    #[test]
    fn invisible_appearance_fills_and_borders_are_dropped() {
        let scene = scene_of(
            r##"
            <gui version="0.2">
              <col w="100" h="50">
                <appearance>
                  <fill type="color" value="#000000" visible="false" />
                  <border color="#111111" w="1" visible="false" />
                </appearance>
              </col>
            </gui>
            "##,
        );

        assert!(scene.root.fills.is_empty());
        assert!(scene.root.borders.is_empty());
    }

    #[test]
    fn appearance_fill_and_border_resolve_tokens() {
        let scene = scene_of(
            r##"
            <gui version="0.2">
              <tokens>
                <token name="card" value="#FFFFFF" />
                <token name="outline-variant" value="#E0CFC4" />
              </tokens>
              <col w="100" h="50">
                <appearance>
                  <fill type="color" value="$card" />
                  <border color="$outline-variant" w="1" align="inside" />
                </appearance>
              </col>
            </gui>
            "##,
        );

        assert_eq!(scene.root.fill_color(), Some("#FFFFFF"));
        assert_eq!(scene.root.borders[0].color, "#E0CFC4");
    }

    #[test]
    fn outline_and_offset_reach_the_scene() {
        let scene = scene_of(
            r##"
            <gui version="0.2">
              <col w="100" h="50" outline="2 #0d99ff" outline-offset="4" />
            </gui>
            "##,
        );

        let outline = scene.root.outline.as_ref().expect("outline is read");
        assert_eq!(outline.width, 2.0);
        assert_eq!(outline.color, "#0d99ff");
        assert_eq!(outline.style, "solid");
        assert_eq!(outline.offset, 4.0);
    }

    #[test]
    fn an_outline_without_an_offset_sits_on_the_edge() {
        let scene = scene_of(
            r##"
            <gui version="0.2">
              <col w="100" h="50" outline="1 dashed #333333" />
            </gui>
            "##,
        );

        let outline = scene.root.outline.as_ref().expect("outline is read");
        assert_eq!(outline.offset, 0.0);
        assert_eq!(outline.style, "dashed");
    }

    #[test]
    fn corner_smoothing_accepts_a_number_or_a_percentage() {
        for value in ["0.6", "60%"] {
            let scene = scene_of(&format!(
                r##"
                <gui version="0.2">
                  <col w="100" h="50" radius="12" corner-smoothing="{value}" />
                </gui>
                "##
            ));

            assert!(
                (scene.root.corner_smoothing - 0.6).abs() < 1e-6,
                "{value} read as {}",
                scene.root.corner_smoothing
            );
        }
    }

    #[test]
    fn corner_smoothing_clamps_and_defaults_to_none() {
        let scene = scene_of(
            r##"
            <gui version="0.2">
              <col w="100" h="50" radius="12">
                <rect w="10" h="10" corner-smoothing="4" />
                <rect w="10" h="10" corner-smoothing="-1" />
              </col>
            </gui>
            "##,
        );

        assert_eq!(scene.root.corner_smoothing, 0.0);
        assert_eq!(scene.root.children[0].corner_smoothing, 1.0);
        assert_eq!(scene.root.children[1].corner_smoothing, 0.0);
    }

    #[test]
    fn the_shadow_shorthand_becomes_one_effect() {
        let scene = scene_of(
            r##"
            <gui version="0.2">
              <col w="100" h="50" shadow="0 2 6 -1 #0000001F" />
            </gui>
            "##,
        );

        let effect = &scene.root.effects[0];
        assert_eq!(effect.kind, "drop-shadow");
        assert_eq!((effect.x, effect.y), (0.0, 2.0));
        assert_eq!(effect.radius, 6.0);
        assert_eq!(effect.spread, -1.0);
        assert_eq!(effect.color.as_deref(), Some("#0000001F"));
    }

    #[test]
    fn an_inset_shadow_shorthand_becomes_an_inner_shadow() {
        let scene = scene_of(
            r##"
            <gui version="0.2">
              <col w="100" h="50" shadow="inset 0 1 2 #00000033" />
            </gui>
            "##,
        );

        assert_eq!(scene.root.effects[0].kind, "inner-shadow");
        assert_eq!(scene.root.effects[0].radius, 2.0);
    }

    #[test]
    fn a_shadow_colour_keeps_its_own_commas_and_spaces() {
        let scene = scene_of(
            r##"
            <gui version="0.2">
              <col w="100" h="50" shadow="0 2 6 rgba(0, 0, 0, 0.2)" />
            </gui>
            "##,
        );

        assert_eq!(scene.root.effects.len(), 1);
        assert_eq!(
            scene.root.effects[0].color.as_deref(),
            Some("rgba(0, 0, 0, 0.2)")
        );
        assert_eq!(scene.root.effects[0].radius, 6.0);
    }

    #[test]
    fn an_appearance_effect_beats_the_shadow_shorthand() {
        let scene = scene_of(
            r##"
            <gui version="0.2">
              <col w="100" h="50" shadow="0 2 6 #0000001F">
                <appearance>
                  <effect type="drop-shadow" x="0" y="8" radius="24" color="#00000033" />
                </appearance>
              </col>
            </gui>
            "##,
        );

        assert_eq!(scene.root.effects.len(), 1);
        assert_eq!(scene.root.effects[0].y, 8.0);
    }

    #[test]
    fn scene_preserves_sided_border_widths() {
        let document = parse_gui_xml(
            r##"
            <gui version="0.2">
              <col w="100" h="50" border="1 0 0 0 #333333" />
            </gui>
            "##,
        )
        .expect("valid gui");
        let layout = compute_taffy_layout(&document).expect("layout computes");
        let scene = build_scene(&document, &layout);

        assert_eq!(
            scene.root.borders[0].widths,
            BorderWidths {
                top: 1.0,
                right: 0.0,
                bottom: 0.0,
                left: 0.0,
            }
        );
    }
}
