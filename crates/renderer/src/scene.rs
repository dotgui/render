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
    pub radius: Option<f32>,
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
        radius: attr(layout, "radius")
            .map(|value| resolve_token(value, metadata))
            .and_then(|value| parse_number(&value)),
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

/// Reads the ordered effect stack out of a node's `<appearance>` block.
fn effects_for(layout: &LayoutBox, metadata: &GuiMetadata) -> Vec<Effect> {
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
