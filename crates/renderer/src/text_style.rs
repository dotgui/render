//! Resolving the font properties of `<text>` and its `<segment>` children.
//!
//! Layout works on [`GuiNode`], painting on [`LayoutBox`], and both have to
//! reach identical conclusions about what a run of text looks like. They share
//! this module through [`TextSource`] rather than each carrying a copy of the
//! attribute lookups.

use crate::{GuiMetadata, GuiNode, LayoutBox};
use std::collections::BTreeMap;

/// A node that can carry text: either a document node or a laid-out box.
pub(crate) trait TextSource {
    fn tag(&self) -> &str;
    fn attributes(&self) -> &BTreeMap<String, String>;
    fn body_text(&self) -> Option<&str>;
    fn text_children(&self) -> &[Self]
    where
        Self: Sized;
}

impl TextSource for GuiNode {
    fn tag(&self) -> &str {
        &self.tag
    }

    fn attributes(&self) -> &BTreeMap<String, String> {
        &self.attributes
    }

    fn body_text(&self) -> Option<&str> {
        self.text.as_deref()
    }

    fn text_children(&self) -> &[Self] {
        &self.children
    }
}

impl TextSource for LayoutBox {
    fn tag(&self) -> &str {
        &self.tag
    }

    fn attributes(&self) -> &BTreeMap<String, String> {
        &self.attributes
    }

    fn body_text(&self) -> Option<&str> {
        self.text.as_deref()
    }

    fn text_children(&self) -> &[Self] {
        &self.children
    }
}

/// The font properties a run of text is drawn with, after tokens, `text-style`
/// lookups, and inheritance from the parent `<text>` have been applied.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct TextStyle {
    pub font_family: Option<String>,
    pub font_weight: Option<String>,
    pub font_style: Option<String>,
    pub font_size: f32,
    pub line_height: f32,
    pub letter_spacing: f32,
    pub color: Option<String>,
}

/// One styled run of a `<text>` node.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct TextRunStyle {
    pub value: String,
    pub style: TextStyle,
}

const DEFAULT_FONT_SIZE: f32 = 16.0;

/// Line height when none is declared, as a multiple of the font size.
const DEFAULT_LINE_HEIGHT_RATIO: f32 = 1.2;

/// Resolves the style of a `<text>` node itself.
pub(crate) fn resolve_text_style<S: TextSource>(node: &S, metadata: &GuiMetadata) -> TextStyle {
    let font_size = style_number(node, metadata, "font-size")
        .or_else(|| style_number(node, metadata, "size"))
        .unwrap_or(DEFAULT_FONT_SIZE);

    TextStyle {
        font_family: style_value(node, metadata, "font-family"),
        font_weight: style_value(node, metadata, "font-weight"),
        font_style: style_value(node, metadata, "font-style"),
        font_size,
        line_height: style_number(node, metadata, "line-height")
            .unwrap_or(font_size * DEFAULT_LINE_HEIGHT_RATIO),
        letter_spacing: style_number(node, metadata, "letter-spacing").unwrap_or(0.0),
        color: node
            .attributes()
            .get("fill")
            .or_else(|| node.attributes().get("color"))
            .map(|value| resolve_token(value, metadata)),
    }
}

/// Splits a `<text>` node into the runs it should be drawn as.
///
/// A node without `<segment>` children is a single run carrying its own style.
/// Segments inherit every property they do not override, so
/// `<text font-size="14"><segment font-weight="700" /></text>` keeps size 14.
///
/// A node's own `value`/body text is emitted before its segments, which lets
/// `<text value="Total: "><segment value="$12" font-weight="700"/></text>` read
/// naturally.
pub(crate) fn resolve_text_runs<S: TextSource>(
    node: &S,
    metadata: &GuiMetadata,
) -> Vec<TextRunStyle> {
    let parent = resolve_text_style(node, metadata);
    let mut runs = Vec::new();

    let own_value = node
        .attributes()
        .get("value")
        .map(String::as_str)
        .or_else(|| node.body_text())
        .unwrap_or("");
    if !own_value.is_empty() {
        runs.push(TextRunStyle {
            value: own_value.to_owned(),
            style: parent.clone(),
        });
    }

    for child in node.text_children() {
        if child.tag() != "segment" {
            continue;
        }
        let value = child
            .attributes()
            .get("value")
            .map(String::as_str)
            .or_else(|| child.body_text())
            .unwrap_or("");
        if value.is_empty() {
            continue;
        }
        runs.push(TextRunStyle {
            value: value.to_owned(),
            style: inherit_style(child, metadata, &parent),
        });
    }

    if runs.is_empty() {
        runs.push(TextRunStyle {
            value: String::new(),
            style: parent,
        });
    }

    runs
}

fn inherit_style<S: TextSource>(node: &S, metadata: &GuiMetadata, parent: &TextStyle) -> TextStyle {
    let font_size = style_number(node, metadata, "font-size")
        .or_else(|| style_number(node, metadata, "size"))
        .unwrap_or(parent.font_size);

    // A segment that changes size but not line height gets a line height
    // derived from its own size, so a larger run is not clipped.
    let inherited_line_height = if font_size == parent.font_size {
        parent.line_height
    } else {
        font_size * DEFAULT_LINE_HEIGHT_RATIO
    };
    let line_height = style_number(node, metadata, "line-height").unwrap_or(inherited_line_height);

    TextStyle {
        font_family: style_value(node, metadata, "font-family")
            .or_else(|| parent.font_family.clone()),
        font_weight: style_value(node, metadata, "font-weight")
            .or_else(|| parent.font_weight.clone()),
        font_style: style_value(node, metadata, "font-style").or_else(|| parent.font_style.clone()),
        font_size,
        line_height,
        letter_spacing: style_number(node, metadata, "letter-spacing")
            .unwrap_or(parent.letter_spacing),
        color: node
            .attributes()
            .get("fill")
            .or_else(|| node.attributes().get("color"))
            .map(|value| resolve_token(value, metadata))
            .or_else(|| parent.color.clone()),
    }
}

fn style_value<S: TextSource>(node: &S, metadata: &GuiMetadata, name: &str) -> Option<String> {
    node.attributes()
        .get(name)
        .map(|value| resolve_token(value, metadata))
        .or_else(|| named_style(node, metadata)?.get(name).cloned())
}

fn style_number<S: TextSource>(node: &S, metadata: &GuiMetadata, name: &str) -> Option<f32> {
    node.attributes()
        .get(name)
        .map(|value| resolve_token(value, metadata))
        .and_then(|value| parse_number(&value))
        .or_else(|| {
            named_style(node, metadata)?
                .get(name)
                .and_then(|value| parse_number(value))
        })
}

fn named_style<'a, S: TextSource>(
    node: &S,
    metadata: &'a GuiMetadata,
) -> Option<&'a BTreeMap<String, String>> {
    let name = node
        .attributes()
        .get("text-style")
        .or_else(|| node.attributes().get("style"))?;
    metadata.styles.get(name)
}

/// Substitutes `$name` token references.
///
/// Every whitespace-separated part is resolved independently, because compound
/// values carry tokens inside them: `border="1 $rule"`, `p="$sp-2 $sp-4"`.
pub(crate) fn resolve_token(value: &str, metadata: &GuiMetadata) -> String {
    if !value.contains('$') {
        return value.to_owned();
    }

    value
        .split_whitespace()
        .map(|part| match part.strip_prefix('$') {
            Some(name) => metadata
                .tokens
                .get(name)
                .cloned()
                .unwrap_or_else(|| part.to_owned()),
            None => part.to_owned(),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn parse_number(value: &str) -> Option<f32> {
    if matches!(value, "fill" | "hug" | "auto") {
        return None;
    }
    value
        .trim()
        .trim_end_matches("px")
        .trim_end_matches('%')
        .parse::<f32>()
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse_gui_xml;

    fn text_node(xml: &str) -> (GuiNode, GuiMetadata) {
        let document = parse_gui_xml(xml).expect("valid gui");
        let node = document.root.children[0].clone();
        (node, document.metadata)
    }

    #[test]
    fn a_plain_text_node_is_one_run() {
        let (node, metadata) = text_node(
            r##"
            <gui version="0.2">
              <col>
                <text value="Hello" font-size="14" />
              </col>
            </gui>
            "##,
        );

        let runs = resolve_text_runs(&node, &metadata);
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].value, "Hello");
        assert_eq!(runs[0].style.font_size, 14.0);
    }

    #[test]
    fn segments_inherit_what_they_do_not_override() {
        let (node, metadata) = text_node(
            r##"
            <gui version="0.2">
              <col>
                <text font-size="14" font-family="Inter" fill="#111111">
                  <segment value="Total: " />
                  <segment value="$12" font-weight="700" fill="#ff0000" />
                </text>
              </col>
            </gui>
            "##,
        );

        let runs = resolve_text_runs(&node, &metadata);
        assert_eq!(runs.len(), 2);

        assert_eq!(runs[0].value, "Total: ");
        assert_eq!(runs[0].style.font_weight, None);
        assert_eq!(runs[0].style.color.as_deref(), Some("#111111"));

        assert_eq!(runs[1].value, "$12");
        assert_eq!(runs[1].style.font_weight.as_deref(), Some("700"));
        assert_eq!(runs[1].style.color.as_deref(), Some("#ff0000"));
        // Inherited, not defaulted.
        assert_eq!(runs[1].style.font_family.as_deref(), Some("Inter"));
        assert_eq!(runs[1].style.font_size, 14.0);
    }

    #[test]
    fn a_text_value_precedes_its_segments() {
        let (node, metadata) = text_node(
            r##"
            <gui version="0.2">
              <col>
                <text value="Total: " font-size="14">
                  <segment value="$12" font-weight="700" />
                </text>
              </col>
            </gui>
            "##,
        );

        let runs = resolve_text_runs(&node, &metadata);
        assert_eq!(
            runs.iter()
                .map(|run| run.value.as_str())
                .collect::<Vec<_>>(),
            vec!["Total: ", "$12"]
        );
    }

    #[test]
    fn a_resized_segment_gets_its_own_line_height() {
        let (node, metadata) = text_node(
            r##"
            <gui version="0.2">
              <col>
                <text font-size="10" line-height="12">
                  <segment value="small" />
                  <segment value="big" font-size="30" />
                </text>
              </col>
            </gui>
            "##,
        );

        let runs = resolve_text_runs(&node, &metadata);
        assert_eq!(runs[0].style.line_height, 12.0);
        assert_eq!(runs[1].style.line_height, 36.0);
    }

    #[test]
    fn segments_resolve_tokens_and_named_styles() {
        let (node, metadata) = text_node(
            r##"
            <gui version="0.2">
              <tokens><color name="danger" value="#ff0000" /></tokens>
              <styles><text-style name="strong" font-weight="700" /></styles>
              <col>
                <text font-size="14">
                  <segment value="oops" text-style="strong" fill="$danger" />
                </text>
              </col>
            </gui>
            "##,
        );

        let runs = resolve_text_runs(&node, &metadata);
        assert_eq!(runs[0].style.font_weight.as_deref(), Some("700"));
        assert_eq!(runs[0].style.color.as_deref(), Some("#ff0000"));
    }
}
