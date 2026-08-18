use crate::{GuiDocument, GuiMetadata, GuiNode};
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
    pub rect: LayoutRect,
    pub children: Vec<LayoutBox>,
}

pub trait TextMeasurer {
    fn text_width(
        &self,
        value: &str,
        font_family: Option<&str>,
        font_weight: Option<&str>,
        font_style: Option<&str>,
        font_size: f32,
    ) -> f32;
}

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
    ) -> f32 {
        value.chars().count() as f32 * font_size * 0.55
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct Insets {
    top: f32,
    right: f32,
    bottom: f32,
    left: f32,
}

impl Insets {
    fn horizontal(self) -> f32 {
        self.left + self.right
    }

    fn vertical(self) -> f32 {
        self.top + self.bottom
    }
}

pub fn compute_layout(document: &GuiDocument) -> LayoutBox {
    compute_layout_with_text(document, &ApproxTextMeasurer)
}

pub fn compute_layout_with_text(
    document: &GuiDocument,
    text_measurer: &dyn TextMeasurer,
) -> LayoutBox {
    layout_node(
        &document.root,
        &document.metadata,
        text_measurer,
        0.0,
        0.0,
        Constraints::default(),
    )
}

#[derive(Debug, Clone, Copy, Default)]
struct Constraints {
    width: Option<f32>,
    height: Option<f32>,
}

fn layout_node(
    node: &GuiNode,
    metadata: &GuiMetadata,
    text_measurer: &dyn TextMeasurer,
    x: f32,
    y: f32,
    constraints: Constraints,
) -> LayoutBox {
    match node.tag.as_str() {
        "row" => layout_row(node, metadata, text_measurer, x, y, constraints),
        "col" => layout_col(node, metadata, text_measurer, x, y, constraints),
        "stack" if attr_is(node, "direction", "horizontal") => {
            layout_row(node, metadata, text_measurer, x, y, constraints)
        }
        "stack" => layout_col(node, metadata, text_measurer, x, y, constraints),
        "frame" | "group" => layout_frame(node, metadata, text_measurer, x, y, constraints),
        _ => layout_leaf(node, metadata, text_measurer, x, y, constraints),
    }
}

fn layout_row(
    node: &GuiNode,
    metadata: &GuiMetadata,
    text_measurer: &dyn TextMeasurer,
    x: f32,
    y: f32,
    constraints: Constraints,
) -> LayoutBox {
    let padding = padding(node, metadata);
    let known_width = resolved_size(node, metadata, "w", constraints.width);
    let known_height = resolved_size(node, metadata, "h", constraints.height);
    let child_constraints = Constraints {
        width: None,
        height: known_height.map(|height| (height - padding.vertical()).max(0.0)),
    };
    let flow = flow_children(node).collect::<Vec<_>>();
    let fill_count = flow
        .iter()
        .filter(|child| attr_is(child, "w", "fill"))
        .count();
    let numeric_gap = number_attr(node, metadata, "gap");
    let gap = numeric_gap.unwrap_or(0.0);

    let mut measured_children = Vec::new();
    let mut fixed_width = 0.0_f32;
    let mut content_height = 0.0_f32;

    for child in &flow {
        if attr_is(child, "w", "fill") {
            continue;
        }
        let child_layout = layout_node(
            child,
            metadata,
            text_measurer,
            0.0,
            y + padding.top,
            child_constraints,
        );
        fixed_width += child_layout.rect.width;
        content_height = content_height.max(child_layout.rect.height);
        measured_children.push((*child as *const GuiNode, child_layout));
    }

    let numeric_gap_total = gap * flow.len().saturating_sub(1) as f32;
    let fill_width = known_width.map(|width| {
        let available = (width - padding.horizontal() - fixed_width - numeric_gap_total).max(0.0);
        if fill_count == 0 {
            0.0
        } else {
            available / fill_count as f32
        }
    });

    let mut children = Vec::new();
    let mut natural_content_width = 0.0_f32;
    for child in &flow {
        if attr_is(child, "w", "fill") {
            let child_layout = layout_node(
                child,
                metadata,
                text_measurer,
                0.0,
                y + padding.top,
                Constraints {
                    width: fill_width,
                    height: child_constraints.height,
                },
            );
            natural_content_width += child_layout.rect.width;
            content_height = content_height.max(child_layout.rect.height);
            children.push(child_layout);
        } else {
            let index = measured_children
                .iter()
                .position(|(ptr, _)| *ptr == *child as *const GuiNode)
                .expect("measured child exists");
            let (_, child_layout) = measured_children.remove(index);
            natural_content_width += child_layout.rect.width;
            children.push(child_layout);
        }
    }

    let known_content_area_width = known_width.map(|width| (width - padding.horizontal()).max(0.0));
    let actual_gap = if node
        .attributes
        .get("gap")
        .is_some_and(|value| value == "auto")
        && fill_count == 0
        && flow.len() > 1
    {
        known_content_area_width
            .map(|width| ((width - natural_content_width) / (flow.len() - 1) as f32).max(0.0))
            .unwrap_or(0.0)
    } else {
        gap
    };

    let mut cursor_x = x + padding.left;
    let child_y = y + padding.top;
    let mut content_width = 0.0_f32;
    let gap_total = actual_gap * flow.len().saturating_sub(1) as f32;
    let content_area_width = known_content_area_width.unwrap_or(natural_content_width + gap_total);
    let occupied_width = natural_content_width + gap_total;
    let main_offset = row_main_axis_offset(node, content_area_width, occupied_width);
    cursor_x += main_offset;
    for (index, child_layout) in children.iter_mut().enumerate() {
        if index > 0 {
            cursor_x += actual_gap;
            content_width += actual_gap;
        }
        translate_layout(
            child_layout,
            cursor_x - child_layout.rect.x,
            child_y - child_layout.rect.y,
        );
        cursor_x += child_layout.rect.width;
        content_width += child_layout.rect.width;
    }

    let width = known_width.unwrap_or(content_width + padding.horizontal());
    let height = known_height.unwrap_or(content_height + padding.vertical());

    apply_cross_axis_alignment(node, &mut children, y, height, padding, Axis::Horizontal);
    append_absolute_children(node, metadata, text_measurer, x, y, &mut children);

    LayoutBox {
        tag: node.tag.clone(),
        attributes: node.attributes.clone(),
        rect: LayoutRect {
            x,
            y,
            width,
            height,
        },
        children,
    }
}

fn layout_col(
    node: &GuiNode,
    metadata: &GuiMetadata,
    text_measurer: &dyn TextMeasurer,
    x: f32,
    y: f32,
    constraints: Constraints,
) -> LayoutBox {
    let padding = padding(node, metadata);
    let numeric_gap = number_attr(node, metadata, "gap");
    let gap = numeric_gap.unwrap_or(0.0);
    let known_width = resolved_size(node, metadata, "w", constraints.width);
    let known_height = resolved_size(node, metadata, "h", constraints.height);
    let child_constraints = Constraints {
        width: known_width.map(|width| (width - padding.horizontal()).max(0.0)),
        height: None,
    };
    let child_x = x + padding.left;
    let flow = flow_children(node).collect::<Vec<_>>();
    let fill_count = flow
        .iter()
        .filter(|child| attr_is(child, "h", "fill"))
        .count();
    let mut measured_children = Vec::new();
    let mut content_width = 0.0_f32;
    let mut fixed_height = 0.0_f32;

    for child in &flow {
        if attr_is(child, "h", "fill") {
            continue;
        }
        let child_layout = layout_node(
            child,
            metadata,
            text_measurer,
            child_x,
            0.0,
            child_constraints,
        );
        content_width = content_width.max(child_layout.rect.width);
        fixed_height += child_layout.rect.height;
        measured_children.push((*child as *const GuiNode, child_layout));
    }

    let numeric_gap_total = gap * flow.len().saturating_sub(1) as f32;
    let fill_height = known_height.map(|height| {
        let available = (height - padding.vertical() - fixed_height - numeric_gap_total).max(0.0);
        if fill_count == 0 {
            0.0
        } else {
            available / fill_count as f32
        }
    });

    let mut children = Vec::new();
    let mut natural_content_height = 0.0_f32;
    for child in &flow {
        if attr_is(child, "h", "fill") {
            let child_layout = layout_node(
                child,
                metadata,
                text_measurer,
                child_x,
                0.0,
                Constraints {
                    width: child_constraints.width,
                    height: fill_height,
                },
            );
            content_width = content_width.max(child_layout.rect.width);
            natural_content_height += child_layout.rect.height;
            children.push(child_layout);
        } else {
            let index = measured_children
                .iter()
                .position(|(ptr, _)| *ptr == *child as *const GuiNode)
                .expect("measured child exists");
            let (_, child_layout) = measured_children.remove(index);
            natural_content_height += child_layout.rect.height;
            children.push(child_layout);
        }
    }

    let known_content_area_height =
        known_height.map(|height| (height - padding.vertical()).max(0.0));
    let actual_gap = if node
        .attributes
        .get("gap")
        .is_some_and(|value| value == "auto")
        && fill_count == 0
        && flow.len() > 1
    {
        known_content_area_height
            .map(|height| ((height - natural_content_height) / (flow.len() - 1) as f32).max(0.0))
            .unwrap_or(0.0)
    } else {
        gap
    };

    let gap_total = actual_gap * flow.len().saturating_sub(1) as f32;
    let content_area_height =
        known_content_area_height.unwrap_or(natural_content_height + gap_total);
    let occupied_height = natural_content_height + gap_total;
    let mut cursor_y =
        y + padding.top + col_main_axis_offset(node, content_area_height, occupied_height);
    let mut content_height = 0.0_f32;
    for (index, child_layout) in children.iter_mut().enumerate() {
        if index > 0 {
            cursor_y += actual_gap;
            content_height += actual_gap;
        }
        translate_layout(
            child_layout,
            child_x - child_layout.rect.x,
            cursor_y - child_layout.rect.y,
        );
        cursor_y += child_layout.rect.height;
        content_height += child_layout.rect.height;
    }

    let width = known_width.unwrap_or(content_width + padding.horizontal());
    let height = known_height.unwrap_or(content_height + padding.vertical());

    apply_cross_axis_alignment(node, &mut children, x, width, padding, Axis::Vertical);
    append_absolute_children(node, metadata, text_measurer, x, y, &mut children);

    LayoutBox {
        tag: node.tag.clone(),
        attributes: node.attributes.clone(),
        rect: LayoutRect {
            x,
            y,
            width,
            height,
        },
        children,
    }
}

fn layout_frame(
    node: &GuiNode,
    metadata: &GuiMetadata,
    text_measurer: &dyn TextMeasurer,
    x: f32,
    y: f32,
    constraints: Constraints,
) -> LayoutBox {
    let width = resolved_size(node, metadata, "w", constraints.width).unwrap_or(0.0);
    let height = resolved_size(node, metadata, "h", constraints.height).unwrap_or(0.0);
    let mut children = Vec::new();

    for child in node
        .children
        .iter()
        .filter(|child| child.tag != "appearance")
    {
        let child_x = x + number_attr(child, metadata, "x").unwrap_or(0.0);
        let child_y = y + number_attr(child, metadata, "y").unwrap_or(0.0);
        children.push(layout_node(
            child,
            metadata,
            text_measurer,
            child_x,
            child_y,
            Constraints {
                width: Some(width),
                height: Some(height),
            },
        ));
    }

    LayoutBox {
        tag: node.tag.clone(),
        attributes: node.attributes.clone(),
        rect: LayoutRect {
            x,
            y,
            width,
            height,
        },
        children,
    }
}

fn layout_leaf(
    node: &GuiNode,
    metadata: &GuiMetadata,
    text_measurer: &dyn TextMeasurer,
    x: f32,
    y: f32,
    constraints: Constraints,
) -> LayoutBox {
    let intrinsic_width = intrinsic_width(node, metadata, text_measurer);
    let width = resolved_size(node, metadata, "w", constraints.width).unwrap_or_else(|| {
        if node.tag == "text" {
            constraints
                .width
                .map(|available| intrinsic_width.min(available))
                .unwrap_or(intrinsic_width)
        } else {
            intrinsic_width
        }
    });
    let height = resolved_size(node, metadata, "h", constraints.height)
        .unwrap_or_else(|| intrinsic_height(node, metadata, text_measurer, Some(width)));

    LayoutBox {
        tag: node.tag.clone(),
        attributes: node.attributes.clone(),
        rect: LayoutRect {
            x,
            y,
            width,
            height,
        },
        children: node
            .children
            .iter()
            .filter(|child| child.tag != "appearance")
            .map(|child| layout_node(child, metadata, text_measurer, x, y, Constraints::default()))
            .collect(),
    }
}

fn flow_children(node: &GuiNode) -> impl Iterator<Item = &GuiNode> {
    node.children.iter().filter(|child| {
        child.tag != "appearance"
            && child
                .attributes
                .get("abs")
                .or_else(|| child.attributes.get("layout-position"))
                .is_none_or(|value| value != "true" && value != "absolute")
    })
}

fn attr_is(node: &GuiNode, name: &str, value: &str) -> bool {
    node.attributes
        .get(name)
        .is_some_and(|actual| actual == value)
}

fn append_absolute_children(
    node: &GuiNode,
    metadata: &GuiMetadata,
    text_measurer: &dyn TextMeasurer,
    x: f32,
    y: f32,
    children: &mut Vec<LayoutBox>,
) {
    for child in node.children.iter().filter(|child| {
        child
            .attributes
            .get("abs")
            .is_some_and(|value| value == "true")
            || child
                .attributes
                .get("layout-position")
                .is_some_and(|value| value == "absolute")
    }) {
        let child_x = x + number_attr(child, metadata, "x").unwrap_or(0.0);
        let child_y = y + number_attr(child, metadata, "y").unwrap_or(0.0);
        children.push(layout_node(
            child,
            metadata,
            text_measurer,
            child_x,
            child_y,
            Constraints::default(),
        ));
    }
}

#[derive(Debug, Clone, Copy)]
enum Axis {
    Horizontal,
    Vertical,
}

fn apply_cross_axis_alignment(
    node: &GuiNode,
    children: &mut [LayoutBox],
    container_origin: f32,
    container_cross_size: f32,
    padding: Insets,
    axis: Axis,
) {
    let Some(align) = node.attributes.get("align") else {
        return;
    };

    let cross_align = if align == "stretch" {
        "stretch"
    } else {
        match axis {
            Axis::Horizontal => align.split('-').next().unwrap_or("top"),
            Axis::Vertical => align.split('-').nth(1).unwrap_or("left"),
        }
    };

    for child in children {
        match axis {
            Axis::Horizontal => {
                if cross_align == "stretch" && !has_size_attr(child, "h") {
                    child.rect.height = (container_cross_size - padding.vertical()).max(0.0);
                }
                let next_y = aligned_cross_origin(
                    container_origin,
                    cross_align,
                    container_cross_size,
                    child.rect.height,
                    padding.top,
                    padding.bottom,
                );
                translate_layout(child, 0.0, next_y - child.rect.y);
            }
            Axis::Vertical => {
                if cross_align == "stretch" && !has_size_attr(child, "w") {
                    child.rect.width = (container_cross_size - padding.horizontal()).max(0.0);
                }
                let next_x = aligned_cross_origin(
                    container_origin,
                    cross_align,
                    container_cross_size,
                    child.rect.width,
                    padding.left,
                    padding.right,
                );
                translate_layout(child, next_x - child.rect.x, 0.0);
            }
        }
    }
}

fn has_size_attr(layout: &LayoutBox, name: &str) -> bool {
    layout
        .attributes
        .get(name)
        .is_some_and(|value| value != "hug")
}

fn row_main_axis_offset(node: &GuiNode, container_width: f32, content_width: f32) -> f32 {
    let Some(align) = node.attributes.get("align") else {
        return 0.0;
    };
    let main_align = align.split('-').nth(1).unwrap_or("left");
    let leftover = (container_width - content_width).max(0.0);

    if matches!(main_align, "center" | "middle") {
        leftover / 2.0
    } else if main_align == "right" {
        leftover
    } else {
        0.0
    }
}

fn col_main_axis_offset(node: &GuiNode, container_height: f32, content_height: f32) -> f32 {
    let Some(align) = node.attributes.get("align") else {
        return 0.0;
    };
    let main_align = align.split('-').next().unwrap_or("top");
    let leftover = (container_height - content_height).max(0.0);

    if matches!(main_align, "middle" | "center") {
        leftover / 2.0
    } else if main_align == "bottom" {
        leftover
    } else {
        0.0
    }
}

fn translate_layout(layout: &mut LayoutBox, dx: f32, dy: f32) {
    layout.rect.x += dx;
    layout.rect.y += dy;
    for child in &mut layout.children {
        translate_layout(child, dx, dy);
    }
}

fn aligned_cross_origin(
    container_origin: f32,
    align: &str,
    container_size: f32,
    child_size: f32,
    before: f32,
    after: f32,
) -> f32 {
    container_origin
        + if matches!(align, "center" | "middle") {
            (container_size - child_size) / 2.0
        } else if matches!(align, "right" | "bottom") {
            container_size - after - child_size
        } else if align == "stretch" {
            before
        } else {
            before
        }
}

fn intrinsic_width(
    node: &GuiNode,
    metadata: &GuiMetadata,
    text_measurer: &dyn TextMeasurer,
) -> f32 {
    if node.tag == "line" {
        let direction = node
            .attributes
            .get("direction")
            .map(String::as_str)
            .unwrap_or("horizontal");
        return if direction == "vertical" {
            number_attr(node, metadata, "thickness").unwrap_or(1.0)
        } else {
            0.0
        };
    }

    if node.tag == "text" {
        let value = node
            .attributes
            .get("value")
            .or(node.text.as_ref())
            .map(String::as_str)
            .unwrap_or("");
        let font_size = text_style_number(node, metadata, "font-size")
            .or_else(|| text_style_number(node, metadata, "size"))
            .unwrap_or(16.0);
        let font_family = text_style_value(node, metadata, "font-family");
        let font_weight = text_style_value(node, metadata, "font-weight");
        let font_style = text_style_value(node, metadata, "font-style");
        return text_measurer.text_width(
            value,
            font_family.as_deref(),
            font_weight.as_deref(),
            font_style.as_deref(),
            font_size,
        );
    }
    0.0
}

fn intrinsic_height(
    node: &GuiNode,
    metadata: &GuiMetadata,
    text_measurer: &dyn TextMeasurer,
    width: Option<f32>,
) -> f32 {
    if node.tag == "line" {
        let direction = node
            .attributes
            .get("direction")
            .map(String::as_str)
            .unwrap_or("horizontal");
        return if direction == "vertical" {
            0.0
        } else {
            number_attr(node, metadata, "thickness").unwrap_or(1.0)
        };
    }

    if node.tag == "text" {
        let line_height = text_style_number(node, metadata, "line-height")
            .or_else(|| {
                text_style_number(node, metadata, "font-size")
                    .or_else(|| text_style_number(node, metadata, "size"))
                    .map(|size| size * 1.2)
            })
            .unwrap_or(19.2);
        let Some(width) = width.filter(|width| *width > 0.0) else {
            return line_height;
        };
        let value = node
            .attributes
            .get("value")
            .or(node.text.as_ref())
            .map(String::as_str)
            .unwrap_or("");
        let font_size = text_style_number(node, metadata, "font-size")
            .or_else(|| text_style_number(node, metadata, "size"))
            .unwrap_or(16.0);
        let font_family = text_style_value(node, metadata, "font-family");
        let font_weight = text_style_value(node, metadata, "font-weight");
        let font_style = text_style_value(node, metadata, "font-style");
        let lines = estimate_wrapped_line_count(
            value,
            width,
            font_family.as_deref(),
            font_weight.as_deref(),
            font_style.as_deref(),
            font_size,
            text_measurer,
        );
        let max_lines = max_text_lines(node, metadata).unwrap_or(lines);
        return line_height * lines.min(max_lines) as f32;
    }
    0.0
}

fn max_text_lines(node: &GuiNode, metadata: &GuiMetadata) -> Option<usize> {
    if node
        .attributes
        .get("truncate")
        .is_some_and(|value| value != "false")
    {
        return Some(1);
    }

    node.attributes
        .get("max-lines")
        .map(|value| resolve_token(value, metadata))
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|lines| *lines > 0)
}

fn estimate_wrapped_line_count(
    value: &str,
    width: f32,
    font_family: Option<&str>,
    font_weight: Option<&str>,
    font_style: Option<&str>,
    font_size: f32,
    text_measurer: &dyn TextMeasurer,
) -> usize {
    let mut lines = 1;
    let mut current = 0.0_f32;
    for word in value.split_whitespace() {
        let word_width =
            text_measurer.text_width(word, font_family, font_weight, font_style, font_size);
        let separator = if current > 0.0 {
            text_measurer.text_width(" ", font_family, font_weight, font_style, font_size)
        } else {
            0.0
        };
        if current > 0.0 && current + separator + word_width > width + 0.5 {
            lines += 1;
            current = word_width;
        } else {
            current += separator + word_width;
        }
    }

    lines
}

fn text_style_value(node: &GuiNode, metadata: &GuiMetadata, name: &str) -> Option<String> {
    node.attributes.get(name).cloned().or_else(|| {
        let style_name = node
            .attributes
            .get("text-style")
            .or_else(|| node.attributes.get("style"))?;
        let style = metadata.styles.get(style_name)?;
        style.get(name).cloned()
    })
}

fn padding(node: &GuiNode, metadata: &GuiMetadata) -> Insets {
    let mut insets = parse_insets(
        node.attributes
            .get("p")
            .or_else(|| node.attributes.get("padding"))
            .map(String::as_str),
        metadata,
    );

    if let Some(v) = number_attr(node, metadata, "pt") {
        insets.top = v;
    }
    if let Some(v) = number_attr(node, metadata, "pr") {
        insets.right = v;
    }
    if let Some(v) = number_attr(node, metadata, "pb") {
        insets.bottom = v;
    }
    if let Some(v) = number_attr(node, metadata, "pl") {
        insets.left = v;
    }

    insets
}

fn parse_insets(value: Option<&str>, metadata: &GuiMetadata) -> Insets {
    let Some(value) = value else {
        return Insets::default();
    };
    let resolved = resolve_token(value, metadata);
    let values = resolved
        .split_whitespace()
        .filter_map(parse_number)
        .collect::<Vec<_>>();

    match values.as_slice() {
        [] => Insets::default(),
        [all] => Insets {
            top: *all,
            right: *all,
            bottom: *all,
            left: *all,
        },
        [vertical, horizontal] => Insets {
            top: *vertical,
            right: *horizontal,
            bottom: *vertical,
            left: *horizontal,
        },
        [top, horizontal, bottom] => Insets {
            top: *top,
            right: *horizontal,
            bottom: *bottom,
            left: *horizontal,
        },
        [top, right, bottom, left, ..] => Insets {
            top: *top,
            right: *right,
            bottom: *bottom,
            left: *left,
        },
    }
}

fn number_attr(node: &GuiNode, metadata: &GuiMetadata, name: &str) -> Option<f32> {
    node.attributes
        .get(name)
        .map(|value| resolve_token(value, metadata))
        .and_then(|value| parse_number(&value))
}

fn resolved_size(
    node: &GuiNode,
    metadata: &GuiMetadata,
    name: &str,
    available: Option<f32>,
) -> Option<f32> {
    match node.attributes.get(name).map(String::as_str) {
        Some("fill") => available,
        Some("hug") | Some("auto") | None => None,
        Some(_) => number_attr(node, metadata, name),
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse_gui_xml;

    #[test]
    fn computes_column_flow_boxes() {
        let document = parse_gui_xml(
            r##"
            <gui version="0.2">
              <tokens>
                <token name="space" value="10" />
              </tokens>
              <col w="200" p="$space" gap="5">
                <rect w="50" h="20" />
                <rect w="80" h="30" />
              </col>
            </gui>
            "##,
        )
        .expect("valid gui");

        let layout = compute_layout(&document);

        assert_eq!(layout.rect.width, 200.0);
        assert_eq!(layout.rect.height, 75.0);
        assert_eq!(layout.children[0].rect.x, 10.0);
        assert_eq!(layout.children[0].rect.y, 10.0);
        assert_eq!(layout.children[1].rect.y, 35.0);
    }

    #[test]
    fn computes_frame_absolute_child_boxes() {
        let document = parse_gui_xml(
            r#"
            <gui version="0.2">
              <frame w="320" h="240">
                <rect x="12" y="18" w="40" h="50" />
              </frame>
            </gui>
            "#,
        )
        .expect("valid gui");

        let layout = compute_layout(&document);

        assert_eq!(layout.rect.width, 320.0);
        assert_eq!(layout.rect.height, 240.0);
        assert_eq!(layout.children[0].rect.x, 12.0);
        assert_eq!(layout.children[0].rect.y, 18.0);
    }

    #[test]
    fn fill_child_uses_parent_content_width() {
        let document = parse_gui_xml(
            r#"
            <gui version="0.2">
              <col w="200" p="10">
                <row w="fill" h="20" />
              </col>
            </gui>
            "#,
        )
        .expect("valid gui");

        let layout = compute_layout(&document);

        assert_eq!(layout.children[0].rect.width, 180.0);
    }

    #[test]
    fn auto_gap_distributes_row_children_when_width_is_known() {
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

        let layout = compute_layout(&document);

        assert_eq!(layout.children[0].rect.x, 0.0);
        assert_eq!(layout.children[1].rect.x, 80.0);
    }

    #[test]
    fn row_positioning_moves_nested_descendants() {
        let document = parse_gui_xml(
            r#"
            <gui version="0.2">
              <row w="100" gap="10">
                <rect w="20" h="20" />
                <row w="30" h="20">
                  <rect w="5" h="5" />
                </row>
              </row>
            </gui>
            "#,
        )
        .expect("valid gui");

        let layout = compute_layout(&document);

        assert_eq!(layout.children[1].rect.x, 30.0);
        assert_eq!(layout.children[1].children[0].rect.x, 30.0);
    }

    #[test]
    fn row_center_alignment_offsets_children_on_main_axis() {
        let document = parse_gui_xml(
            r#"
            <gui version="0.2">
              <row w="48" h="48" align="middle-center">
                <rect w="24" h="24" />
              </row>
            </gui>
            "#,
        )
        .expect("valid gui");

        let layout = compute_layout(&document);

        assert_eq!(layout.children[0].rect.x, 12.0);
        assert_eq!(layout.children[0].rect.y, 12.0);
    }

    #[test]
    fn exact_fit_text_stays_on_one_line() {
        let document = parse_gui_xml(
            r#"
            <gui version="0.2">
              <styles>
                <text-style name="Label" font-size="12" line-height="16" />
              </styles>
              <row w="328" gap="auto" align="middle-left">
                <text text-style="Label" value="STEP 2 OF 3" />
                <text text-style="Label" value="Return details" />
              </row>
            </gui>
            "#,
        )
        .expect("valid gui");

        let layout = compute_layout(&document);

        assert_eq!(layout.children[1].rect.height, 16.0);
    }

    #[test]
    fn truncate_limits_text_layout_to_one_line() {
        let document = parse_gui_xml(
            r#"
            <gui version="0.2">
              <styles>
                <text-style name="Body" font-size="16" line-height="24" />
              </styles>
              <col w="120">
                <text text-style="Body" value="This preview should be clipped to one line" w="120" truncate />
              </col>
            </gui>
            "#,
        )
        .expect("valid gui");

        let layout = compute_layout(&document);

        assert_eq!(layout.children[0].rect.height, 24.0);
    }

    #[test]
    fn stack_direction_horizontal_flows_like_row() {
        let document = parse_gui_xml(
            r#"
            <gui version="0.2">
              <stack direction="horizontal" gap="4">
                <rect w="10" h="10" />
                <rect w="20" h="10" />
              </stack>
            </gui>
            "#,
        )
        .expect("valid gui");

        let layout = compute_layout(&document);

        assert_eq!(layout.children[1].rect.x, 14.0);
        assert_eq!(layout.rect.width, 34.0);
    }

    #[test]
    fn column_fill_height_uses_remaining_space() {
        let document = parse_gui_xml(
            r#"
            <gui version="0.2">
              <col h="100" gap="10">
                <rect w="10" h="20" />
                <row w="10" h="fill" />
              </col>
            </gui>
            "#,
        )
        .expect("valid gui");

        let layout = compute_layout(&document);

        assert_eq!(layout.children[1].rect.height, 70.0);
        assert_eq!(layout.children[1].rect.y, 30.0);
    }

    #[test]
    fn column_auto_gap_distributes_vertical_space() {
        let document = parse_gui_xml(
            r#"
            <gui version="0.2">
              <col h="100" gap="auto">
                <rect w="10" h="20" />
                <rect w="10" h="20" />
              </col>
            </gui>
            "#,
        )
        .expect("valid gui");

        let layout = compute_layout(&document);

        assert_eq!(layout.children[1].rect.y, 80.0);
    }

    #[test]
    fn column_middle_alignment_offsets_children_on_main_axis() {
        let document = parse_gui_xml(
            r#"
            <gui version="0.2">
              <col h="100" align="middle-center">
                <rect w="10" h="20" />
              </col>
            </gui>
            "#,
        )
        .expect("valid gui");

        let layout = compute_layout(&document);

        assert_eq!(layout.children[0].rect.y, 40.0);
        assert_eq!(layout.children[0].rect.x, 0.0);
    }

    #[test]
    fn row_stretch_alignment_expands_auto_height_children() {
        let document = parse_gui_xml(
            r#"
            <gui version="0.2">
              <row w="100" h="48" align="stretch">
                <rect w="20" />
                <rect w="20" h="12" />
              </row>
            </gui>
            "#,
        )
        .expect("valid gui");

        let layout = compute_layout(&document);

        assert_eq!(layout.children[0].rect.height, 48.0);
        assert_eq!(layout.children[0].rect.y, 0.0);
        assert_eq!(layout.children[1].rect.height, 12.0);
    }

    #[test]
    fn column_stretch_alignment_expands_auto_width_children() {
        let document = parse_gui_xml(
            r#"
            <gui version="0.2">
              <col w="100" h="48" align="stretch">
                <rect h="20" />
                <rect w="12" h="20" />
              </col>
            </gui>
            "#,
        )
        .expect("valid gui");

        let layout = compute_layout(&document);

        assert_eq!(layout.children[0].rect.width, 100.0);
        assert_eq!(layout.children[0].rect.x, 0.0);
        assert_eq!(layout.children[1].rect.width, 12.0);
    }

    #[test]
    fn padding_alias_matches_p() {
        let document = parse_gui_xml(
            r#"
            <gui version="0.2">
              <col w="100" padding="10">
                <rect w="20" h="20" />
              </col>
            </gui>
            "#,
        )
        .expect("valid gui");

        let layout = compute_layout(&document);

        assert_eq!(layout.children[0].rect.x, 10.0);
        assert_eq!(layout.children[0].rect.y, 10.0);
        assert_eq!(layout.rect.height, 40.0);
    }

    #[test]
    fn horizontal_line_defaults_to_one_pixel_thickness() {
        let document = parse_gui_xml(
            r##"
            <gui version="0.2">
              <col w="100">
                <line w="fill" fill="#000000" />
              </col>
            </gui>
            "##,
        )
        .expect("valid gui");

        let layout = compute_layout(&document);

        assert_eq!(layout.children[0].rect.width, 100.0);
        assert_eq!(layout.children[0].rect.height, 1.0);
        assert_eq!(layout.rect.height, 1.0);
    }
}
