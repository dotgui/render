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
    pub fill: Option<String>,
    pub border: Option<Border>,
    pub radius: Option<f32>,
    pub opacity: f32,
    pub clip: bool,
    pub content: PaintContent,
    pub children: Vec<SceneNode>,
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
        fill: attr(layout, "fill")
            .or_else(|| attr(layout, "color"))
            .map(|value| resolve_token(value, metadata)),
        border: attr(layout, "border")
            .and_then(|value| parse_border(&resolve_token(value, metadata))),
        radius: attr(layout, "radius")
            .map(|value| resolve_token(value, metadata))
            .and_then(|value| parse_number(&value)),
        opacity: attr(layout, "opacity")
            .and_then(parse_number)
            .unwrap_or(1.0),
        clip: attr(layout, "clip").is_some_and(|value| value != "false"),
        content: content_for(layout, metadata),
        children: layout
            .children
            .iter()
            .filter(|child| child.tag != "segment")
            .map(|child| build_scene_node(child, metadata))
            .collect(),
    }
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
        assert_eq!(scene.root.fill.as_deref(), Some("#ffffff"));
        assert_eq!(scene.root.border.as_ref().unwrap().color, "#dddddd");
        assert_eq!(
            scene.root.border.as_ref().unwrap().widths,
            BorderWidths::uniform(1.0)
        );
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

        assert_eq!(scene.root.children[0].fill.as_deref(), Some("#17211B"));
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
            scene.root.border.as_ref().unwrap().widths,
            BorderWidths {
                top: 1.0,
                right: 0.0,
                bottom: 0.0,
                left: 0.0,
            }
        );
    }
}
