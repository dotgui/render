use crate::{GuiDocument, GuiMetadata, LayoutBox, LayoutRect};
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum PaintContent {
    None,
    Text {
        value: String,
        font_family: Option<String>,
        font_source: Option<String>,
        font_weight: Option<String>,
        font_style: Option<String>,
        font_size: f32,
        line_height: f32,
        can_wrap: bool,
        max_lines: Option<usize>,
        truncate: bool,
        text_align: Option<String>,
        letter_spacing: f32,
    },
    Image {
        src: String,
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
            .map(|child| build_scene_node(child, metadata))
            .collect(),
    }
}

fn content_for(layout: &LayoutBox, metadata: &GuiMetadata) -> PaintContent {
    match layout.tag.as_str() {
        "text" => layout
            .attributes
            .get("value")
            .cloned()
            .map(|value| {
                let font_size = text_style_number(layout, metadata, "font-size")
                    .or_else(|| text_style_number(layout, metadata, "size"))
                    .unwrap_or(16.0);
                let line_height =
                    text_style_number(layout, metadata, "line-height").unwrap_or(font_size * 1.2);
                let font_family = text_style_value(layout, metadata, "font-family");
                let font_weight = text_style_value(layout, metadata, "font-weight");
                let font_style = text_style_value(layout, metadata, "font-style");
                let font_source = font_family
                    .as_ref()
                    .and_then(|family| metadata.fonts.get(family))
                    .map(|font| font.source.clone());
                let can_wrap = layout.attributes.contains_key("w")
                    || text_width_estimate(&value, font_size) > layout.rect.width + 0.5;
                let max_lines = max_text_lines(layout);
                let truncate = attr(layout, "truncate").is_some_and(|value| value != "false")
                    || attr(layout, "overflow").is_some_and(|value| value == "ellipsis");
                let text_align = attr(layout, "align").map(ToOwned::to_owned);
                let letter_spacing = text_style_number(layout, metadata, "letter-spacing").unwrap_or(0.0);
                PaintContent::Text {
                    value,
                    font_family,
                    font_source,
                    font_weight,
                    font_style,
                    font_size,
                    line_height,
                    can_wrap,
                    max_lines,
                    truncate,
                    text_align,
                    letter_spacing,
                }
            })
            .unwrap_or(PaintContent::None),
        "img" => layout
            .attributes
            .get("src")
            .cloned()
            .map(|src| PaintContent::Image { src })
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

fn resolve_token(value: &str, metadata: &GuiMetadata) -> String {
    value
        .split_whitespace()
        .map(|part| {
            if let Some(name) = part.strip_prefix('$') {
                metadata
                    .tokens
                    .get(name)
                    .cloned()
                    .unwrap_or_else(|| part.to_owned())
            } else {
                part.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn parse_number(value: &str) -> Option<f32> {
    value.trim().trim_end_matches("px").parse::<f32>().ok()
}

fn text_style_number(layout: &LayoutBox, metadata: &GuiMetadata, name: &str) -> Option<f32> {
    attr(layout, name).and_then(parse_number).or_else(|| {
        let style_name = attr(layout, "text-style").or_else(|| attr(layout, "style"))?;
        let style = metadata.styles.get(style_name)?;
        style.get(name).and_then(|value| parse_number(value))
    })
}

fn text_style_value(layout: &LayoutBox, metadata: &GuiMetadata, name: &str) -> Option<String> {
    attr(layout, name).map(str::to_owned).or_else(|| {
        let style_name = attr(layout, "text-style").or_else(|| attr(layout, "style"))?;
        let style = metadata.styles.get(style_name)?;
        style.get(name).cloned()
    })
}

fn text_width_estimate(value: &str, font_size: f32) -> f32 {
    value.chars().count() as f32 * font_size * 0.55
}

fn max_text_lines(layout: &LayoutBox) -> Option<usize> {
    let max_lines = attr(layout, "max-lines")
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|lines| *lines > 0);

    if max_lines.is_some() {
        return max_lines;
    }

    if attr(layout, "truncate").is_some_and(|value| value != "false") {
        return Some(1);
    }

    None
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
                font_family: None,
                font_source: None,
                font_weight: None,
                font_style: None,
                font_size: 16.0,
                line_height: 19.2,
                can_wrap: false,
                max_lines: None,
                truncate: false,
                text_align: None,
                letter_spacing: 0.0,
            }
        );
        assert_eq!(
            scene.root.children[1].content,
            PaintContent::Image {
                src: "assets/icon.svg".to_owned()
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
                font_family: Some("Roboto".to_owned()),
                font_source: Some("google".to_owned()),
                font_weight: Some("500".to_owned()),
                font_style: None,
                font_size: 22.0,
                line_height: 28.0,
                can_wrap: false,
                max_lines: None,
                truncate: false,
                text_align: None,
                letter_spacing: 0.0,
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
                font_family: None,
                font_source: None,
                font_weight: None,
                font_style: None,
                font_size: 14.0,
                line_height: 20.0,
                can_wrap: true,
                max_lines: Some(1),
                truncate: true,
                text_align: Some("right".to_owned()),
                letter_spacing: 0.0,
            }
        );
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
