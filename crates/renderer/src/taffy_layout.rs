use crate::{
    fonts::FontAxes,
    grid::{self, GridMode},
    text,
    text_style::{resolve_text_runs, resolve_token, TextRunStyle},
    ApproxTextMeasurer, GuiDocument, GuiMetadata, GuiNode, LayoutBox, LayoutRect, TextMeasurer,
};
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

/// Everything the measure function needs to break a `<text>` node into lines.
///
/// Taffy only hands the measure function its node context, so the resolved
/// font attributes are captured here while the tree is built.
struct TextContext {
    /// Styled runs, already resolved and inherited.
    runs: Vec<TextRunStyle>,
    max_lines: Option<usize>,
    /// Where lines may break, so measurement wraps the way painting will.
    wrap: text::WrapOptions,
    /// The list marker's text, whose width is reserved on the first line.
    list_marker: Option<String>,
    /// Left indent for the whole block, from `list-level`.
    list_indent: f32,
    /// Whether `leading-trim` takes the half-leading off the first line.
    leading_trim: bool,
}

/// Lays the document out using rough per-character width estimates.
///
/// Prefer [`compute_taffy_layout_with_text`] wherever a [`FontStore`] is
/// available: painting measures with real font metrics, so layout has to as
/// well or wrapped text is sized against the wrong widths.
///
/// [`FontStore`]: crate::FontStore
pub fn compute_taffy_layout(document: &GuiDocument) -> Result<LayoutBox, TaffyLayoutError> {
    compute_taffy_layout_with_text(document, &ApproxTextMeasurer)
}

/// Lays the document out, measuring text with `text_measurer`.
pub fn compute_taffy_layout_with_text(
    document: &GuiDocument,
    text_measurer: &dyn TextMeasurer,
) -> Result<LayoutBox, TaffyLayoutError> {
    let mut tree: TaffyTree<TextContext> = TaffyTree::new();
    // Taffy rounds boxes to whole pixels by default, which can leave a text box
    // a fraction narrower than the string it was measured for; painting would
    // then re-wrap and drop the overflowing word.
    tree.disable_rounding();
    let built = build_node(
        &mut tree,
        &document.root,
        &document.metadata,
        ParentLayout::Column,
        1,
    )?;

    let width = number_attr(&document.root, &document.metadata, "w");
    let height = number_attr(&document.root, &document.metadata, "h");
    tree.compute_layout_with_measure(
        built.node_id,
        Size {
            width: width.map_or(AvailableSpace::MaxContent, AvailableSpace::Definite),
            height: height.map_or(AvailableSpace::MaxContent, AvailableSpace::Definite),
        },
        |known_dimensions, available_space, _node_id, node_context, _style| {
            let Some(context) = node_context else {
                return Size::ZERO;
            };
            measure_text(context, text_measurer, known_dimensions, available_space)
        },
    )?;

    read_layout(&tree, &built, 0.0, 0.0).map_err(TaffyLayoutError::from)
}

/// What kind of container a node sits inside, which decides how `fill` and the
/// grid placement attributes are read.
#[derive(Clone, Copy, PartialEq)]
enum ParentLayout {
    Row,
    Column,
    /// A `<grid>`, whose children may place themselves with `gc` / `gr`.
    Grid,
    /// A `<stack direction="grid">`, which predates placement attributes and
    /// only auto-flows its children.
    LegacyGrid,
    /// A `<frame>` or `<group>`: a positioned container, whose children sit at
    /// their own `x`/`y` rather than flowing.
    Positioned,
}

fn parent_layout_of(node: &GuiNode, metadata: &GuiMetadata) -> ParentLayout {
    if let Some(mode) = grid::grid_mode(node, metadata) {
        if matches!(mode, GridMode::LegacyStack) {
            ParentLayout::LegacyGrid
        } else {
            ParentLayout::Grid
        }
    } else if matches!(node.tag.as_str(), "frame" | "group") {
        // A `<frame>` positions its children; only `<stack>`, `<row>`, `<col>`
        // and `<grid>` lay them out in flow. The spec says the same of
        // `<group>`: "children are absolutely positioned relative to the group
        // origin".
        ParentLayout::Positioned
    } else if flex_direction_for(node) == FlexDirection::Row {
        ParentLayout::Row
    } else {
        ParentLayout::Column
    }
}

fn build_node<'a>(
    tree: &mut TaffyTree<TextContext>,
    node: &'a GuiNode,
    metadata: &GuiMetadata,
    parent_layout: ParentLayout,
    ordinal: usize,
) -> Result<BuiltNode<'a>, TaffyError> {
    let layout = parent_layout_of(node, metadata);
    // `appearance` describes the parent's paint and `segment` is text content;
    // neither is a box. Keeping segments out also leaves `<text>` childless, so
    // Taffy still calls the measure function on it.
    // A decimal list item is numbered by its place among its list-item
    // siblings, and the marker's width is part of measurement, so the count
    // has to happen here rather than only when the scene is built.
    let mut child_ordinal = 0usize;
    let children = node
        .children
        .iter()
        .filter(|child| child.tag != "appearance" && child.tag != "segment")
        .map(|child| {
            if child
                .attributes
                .get("list")
                .is_some_and(|value| value != "none")
            {
                child_ordinal += 1;
            }
            build_node(tree, child, metadata, layout, child_ordinal.max(1))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let child_ids = children
        .iter()
        .map(|child| child.node_id)
        .collect::<Vec<_>>();

    warn_on_unsupported_tag(node);

    let style = style_for_node(node, metadata, parent_layout);
    let node_id = if !child_ids.is_empty() {
        tree.new_with_children(style, &child_ids)?
    } else if let Some(context) = text_context(node, metadata, ordinal) {
        // Taffy only measures childless leaves, which is exactly where text
        // lives: `<text>` carries its string in an attribute.
        tree.new_leaf_with_context(style, context)?
    } else {
        tree.new_leaf(style)?
    };

    Ok(BuiltNode {
        source: node,
        node_id,
        children,
    })
}

fn read_layout(
    tree: &TaffyTree<TextContext>,
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
        text: built.source.text.clone(),
        rect: LayoutRect {
            x,
            y,
            width: layout.size.width,
            height: layout.size.height,
        },
        children: read_children(tree, built, x, y)?,
    })
}

fn read_children(
    tree: &TaffyTree<TextContext>,
    built: &BuiltNode<'_>,
    x: f32,
    y: f32,
) -> Result<Vec<LayoutBox>, TaffyError> {
    let mut children = built
        .children
        .iter()
        .map(|child| read_layout(tree, child, x, y))
        .collect::<Result<Vec<_>, _>>()?;

    // `<segment>` and `<appearance>` never entered the layout tree: one is text
    // content, the other describes the parent's paint. Keeping segments out is
    // also what lets Taffy measure `<text>` as a leaf. Both still have to reach
    // the scene, so they ride along without geometry.
    children.extend(
        built
            .source
            .children
            .iter()
            .filter(|child| child.tag == "segment" || child.tag == "appearance")
            .map(|child| content_box(child, x, y)),
    );

    Ok(children)
}

/// Copies a non-layout subtree into the box tree, carrying attributes and text
/// but no geometry.
fn content_box(node: &GuiNode, x: f32, y: f32) -> LayoutBox {
    LayoutBox {
        tag: node.tag.clone(),
        attributes: node.attributes.clone(),
        text: node.text.clone(),
        rect: LayoutRect {
            x,
            y,
            width: 0.0,
            height: 0.0,
        },
        children: node
            .children
            .iter()
            .map(|child| content_box(child, x, y))
            .collect(),
    }
}

fn style_for_node(node: &GuiNode, metadata: &GuiMetadata, parent_layout: ParentLayout) -> Style {
    let mut style = Style {
        display: display_for(node, metadata),
        flex_direction: flex_direction_for(node),
        size: Size {
            width: dimension_attr(node, metadata, "w"),
            height: dimension_attr(node, metadata, "h"),
        },
        min_size: Size {
            width: constraint_attr(node, metadata, "min-width", "min-w"),
            height: constraint_attr(node, metadata, "min-height", "min-h"),
        },
        max_size: Size {
            width: constraint_attr(node, metadata, "max-width", "max-w"),
            height: constraint_attr(node, metadata, "max-height", "max-h"),
        },
        padding: Rect {
            left: length(padding_side(node, metadata, Side::Left)),
            right: length(padding_side(node, metadata, Side::Right)),
            top: length(padding_side(node, metadata, Side::Top)),
            bottom: length(padding_side(node, metadata, Side::Bottom)),
        },
        aspect_ratio: aspect_ratio(node, metadata),
        // `paragraph-spacing` is room after the block, which kit gets from a
        // bottom margin; the same is true here.
        margin: Rect {
            left: zero(),
            right: zero(),
            top: zero(),
            bottom: number_attr(node, metadata, "paragraph-spacing").map_or(zero(), length),
        },
        gap: gap_size(node, metadata),
        align_items: align_items_for(node),
        justify_content: justify_content_for(node),
        // A declared size is a fixed size, as in a design tool: only `fill`
        // boxes give way when a container runs short of room.
        flex_shrink: 0.0,
        ..Default::default()
    };

    // `fill` means different things per axis. Along the parent's main axis it
    // is "take the free space", i.e. flex-grow. Across it, stretching to the
    // parent's width/height is what `dimension_attr` already returns, and
    // growing there would collapse the box's own size contribution.
    let fills_main_axis = match parent_layout {
        ParentLayout::Row => attr_is(node, "w", "fill"),
        ParentLayout::Column => attr_is(node, "h", "fill"),
        // A grid child has no main axis to grow along; `fill` resolves to a
        // percentage of the track it occupies, via `dimension_attr`.
        ParentLayout::Grid | ParentLayout::LegacyGrid => false,
        // A positioned child has no main axis to grow along either; `fill`
        // resolves to a percentage of the container, via `dimension_attr`.
        ParentLayout::Positioned => false,
    };
    if fills_main_axis {
        style.flex_grow = 1.0;
        style.flex_shrink = 1.0;
        style.flex_basis = zero();
    }
    if let Some(mode) = grid::grid_mode(node, metadata) {
        grid::apply_container(&mut style, node, metadata, &mode);
    }
    if parent_layout == ParentLayout::Grid {
        grid::apply_placement(&mut style, node, metadata);
    }
    // A child is out of flow either because it says so, or because its parent
    // positions everything it holds.
    if is_absolute(node) || parent_layout == ParentLayout::Positioned {
        style.position = Position::Absolute;
        style.inset.left = number_attr(node, metadata, "x").map_or(auto(), length);
        style.inset.top = number_attr(node, metadata, "y").map_or(auto(), length);
    }
    style
}

/// Tags the layout engine understands. Anything else is laid out as a plain
/// block, which is rarely what the document intended.
const SUPPORTED_TAGS: &[&str] = &[
    "gui", "row", "col", "stack", "frame", "group", "grid", "rect", "line", "ellipse", "text",
    "img",
];

fn warn_on_unsupported_tag(node: &GuiNode) {
    if !SUPPORTED_TAGS.contains(&node.tag.as_str()) {
        eprintln!("warning: unsupported element tag <{}>", node.tag);
    }
}

fn display_for(node: &GuiNode, metadata: &GuiMetadata) -> Display {
    if grid::grid_mode(node, metadata).is_some() {
        return Display::Grid;
    }

    match node.tag.as_str() {
        "row" | "col" | "stack" | "frame" => Display::Flex,
        _ => Display::Block,
    }
}

fn flex_direction_for(node: &GuiNode) -> FlexDirection {
    // `<stack>` carries its axis in an attribute; every other container tag
    // implies one.
    if node.tag == "stack" {
        return match node.attributes.get("direction").map(String::as_str) {
            Some("horizontal") => FlexDirection::Row,
            _ => FlexDirection::Column,
        };
    }

    match node.tag.as_str() {
        "row" => FlexDirection::Row,
        _ => FlexDirection::Column,
    }
}

/// `aspect-ratio="16/9"`, or a bare number.
///
/// Taffy resolves the ratio itself, so this only has to read it. A ratio with
/// a zero or negative side would make the box collapse, so it is dropped.
fn aspect_ratio(node: &GuiNode, metadata: &GuiMetadata) -> Option<f32> {
    let raw = resolve_token(node.attributes.get("aspect-ratio")?, metadata);

    let ratio = match raw.split_once('/') {
        Some((width, height)) => {
            let width = parse_number(width.trim())?;
            let height = parse_number(height.trim())?;
            if height == 0.0 {
                return None;
            }
            width / height
        }
        None => parse_number(raw.trim())?,
    };

    (ratio > 0.0 && ratio.is_finite()).then_some(ratio)
}

/// Min/max constraints, read under the spec name with the short form kept as an
/// alias.
///
/// The spec (and kit) say `min-width`/`max-width`/`min-height`/`max-height`.
/// Real documents in the corpus also write `min-h`, and the intent there is
/// unambiguous, so both spellings are accepted with the spec name winning when
/// a node carries both.
fn constraint_attr(
    node: &GuiNode,
    metadata: &GuiMetadata,
    spec_name: &str,
    alias: &str,
) -> Dimension {
    if node.attributes.contains_key(spec_name) {
        dimension_attr(node, metadata, spec_name)
    } else {
        dimension_attr(node, metadata, alias)
    }
}

fn dimension_attr(node: &GuiNode, metadata: &GuiMetadata, name: &str) -> Dimension {
    let Some(raw) = node.attributes.get(name) else {
        return auto();
    };

    let resolved = resolve_token(raw, metadata);
    match resolved.as_str() {
        "fill" => percent(1.0),
        "hug" | "auto" => auto(),
        // `parse_number` drops a trailing `%`, which would quietly turn
        // `50%` into 50px, so percentages are handled before it is reached.
        value => match value.trim().strip_suffix('%') {
            Some(percentage) => percentage
                .trim()
                .parse::<f32>()
                .map_or(auto(), |value| percent(value / 100.0)),
            None => parse_number(value).map_or(auto(), length),
        },
    }
}

/// `gap="16"` is uniform; `gap="16 8"` is column gap then row gap.
///
/// `gap="auto"` means "push the children apart", handled as zero spacing plus
/// space-between justification.
fn gap_size(node: &GuiNode, metadata: &GuiMetadata) -> Size<LengthPercentage> {
    let Some(raw) = node.attributes.get("gap") else {
        return Size {
            width: zero(),
            height: zero(),
        };
    };

    let resolved = resolve_token(raw, metadata);
    let mut values = resolved.split_whitespace().map(|part| {
        if part == "auto" {
            return zero();
        }
        parse_number(part).map_or(zero(), length)
    });

    let column = values.next().unwrap_or(zero());
    let row = values.next().unwrap_or(column);
    Size {
        width: column,
        height: row,
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

fn text_context(node: &GuiNode, metadata: &GuiMetadata, ordinal: usize) -> Option<TextContext> {
    if node.tag != "text" {
        return None;
    }

    Some(TextContext {
        runs: resolve_text_runs(node, metadata),
        max_lines: max_text_lines(node, metadata),
        wrap: text::WrapOptions::new(
            node.attributes.get("white-space").map(String::as_str),
            node.attributes.get("text-wrap").map(String::as_str),
            node.attributes.get("word-break").map(String::as_str),
            number_attr(node, metadata, "paragraph-indent").unwrap_or(0.0),
        ),
        list_marker: crate::scene::list_marker_text(&node.attributes, metadata, ordinal),
        list_indent: crate::scene::list_indent(&node.attributes, metadata),
        leading_trim: node
            .attributes
            .get("leading-trim")
            .is_some_and(|value| value != "normal"),
    })
}

/// Resolves how many lines a `<text>` node may occupy.
///
/// An explicit `max-lines` wins over `truncate`, which on its own means a
/// single ellipsized line. `crate::scene` resolves this the same way, so the
/// height reserved here matches the lines that get painted.
fn max_text_lines(node: &GuiNode, metadata: &GuiMetadata) -> Option<usize> {
    let explicit = node
        .attributes
        .get("max-lines")
        .map(|value| resolve_token(value, metadata))
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|lines| *lines > 0);
    if explicit.is_some() {
        return explicit;
    }

    let truncates = node
        .attributes
        .get("truncate")
        .is_some_and(|value| value != "false")
        || node
            .attributes
            .get("overflow")
            .is_some_and(|value| value == "ellipsis");

    truncates.then_some(1)
}

fn measure_text(
    context: &TextContext,
    text_measurer: &dyn TextMeasurer,
    known_dimensions: Size<Option<f32>>,
    available_space: Size<AvailableSpace>,
) -> Size<f32> {
    let measure = |value: &str, style: usize| {
        let style = &context.runs[style].style;
        text_measurer.text_width(
            value,
            style.font_family.as_deref(),
            style.font_weight.as_deref(),
            style.font_style.as_deref(),
            style.font_size,
            FontAxes::from_style(
                style.font_stretch.as_deref(),
                style.font_optical_sizing.as_deref(),
                style.font_size,
            ),
        ) + value.chars().count() as f32 * style.letter_spacing
            + value.chars().filter(|ch| *ch == ' ').count() as f32 * style.word_spacing
    };

    // `MinContent` asks how narrow the text can get, which is a zero-width
    // wrap: every word lands on its own line and the widest one wins.
    let wrap_width = known_dimensions.width.or(match available_space.width {
        AvailableSpace::Definite(width) => Some(width),
        AvailableSpace::MinContent => Some(0.0),
        AvailableSpace::MaxContent => None,
    });

    let runs = context
        .runs
        .iter()
        .enumerate()
        .map(|(index, run)| text::Run {
            text: run.value.clone(),
            style: index,
        })
        .collect::<Vec<_>>();

    // A list item's marker takes room on the first line, and its indent takes
    // room from every line. Painting reserves the same, or a box would be
    // sized for one wrap and drawn with another.
    let marker_width = context
        .list_marker
        .as_deref()
        .map_or(0.0, |marker| measure(marker, 0));
    let mut wrap = context.wrap;
    wrap.indent += marker_width;
    let wrap_width = wrap_width.map(|width| (width - context.list_indent).max(1.0));

    let mut lines = text::wrap_runs(&runs, wrap_width, &measure, wrap);
    if let Some(max_lines) = context.max_lines {
        lines.truncate(max_lines.max(1));
    }

    // Each line is as tall as its tallest run, matching how painting stacks
    // them, so a resized segment reserves the room it needs.
    let height: f32 = lines
        .iter()
        .map(|line| {
            line.iter()
                .map(|run| context.runs[run.style].style.line_height)
                .fold(0.0_f32, f32::max)
                .max(context.runs[0].style.line_height)
        })
        .sum();

    // `leading-trim` takes the half-leading off the top of the block, so the
    // box is as tall as the text rather than as tall as its leading.
    let trim = if context.leading_trim {
        let style = &context.runs[0].style;
        text_measurer.leading_trim(
            style.font_family.as_deref(),
            style.font_weight.as_deref(),
            style.font_style.as_deref(),
            style.font_size,
            style.line_height,
        )
    } else {
        0.0
    };

    Size {
        width: known_dimensions.width.unwrap_or_else(|| {
            text::max_line_width(&lines, &measure) + marker_width + context.list_indent
        }),
        height: known_dimensions.height.unwrap_or((height - trim).max(0.0)),
    }
}

fn number_attr(node: &GuiNode, metadata: &GuiMetadata, name: &str) -> Option<f32> {
    node.attributes
        .get(name)
        .map(|value| resolve_token(value, metadata))
        .and_then(|value| parse_number(&value))
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

    /// `ApproxTextMeasurer` bills 0.55 * font size per character, so at
    /// `font-size="10"` every character is 5.5px wide.
    fn layout_of(xml: &str) -> LayoutBox {
        let document = parse_gui_xml(xml).expect("valid gui");
        compute_taffy_layout(&document).expect("layout computes")
    }

    #[test]
    fn wrapped_text_reserves_height_for_every_line() {
        // "aaaa bbbb" is 49.5px and fits; adding " cccc" reaches 77px and wraps.
        let layout = layout_of(
            r#"
            <gui version="0.2">
              <col w="50">
                <text value="aaaa bbbb cccc" font-size="10" line-height="12" />
              </col>
            </gui>
            "#,
        );

        assert_eq!(layout.children[0].rect.height, 24.0);
        assert_eq!(layout.rect.height, 24.0);
    }

    #[test]
    fn max_lines_caps_the_reserved_height() {
        let layout = layout_of(
            r#"
            <gui version="0.2">
              <col w="50">
                <text value="aaaa bbbb cccc" font-size="10" line-height="12" max-lines="1" />
              </col>
            </gui>
            "#,
        );

        assert_eq!(layout.children[0].rect.height, 12.0);
    }

    #[test]
    fn truncate_alone_reserves_a_single_line() {
        let layout = layout_of(
            r#"
            <gui version="0.2">
              <col w="50">
                <text value="aaaa bbbb cccc" font-size="10" line-height="12" truncate />
              </col>
            </gui>
            "#,
        );

        assert_eq!(layout.children[0].rect.height, 12.0);
    }

    #[test]
    fn hard_newlines_break_without_a_width_constraint() {
        let layout = layout_of(
            r#"
            <gui version="0.2">
              <col w="500">
                <text value="aa&#10;bb" font-size="10" line-height="12" />
              </col>
            </gui>
            "#,
        );

        assert_eq!(layout.children[0].rect.height, 24.0);
    }

    #[test]
    fn text_keeps_the_exact_width_it_was_measured_at() {
        // Rounding this down would make painting re-wrap and drop a word.
        let layout = layout_of(
            r#"
            <gui version="0.2">
              <col>
                <text value="Hello" font-size="10" />
              </col>
            </gui>
            "#,
        );

        assert_eq!(layout.children[0].rect.width, 27.5);
    }

    #[test]
    fn truncate_and_max_lines_agree_on_the_line_count() {
        // `truncate` used to force a single painted line while layout reserved
        // room for `max-lines`, leaving an empty gap under the text.
        let layout = layout_of(
            r#"
            <gui version="0.2">
              <col w="50">
                <text value="aaaa bbbb cccc dddd eeee ffff gggg hhhh" font-size="10" line-height="12" truncate max-lines="3" />
              </col>
            </gui>
            "#,
        );

        assert_eq!(layout.children[0].rect.height, 36.0);
    }

    #[test]
    fn unsupported_tags_are_still_laid_out() {
        let layout = layout_of(
            r#"
            <gui version="0.2">
              <col w="40" h="20">
                <marquee w="10" h="10" />
              </col>
            </gui>
            "#,
        );

        assert_eq!(layout.children[0].rect.width, 10.0);
    }

    #[test]
    fn fill_width_inside_a_column_does_not_collapse_heights() {
        // `w="fill"` used to set flex-basis 0 regardless of the parent's
        // direction, which zeroed the vertical contribution of every box in a
        // column and squashed fixed-height rows.
        let layout = layout_of(
            r#"
            <gui version="0.2">
              <col w="200">
                <col w="fill">
                  <row w="fill" h="56"><rect w="10" h="10" /></row>
                  <row w="fill" h="56"><rect w="10" h="10" /></row>
                </col>
              </col>
            </gui>
            "#,
        );

        let inner = &layout.children[0];
        assert_eq!(inner.rect.width, 200.0);
        assert_eq!(inner.children[0].rect.height, 56.0);
        assert_eq!(inner.children[1].rect.height, 56.0);
        assert_eq!(inner.rect.height, 112.0);
    }

    #[test]
    fn fill_width_inside_a_row_still_takes_the_free_space() {
        let layout = layout_of(
            r#"
            <gui version="0.2">
              <row w="200" h="20">
                <rect w="40" h="10" />
                <rect w="fill" h="10" />
              </row>
            </gui>
            "#,
        );

        assert_eq!(layout.children[1].rect.width, 160.0);
    }

    #[test]
    fn a_declared_size_is_not_shrunk_to_fit() {
        // Design-tool semantics: a fixed box overflows its parent rather than
        // being compressed.
        let layout = layout_of(
            r#"
            <gui version="0.2">
              <col w="100" h="40">
                <rect w="10" h="30" />
                <rect w="10" h="30" />
              </col>
            </gui>
            "#,
        );

        assert_eq!(layout.children[0].rect.height, 30.0);
        assert_eq!(layout.children[1].rect.height, 30.0);
    }

    #[test]
    fn segments_measure_as_one_continuous_string() {
        // Both spellings must size identically: styling changes glyphs, not
        // how much text there is.
        let plain = layout_of(
            r#"
            <gui version="0.2">
              <col>
                <text value="one two" font-size="10" />
              </col>
            </gui>
            "#,
        );
        let segmented = layout_of(
            r#"
            <gui version="0.2">
              <col>
                <text font-size="10">
                  <segment value="one " />
                  <segment value="two" font-weight="700" />
                </text>
              </col>
            </gui>
            "#,
        );

        assert_eq!(
            segmented.children[0].rect.width,
            plain.children[0].rect.width
        );
    }

    #[test]
    fn a_larger_segment_raises_the_line_height() {
        let layout = layout_of(
            r#"
            <gui version="0.2">
              <col>
                <text font-size="10" line-height="12">
                  <segment value="small " />
                  <segment value="big" font-size="30" />
                </text>
              </col>
            </gui>
            "#,
        );

        // The 30px segment's own line height (30 * 1.2) wins over the node's 12.
        assert_eq!(layout.children[0].rect.height, 36.0);
    }

    #[test]
    fn text_wraps_across_segment_boundaries() {
        let layout = layout_of(
            r#"
            <gui version="0.2">
              <col w="50">
                <text font-size="10" line-height="12">
                  <segment value="aaaa bbbb " />
                  <segment value="cccc dddd" font-weight="700" />
                </text>
              </col>
            </gui>
            "#,
        );

        // Same wrapping as the equivalent plain string: two lines, not one per
        // segment.
        assert_eq!(layout.children[0].rect.height, 24.0);
    }

    #[test]
    fn segments_are_content_not_boxes() {
        let layout = layout_of(
            r#"
            <gui version="0.2">
              <col>
                <text font-size="10">
                  <segment value="one" />
                  <segment value="two" />
                </text>
              </col>
            </gui>
            "#,
        );

        // They ride along for the scene to read, but contribute no geometry.
        let text = &layout.children[0];
        assert!(text.rect.width > 0.0);
        assert!(text.children.iter().all(|child| child.tag == "segment"));
        assert!(text.children.iter().all(|child| child.rect.width == 0.0));
    }

    // ── RFC-0032 grid ────────────────────────────────────────────────────

    #[test]
    fn a_legacy_stack_grid_reads_its_rows_and_gaps() {
        // `<stack direction="grid">` predates `<grid>`: it takes counts rather
        // than track templates, and its gaps have their own names.
        let layout = layout_of(
            r#"
            <gui version="0.2">
              <stack direction="grid" grid-columns="2" grid-rows="2"
                     grid-col-gap="10" grid-row-gap="20" w="210" h="120">
                <rect />
                <rect />
                <rect />
                <rect />
              </stack>
            </gui>
            "#,
        );

        let at = |index: usize| {
            let rect = layout.children[index].rect;
            (rect.x, rect.y)
        };

        assert_eq!(at(0), (0.0, 0.0));
        assert_eq!(at(1), (110.0, 0.0), "one column plus the 10px column gap");
        assert_eq!(at(2), (0.0, 70.0), "one row plus the 20px row gap");
        assert_eq!(at(3), (110.0, 70.0));
    }

    #[test]
    fn a_grid_child_spans_the_rows_it_asks_for() {
        // `col-span` has a test through `grid_ranges_are_inclusive`; this is
        // its vertical twin.
        let layout = layout_of(
            r#"
            <gui version="0.2">
              <grid cols="2" rows="2" w="200" h="200">
                <rect row-span="2" />
                <rect />
                <rect />
              </grid>
            </gui>
            "#,
        );

        assert_eq!(
            layout.children[0].rect.height, 200.0,
            "the spanning child covers both rows"
        );
        assert_eq!(layout.children[1].rect.height, 100.0);
    }

    #[test]
    fn track_grid_splits_equal_columns() {
        let layout = layout_of(
            r#"
            <gui version="0.2">
              <grid cols="3" w="300" h="50">
                <rect h="10" gc="1" />
                <rect h="10" gc="2" />
                <rect h="10" gc="3" />
              </grid>
            </gui>
            "#,
        );

        assert_eq!(layout.children[0].rect.x, 0.0);
        assert_eq!(layout.children[1].rect.x, 100.0);
        assert_eq!(layout.children[2].rect.x, 200.0);
    }

    #[test]
    fn track_grid_mixes_pixel_and_fraction_tracks() {
        // A bare integer inside a track list is a pixel size, not a count.
        let layout = layout_of(
            r#"
            <gui version="0.2">
              <grid cols="200 1fr" w="500" h="50">
                <rect h="10" gc="1" />
                <rect h="10" gc="2" />
              </grid>
            </gui>
            "#,
        );

        assert_eq!(layout.children[1].rect.x, 200.0);
    }

    #[test]
    fn track_grid_fills_responsive_columns() {
        // "fill 200" is repeat(auto-fill, minmax(200px, 1fr)): 500px fits two.
        let layout = layout_of(
            r#"
            <gui version="0.2">
              <grid cols="fill 200" w="500" h="50">
                <rect h="10" gc="1/-1" />
                <rect h="10" gc="2" />
              </grid>
            </gui>
            "#,
        );

        assert_eq!(layout.children[0].rect.width, 500.0);
        assert_eq!(layout.children[1].rect.x, 250.0);
    }

    #[test]
    fn grid_ranges_are_inclusive() {
        // gc="2/3" is columns 2 and 3, i.e. CSS 2 / 4.
        let layout = layout_of(
            r#"
            <gui version="0.2">
              <grid cols="4" w="400" h="50">
                <rect h="10" gc="2/3" />
              </grid>
            </gui>
            "#,
        );

        assert_eq!(layout.children[0].rect.x, 100.0);
        assert_eq!(layout.children[0].rect.width, 200.0);
    }

    #[test]
    fn a_negative_range_end_reaches_the_last_column() {
        let layout = layout_of(
            r#"
            <gui version="0.2">
              <grid cols="4" w="400" h="50">
                <rect h="10" gc="1/-1" />
              </grid>
            </gui>
            "#,
        );

        assert_eq!(layout.children[0].rect.width, 400.0);
    }

    #[test]
    fn a_range_fills_its_span_only_without_an_explicit_size() {
        let layout = layout_of(
            r#"
            <gui version="0.2">
              <grid cols="4" w="400" h="50">
                <rect h="10" gc="1/2" />
                <rect h="10" gc="3/4" w="30" />
              </grid>
            </gui>
            "#,
        );

        assert_eq!(layout.children[0].rect.width, 200.0, "range fills the span");
        assert_eq!(layout.children[1].rect.width, 30.0, "explicit w wins");
    }

    #[test]
    fn col_span_spans_from_the_current_position() {
        let layout = layout_of(
            r#"
            <gui version="0.2">
              <grid cols="4" w="400" h="50">
                <rect h="10" gc="2" col-span="2" />
              </grid>
            </gui>
            "#,
        );

        assert_eq!(layout.children[0].rect.x, 100.0);
        assert_eq!(layout.children[0].rect.width, 200.0);
    }

    #[test]
    fn unit_grid_places_children_on_a_snapped_coordinate_space() {
        // unit=8 over 320x400 is a 40 by 50 space; column 13 starts at 96px.
        let layout = layout_of(
            r#"
            <gui version="0.2">
              <grid unit="8" w="320" h="400">
                <rect gc="13" gr="9" w="128" h="128" />
              </grid>
            </gui>
            "#,
        );

        let child = &layout.children[0];
        assert_eq!(child.rect.x, 96.0);
        assert_eq!(child.rect.y, 64.0);
        assert_eq!(child.rect.width, 128.0);
        assert_eq!(child.rect.height, 128.0);
    }

    #[test]
    fn unit_grid_ranges_fill_their_span() {
        let layout = layout_of(
            r#"
            <gui version="0.2">
              <grid unit="8" w="320" h="400">
                <rect gc="1/40" gr="1/14" />
              </grid>
            </gui>
            "#,
        );

        let child = &layout.children[0];
        assert_eq!(child.rect.width, 320.0);
        assert_eq!(child.rect.height, 112.0);
    }

    #[test]
    fn unit_grid_children_may_overlap() {
        // Document order is paint order, so overlapping is how layering works.
        let layout = layout_of(
            r#"
            <gui version="0.2">
              <grid unit="8" w="320" h="320">
                <rect gc="5" gr="5" w="64" h="64" />
                <rect gc="5" gr="5" w="32" h="32" />
              </grid>
            </gui>
            "#,
        );

        assert_eq!(layout.children[0].rect.x, layout.children[1].rect.x);
        assert_eq!(layout.children[0].rect.y, layout.children[1].rect.y);
    }

    #[test]
    fn legacy_columns_attribute_still_works() {
        let layout = layout_of(
            r#"
            <gui version="0.2">
              <grid columns="2" w="200" h="50">
                <rect h="10" />
                <rect h="10" />
              </grid>
            </gui>
            "#,
        );

        assert_eq!(layout.children[0].rect.x, 0.0);
        assert_eq!(layout.children[1].rect.x, 100.0);
    }

    #[test]
    fn two_value_gap_sets_column_then_row_spacing() {
        let layout = layout_of(
            r#"
            <gui version="0.2">
              <grid cols="2" w="216" gap="16 8">
                <rect h="10" />
                <rect h="10" />
                <rect h="10" />
              </grid>
            </gui>
            "#,
        );

        // Columns: (216 - 16) / 2 = 100 each, second starting at 116.
        assert_eq!(layout.children[1].rect.x, 116.0);
        // Third wraps to row two, 8px below the first row. The grid is left
        // to hug: a fixed height would stretch the auto rows to fill it, as
        // CSS `align-content: normal` does.
        assert_eq!(layout.children[2].rect.y, 18.0);
    }

    #[test]
    fn a_stack_takes_its_axis_from_its_direction() {
        // `<stack>` is the only container whose axis is an attribute rather
        // than the tag, and it silently laid out as a column before.
        let horizontal = layout_of(
            r#"
            <gui version="0.2">
              <stack direction="horizontal" w="200" h="40">
                <rect w="20" h="20" />
                <rect w="20" h="20" />
              </stack>
            </gui>
            "#,
        );
        assert_eq!(horizontal.children[1].rect.x, 20.0);
        assert_eq!(horizontal.children[1].rect.y, 0.0);

        let vertical = layout_of(
            r#"
            <gui version="0.2">
              <stack direction="vertical" w="200" h="40">
                <rect w="20" h="20" />
                <rect w="20" h="20" />
              </stack>
            </gui>
            "#,
        );
        assert_eq!(vertical.children[1].rect.x, 0.0);
        assert_eq!(vertical.children[1].rect.y, 20.0);
    }

    #[test]
    fn a_stack_can_be_a_grid_in_the_pre_rfc_spelling() {
        let layout = layout_of(
            r#"
            <gui version="0.2">
              <stack direction="grid" grid-columns="2" grid-col-gap="10" w="210" h="50">
                <rect h="10" />
                <rect h="10" />
              </stack>
            </gui>
            "#,
        );

        assert_eq!(layout.children[0].rect.x, 0.0);
        assert_eq!(layout.children[1].rect.x, 110.0);
    }

    #[test]
    fn min_and_max_constraints_clamp_boxes() {
        for (min_height, max_width) in [("min-height", "max-width"), ("min-h", "max-w")] {
            let layout = layout_of(&format!(
                r#"
            <gui version="0.2">
              <col w="100" {min_height}="150" {max_width}="80">
                <rect w="200" h="50" />
              </col>
            </gui>
            "#
            ));

            assert_eq!(layout.rect.width, 80.0, "{max_width}");
            assert_eq!(layout.rect.height, 150.0, "{min_height}");
        }
    }

    #[test]
    fn percentage_constraints_resolve_against_the_parent() {
        for min_width in ["min-width", "min-w"] {
            let layout = layout_of(&format!(
                r#"
            <gui version="0.2">
              <col w="400">
                <col w="100" {min_width}="50%" />
              </col>
            </gui>
            "#
            ));

            assert_eq!(layout.children[0].rect.width, 200.0, "{min_width}");
        }
    }

    #[test]
    fn the_spec_constraint_name_wins_over_the_short_alias() {
        let layout = layout_of(
            r#"
            <gui version="0.2">
              <col w="100" max-width="80" max-w="40">
                <rect w="200" h="50" />
              </col>
            </gui>
            "#,
        );

        assert_eq!(layout.rect.width, 80.0);
    }

    #[test]
    fn aspect_ratio_sizes_the_axis_left_open() {
        let layout = layout_of(
            r#"
            <gui version="0.2">
              <col w="200" h="200">
                <rect w="160" aspect-ratio="16/9" />
              </col>
            </gui>
            "#,
        );

        assert_eq!(layout.children[0].rect.width, 160.0);
        assert_eq!(layout.children[0].rect.height, 90.0);
    }

    #[test]
    fn aspect_ratio_takes_a_bare_number_too() {
        // A row, so width is the main axis and stays free for the ratio to
        // decide; across a column's cross axis the stretch would win instead.
        let layout = layout_of(
            r#"
            <gui version="0.2">
              <row w="200" h="200">
                <rect h="60" aspect-ratio="2" />
              </row>
            </gui>
            "#,
        );

        assert_eq!(layout.children[0].rect.width, 120.0);
    }

    #[test]
    fn a_degenerate_aspect_ratio_is_ignored() {
        // A zero or negative ratio would collapse the box rather than shape it.
        let layout = layout_of(
            r#"
            <gui version="0.2">
              <col w="200" h="200">
                <rect w="80" h="40" aspect-ratio="16/0" />
                <rect w="80" h="40" aspect-ratio="-2" />
              </col>
            </gui>
            "#,
        );

        assert_eq!(layout.children[0].rect.height, 40.0);
        assert_eq!(layout.children[1].rect.height, 40.0);
    }

    #[test]
    fn nowrap_text_measures_as_one_line() {
        let wrapped = layout_of(
            r#"
            <gui version="0.2">
              <col w="50">
                <text value="aaaa bbbb cccc" font-size="10" line-height="12" />
              </col>
            </gui>
            "#,
        );
        let flat = layout_of(
            r#"
            <gui version="0.2">
              <col w="50">
                <text value="aaaa bbbb cccc" font-size="10" line-height="12"
                      white-space="nowrap" />
              </col>
            </gui>
            "#,
        );

        assert!(wrapped.children[0].rect.height > 12.0, "wraps by default");
        assert_eq!(flat.children[0].rect.height, 12.0, "one line when nowrap");
    }

    #[test]
    fn paragraph_spacing_adds_room_after_the_block() {
        let layout = layout_of(
            r#"
            <gui version="0.2">
              <col w="200">
                <text value="One" font-size="10" line-height="12"
                      paragraph-spacing="8" />
                <text value="Two" font-size="10" line-height="12" />
              </col>
            </gui>
            "#,
        );

        // The second block starts a line height plus the spacing below.
        assert_eq!(layout.children[1].rect.y, 20.0);
    }

    #[test]
    fn word_spacing_widens_measurement() {
        let layout = layout_of(
            r#"
            <gui version="0.2">
              <row>
                <text value="a b c" font-size="10" word-spacing="6" />
              </row>
            </gui>
            "#,
        );
        let plain = layout_of(
            r#"
            <gui version="0.2">
              <row>
                <text value="a b c" font-size="10" />
              </row>
            </gui>
            "#,
        );

        // Two spaces, six extra pixels each.
        assert_eq!(
            layout.children[0].rect.width - plain.children[0].rect.width,
            12.0
        );
    }

    #[test]
    fn a_frame_positions_its_children_rather_than_stacking_them() {
        // Two overlapping children at the same origin: in a frame they sit on
        // top of each other, which is what a card overlay needs.
        let layout = layout_of(
            r#"
            <gui version="0.2">
              <frame w="200" h="100">
                <rect x="0" y="0" w="200" h="100" />
                <text x="16" y="60" w="120" h="20" value="Overlay" />
              </frame>
            </gui>
            "#,
        );

        assert_eq!(
            (layout.children[0].rect.x, layout.children[0].rect.y),
            (0.0, 0.0)
        );
        assert_eq!(
            (layout.children[1].rect.x, layout.children[1].rect.y),
            (16.0, 60.0),
            "the overlay sits where it says, not below its sibling"
        );
    }

    #[test]
    fn a_group_positions_its_children_too() {
        // The spec: "children are absolutely positioned relative to the group
        // origin".
        let layout = layout_of(
            r#"
            <gui version="0.2">
              <frame w="200" h="100">
                <group x="0" y="0" w="200" h="100">
                  <rect x="10" y="10" w="20" h="20" />
                  <rect x="10" y="50" w="20" h="20" />
                </group>
              </frame>
            </gui>
            "#,
        );

        let group = &layout.children[0];
        assert_eq!(group.children[0].rect.y, 10.0);
        assert_eq!(group.children[1].rect.y, 50.0);
    }

    #[test]
    fn a_stack_still_flows_its_children() {
        // The change is to frames and groups only: a col still stacks.
        let layout = layout_of(
            r#"
            <gui version="0.2">
              <col w="200" h="100">
                <rect w="20" h="20" />
                <rect w="20" h="20" />
              </col>
            </gui>
            "#,
        );

        assert_eq!(layout.children[0].rect.y, 0.0);
        assert_eq!(
            layout.children[1].rect.y, 20.0,
            "the second child follows the first"
        );
    }

    #[test]
    fn a_frame_child_without_a_position_sits_at_the_origin() {
        let layout = layout_of(
            r#"
            <gui version="0.2">
              <frame w="200" h="100">
                <rect w="20" h="20" />
              </frame>
            </gui>
            "#,
        );

        assert_eq!(
            (layout.children[0].rect.x, layout.children[0].rect.y),
            (0.0, 0.0)
        );
    }

    #[test]
    fn max_height_clamps_a_box() {
        // The sibling of `min_and_max_constraints_clamp_boxes`, which covers
        // the other three constraints.
        for name in ["max-height", "max-h"] {
            let layout = layout_of(&format!(
                r#"
                <gui version="0.2">
                  <col w="100" {name}="40">
                    <rect w="10" h="200" />
                  </col>
                </gui>
                "#
            ));

            assert_eq!(layout.rect.height, 40.0, "{name}");
        }
    }

    #[test]
    fn bottom_padding_is_read_like_the_other_three_sides() {
        let layout = layout_of(
            r#"
            <gui version="0.2">
              <col pb="12">
                <rect w="10" h="10" />
              </col>
            </gui>
            "#,
        );

        assert_eq!(layout.rect.height, 22.0, "the box grows by its padding");
        assert_eq!(layout.children[0].rect.y, 0.0, "which sits below the child");
    }

    #[test]
    fn every_padding_side_is_wired_to_its_own_edge() {
        // `pt`/`pr`/`pb`/`pl` are four near-identical readers, which is where a
        // copy-paste slip would sit unnoticed.
        let layout = layout_of(
            r#"
            <gui version="0.2">
              <col w="100" h="100" pt="4" pr="8" pb="16" pl="32">
                <rect w="10" h="10" />
              </col>
            </gui>
            "#,
        );

        let child = &layout.children[0];
        assert_eq!((child.rect.x, child.rect.y), (32.0, 4.0), "left and top");

        let hugging = layout_of(
            r#"
            <gui version="0.2">
              <col pt="4" pr="8" pb="16" pl="32">
                <rect w="10" h="10" />
              </col>
            </gui>
            "#,
        );
        assert_eq!(hugging.rect.width, 50.0, "10 + 32 left + 8 right");
        assert_eq!(hugging.rect.height, 30.0, "10 + 4 top + 16 bottom");
    }

    #[test]
    fn the_text_wrap_attribute_reaches_the_wrapper() {
        // `WrapOptions::new` is unit-tested; this is the attribute wiring.
        let wrapped = layout_of(
            r#"
            <gui version="0.2">
              <col w="50">
                <text value="aaaa bbbb cccc" font-size="10" line-height="12" />
              </col>
            </gui>
            "#,
        );
        let flat = layout_of(
            r#"
            <gui version="0.2">
              <col w="50">
                <text value="aaaa bbbb cccc" font-size="10" line-height="12"
                      text-wrap="nowrap" />
              </col>
            </gui>
            "#,
        );

        assert!(wrapped.children[0].rect.height > 12.0);
        assert_eq!(flat.children[0].rect.height, 12.0);
    }

    #[test]
    fn the_word_break_attribute_reaches_the_wrapper() {
        // `break-all` lets a long word split, so it stops overflowing and the
        // box needs more lines for it.
        let normal = layout_of(
            r#"
            <gui version="0.2">
              <col w="40">
                <text value="aaaaaaaaaaaaaaaa" font-size="10" line-height="12" />
              </col>
            </gui>
            "#,
        );
        let broken = layout_of(
            r#"
            <gui version="0.2">
              <col w="40">
                <text value="aaaaaaaaaaaaaaaa" font-size="10" line-height="12"
                      word-break="break-all" />
              </col>
            </gui>
            "#,
        );

        assert_eq!(normal.children[0].rect.height, 12.0, "one overflowing line");
        assert!(
            broken.children[0].rect.height > 12.0,
            "break-all splits it across lines"
        );
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
