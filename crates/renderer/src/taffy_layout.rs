use crate::{GuiDocument, GuiMetadata, GuiNode, LayoutBox, LayoutRect};
use taffy::prelude::*;
use taffy::TaffyError;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum TaffyLayoutError {
    #[error("taffy layout failed: {0}")]
    Taffy(#[from] TaffyError),
}

struct BuiltNode<'a> {
    source: &'a GuiNode,
    node_id: NodeId,
    children: Vec<BuiltNode<'a>>,
}

pub fn compute_taffy_layout(document: &GuiDocument) -> Result<LayoutBox, TaffyLayoutError> {
    let mut tree: TaffyTree<()> = TaffyTree::new();
    let built = build_node(&mut tree, &document.root, &document.metadata)?;

    let width = number_attr(&document.root, &document.metadata, "w");
    let height = number_attr(&document.root, &document.metadata, "h");
    tree.compute_layout(
        built.node_id,
        Size {
            width: width.map_or(AvailableSpace::MaxContent, AvailableSpace::Definite),
            height: height.map_or(AvailableSpace::MaxContent, AvailableSpace::Definite),
        },
    )?;

    read_layout(&tree, &built, 0.0, 0.0).map_err(TaffyLayoutError::from)
}

fn build_node<'a>(
    tree: &mut TaffyTree<()>,
    node: &'a GuiNode,
    metadata: &GuiMetadata,
) -> Result<BuiltNode<'a>, TaffyError> {
    let children = node
        .children
        .iter()
        .filter(|child| child.tag != "appearance")
        .map(|child| build_node(tree, child, metadata))
        .collect::<Result<Vec<_>, _>>()?;
    let child_ids = children
        .iter()
        .map(|child| child.node_id)
        .collect::<Vec<_>>();

    let style = style_for_node(node, metadata);
    let node_id = if child_ids.is_empty() {
        tree.new_leaf(style)?
    } else {
        tree.new_with_children(style, &child_ids)?
    };

    Ok(BuiltNode {
        source: node,
        node_id,
        children,
    })
}

fn read_layout(
    tree: &TaffyTree<()>,
    built: &BuiltNode<'_>,
    offset_x: f32,
    offset_y: f32,
) -> Result<LayoutBox, TaffyError> {
    let layout = tree.layout(built.node_id)?;
    let x = offset_x + layout.location.x;
    let y = offset_y + layout.location.y;

    Ok(LayoutBox {
        tag: built.source.tag.clone(),
        attributes: built.source.attributes.clone(),
        rect: LayoutRect {
            x,
            y,
            width: layout.size.width,
            height: layout.size.height,
        },
        children: built
            .children
            .iter()
            .map(|child| read_layout(tree, child, x, y))
            .collect::<Result<Vec<_>, _>>()?,
    })
}

fn style_for_node(node: &GuiNode, metadata: &GuiMetadata) -> Style {
    let mut style = Style {
        display: display_for(node),
        flex_direction: flex_direction_for(node),
        size: Size {
            width: dimension_attr(node, metadata, "w"),
            height: dimension_attr(node, metadata, "h"),
        },
        min_size: Size {
            width: dimension_attr(node, metadata, "min-w"),
            height: dimension_attr(node, metadata, "min-h"),
        },
        max_size: Size {
            width: dimension_attr(node, metadata, "max-w"),
            height: dimension_attr(node, metadata, "max-h"),
        },
        padding: Rect {
            left: length(padding_side(node, metadata, Side::Left)),
            right: length(padding_side(node, metadata, Side::Right)),
            top: length(padding_side(node, metadata, Side::Top)),
            bottom: length(padding_side(node, metadata, Side::Bottom)),
        },
        gap: Size {
            width: gap_dimension(node, metadata),
            height: gap_dimension(node, metadata),
        },
        align_items: align_items_for(node),
        justify_content: justify_content_for(node),
        ..Default::default()
    };

    if attr_is(node, "w", "fill") {
        style.flex_grow = 1.0;
        style.flex_shrink = 1.0;
        style.flex_basis = zero();
    }
    if attr_is(node, "h", "fill") {
        style.flex_grow = 1.0;
        style.flex_shrink = 1.0;
    }
    if is_absolute(node) {
        style.position = Position::Absolute;
        style.inset.left = number_attr(node, metadata, "x").map_or(auto(), length);
        style.inset.top = number_attr(node, metadata, "y").map_or(auto(), length);
    }
    if node.tag == "text" && !node.attributes.contains_key("w") {
        style.size.width = length(intrinsic_text_width(node, metadata));
    }
    if node.tag == "text" && !node.attributes.contains_key("h") {
        style.size.height = length(intrinsic_text_height(node, metadata));
    }

    style
}

fn display_for(node: &GuiNode) -> Display {
    match node.tag.as_str() {
        "row" | "col" | "stack" | "frame" => Display::Flex,
        "grid" => Display::Grid,
        _ => Display::Block,
    }
}

fn flex_direction_for(node: &GuiNode) -> FlexDirection {
    match node.tag.as_str() {
        "row" => FlexDirection::Row,
        _ => FlexDirection::Column,
    }
}

fn dimension_attr(node: &GuiNode, metadata: &GuiMetadata, name: &str) -> Dimension {
    match node.attributes.get(name).map(String::as_str) {
        Some("fill") => percent(1.0),
        Some("hug") | Some("auto") | None => auto(),
        Some(_) => number_attr(node, metadata, name).map_or(auto(), length),
    }
}

fn gap_dimension(node: &GuiNode, metadata: &GuiMetadata) -> LengthPercentage {
    if node
        .attributes
        .get("gap")
        .is_some_and(|value| value == "auto")
    {
        zero()
    } else {
        number_attr(node, metadata, "gap").map_or(zero(), length)
    }
}

fn align_items_for(node: &GuiNode) -> Option<AlignItems> {
    let align = node.attributes.get("align")?;
    let cross = if node.tag == "row" {
        align.split('-').next().unwrap_or("top")
    } else {
        align.split('-').nth(1).unwrap_or("left")
    };

    Some(match cross {
        "center" | "middle" => AlignItems::CENTER,
        "right" | "bottom" => AlignItems::END,
        _ => AlignItems::START,
    })
}

fn justify_content_for(node: &GuiNode) -> Option<JustifyContent> {
    if node
        .attributes
        .get("gap")
        .is_some_and(|value| value == "auto")
    {
        return Some(JustifyContent::SPACE_BETWEEN);
    }

    let align = node.attributes.get("align")?;
    let main = if node.tag == "row" {
        align.split('-').nth(1).unwrap_or("left")
    } else {
        align.split('-').next().unwrap_or("top")
    };

    Some(match main {
        "center" | "middle" => JustifyContent::CENTER,
        "right" | "bottom" => JustifyContent::END,
        _ => JustifyContent::START,
    })
}

#[derive(Clone, Copy)]
enum Side {
    Top,
    Right,
    Bottom,
    Left,
}

fn padding_side(node: &GuiNode, metadata: &GuiMetadata, side: Side) -> f32 {
    let explicit = match side {
        Side::Top => "pt",
        Side::Right => "pr",
        Side::Bottom => "pb",
        Side::Left => "pl",
    };
    if let Some(value) = number_attr(node, metadata, explicit) {
        return value;
    }

    let Some(value) = node.attributes.get("p") else {
        return 0.0;
    };
    let resolved = resolve_token(value, metadata);
    let values = resolved
        .split_whitespace()
        .filter_map(parse_number)
        .collect::<Vec<_>>();

    match (values.as_slice(), side) {
        ([], _) => 0.0,
        ([all], _) => *all,
        ([vertical, _], Side::Top | Side::Bottom) => *vertical,
        ([_, horizontal], Side::Right | Side::Left) => *horizontal,
        ([top, _, _], Side::Top) => *top,
        ([_, _, bottom], Side::Bottom) => *bottom,
        ([_, horizontal, _], Side::Right | Side::Left) => *horizontal,
        ([top, _, _, _], Side::Top) => *top,
        ([_, right, _, _], Side::Right) => *right,
        ([_, _, bottom, _], Side::Bottom) => *bottom,
        ([_, _, _, left], Side::Left) => *left,
        _ => 0.0,
    }
}

fn intrinsic_text_width(node: &GuiNode, metadata: &GuiMetadata) -> f32 {
    let value = node
        .attributes
        .get("value")
        .or(node.text.as_ref())
        .map(String::as_str)
        .unwrap_or("");
    let font_size = text_style_number(node, metadata, "font-size")
        .or_else(|| text_style_number(node, metadata, "size"))
        .unwrap_or(16.0);
    let letter_spacing = text_style_number(node, metadata, "letter-spacing").unwrap_or(0.0);
    let char_count = value.chars().count() as f32;
    let base_width = char_count * font_size * 0.55;
    base_width + char_count * letter_spacing
}

fn intrinsic_text_height(node: &GuiNode, metadata: &GuiMetadata) -> f32 {
    text_style_number(node, metadata, "line-height")
        .or_else(|| {
            text_style_number(node, metadata, "font-size")
                .or_else(|| text_style_number(node, metadata, "size"))
                .map(|size| size * 1.2)
        })
        .unwrap_or(19.2)
}

fn text_style_number(node: &GuiNode, metadata: &GuiMetadata, name: &str) -> Option<f32> {
    number_attr(node, metadata, name).or_else(|| {
        let style_name = node
            .attributes
            .get("text-style")
            .or_else(|| node.attributes.get("style"))?;
        let style = metadata.styles.get(style_name)?;
        style.get(name).and_then(|value| parse_number(value))
    })
}

fn number_attr(node: &GuiNode, metadata: &GuiMetadata, name: &str) -> Option<f32> {
    node.attributes
        .get(name)
        .map(|value| resolve_token(value, metadata))
        .and_then(|value| parse_number(&value))
}

fn resolve_token(value: &str, metadata: &GuiMetadata) -> String {
    if let Some(name) = value.strip_prefix('$') {
        metadata
            .tokens
            .get(name)
            .cloned()
            .unwrap_or_else(|| value.to_owned())
    } else {
        value.to_owned()
    }
}

fn parse_number(value: &str) -> Option<f32> {
    if matches!(value, "fill" | "hug" | "auto") {
        return None;
    }
    let trimmed = value.trim().trim_end_matches("px").trim_end_matches('%');
    trimmed.parse::<f32>().ok()
}

fn attr_is(node: &GuiNode, name: &str, value: &str) -> bool {
    node.attributes
        .get(name)
        .is_some_and(|actual| actual == value)
}

fn is_absolute(node: &GuiNode) -> bool {
    attr_is(node, "abs", "true")
        || node
            .attributes
            .get("layout-position")
            .is_some_and(|value| value == "absolute")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse_gui_xml;

    #[test]
    fn computes_basic_column_with_taffy() {
        let document = parse_gui_xml(
            r#"
            <gui version="0.2">
              <col w="200" p="10" gap="5">
                <rect w="50" h="20" />
                <rect w="80" h="30" />
              </col>
            </gui>
            "#,
        )
        .expect("valid gui");

        let layout = compute_taffy_layout(&document).expect("layout computes");

        assert_eq!(layout.rect.width, 200.0);
        assert_eq!(layout.rect.height, 75.0);
        assert_eq!(layout.children[0].rect.x, 10.0);
        assert_eq!(layout.children[0].rect.y, 10.0);
        assert_eq!(layout.children[1].rect.y, 35.0);
    }

    #[test]
    fn computes_auto_gap_row_with_taffy() {
        let document = parse_gui_xml(
            r#"
            <gui version="0.2">
              <row w="100" gap="auto">
                <rect w="10" h="10" />
                <rect w="20" h="10" />
              </row>
            </gui>
            "#,
        )
        .expect("valid gui");

        let layout = compute_taffy_layout(&document).expect("layout computes");

        assert_eq!(layout.children[0].rect.x, 0.0);
        assert_eq!(layout.children[1].rect.x, 80.0);
    }
}
