use crate::{
    text_style::{resolve_text_runs, resolve_token, TextDecoration},
    GuiDocument, GuiMetadata, LayoutBox, LayoutRect,
};
use serde::{Deserialize, Serialize};

/// Which rule decides the inside of a shape's path.
///
/// The two rules only disagree where a path crosses itself, and neither a
/// `<rect>` nor an `<ellipse>` does — the spec has no element that carries an
/// arbitrary path. So this changes nothing about what the current shapes look
/// like. It is read and handed to the rasteriser anyway, rather than dropped,
/// because the alternative is silently ignoring a property the document set,
/// and because the shapes are the only thing keeping the two rules equal here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ShapeFillRule {
    NonZero,
    EvenOdd,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Scene {
    pub name: Option<String>,
    pub root: SceneNode,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SceneNode {
    pub tag: String,
    /// The layer name the source design gave this node.
    ///
    /// It paints nothing. It is carried because it is the only handle a
    /// consumer of the scene has on which node is which — a diagnostic naming
    /// the box it is complaining about, a viewer's layer list, a diff between
    /// two versions of a document — and dropping it here would leave them
    /// pointing at a tag and a rectangle.
    pub name: Option<String>,
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
    /// How the node composites against what is already painted behind it,
    /// as in CSS `mix-blend-mode`. `None` is the normal mode.
    pub blend: Option<String>,
    /// Whether the node is its own stacking context, so a descendant's blend
    /// mode sees only this subtree, as in CSS `isolation: isolate`.
    pub isolation: bool,
    /// A CSS `filter` string, e.g. `brightness(1.2) contrast(0.9)`.
    pub filter: Option<String>,
    /// Whether this node is its parent's mask rather than something to draw.
    ///
    /// The first masking child shapes its parent; it is kept in the tree so a
    /// consumer can see what the shape was, but it is not painted.
    pub mask: bool,
    /// An image mask hoisted onto the node, as `<group mask-src="...">`.
    pub image_mask: Option<ImageMask>,
    /// A CSS `clip-path` string, e.g. `circle(40% at 50% 50%)`.
    pub clip_path: Option<String>,
    /// The node's own transform, if it declares one.
    pub transform: Option<Transform2D>,
    /// A link target. It has nothing to paint in a still frame, but an
    /// interactive consumer of the scene needs it, so it is carried rather
    /// than dropped.
    pub href: Option<String>,
    /// `fill-rule`, for the shapes that take one.
    pub fill_rule: ShapeFillRule,
    /// `border-image`: an image drawn in place of the border's own colour,
    /// filling the ring the border occupies.
    pub border_image: Option<String>,
    pub opacity: f32,
    /// Whether the node paints.
    ///
    /// `visible="false"` is CSS `visibility: hidden`, not `display: none`: the
    /// node keeps the space it was laid out into, so everything around it
    /// stays where it was, and only its own paint is skipped. It inherits, and
    /// a descendant can take itself back out of it with `visible="true"`.
    pub visible: bool,
    /// Whether the node clips its children horizontally and vertically.
    ///
    /// Both axes together clip to the node's own shape, rounded corners
    /// included. One axis alone can only clip to a band, because the other
    /// direction is unbounded.
    pub clip_x: bool,
    pub clip_y: bool,
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

/// A node's 2D transform, already resolved to numbers.
///
/// The parts are kept separate rather than pre-multiplied into a matrix so a
/// consumer can still see what the document asked for. The painter composes
/// them in CSS's order: rotate, flip, scale, skew.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Transform2D {
    /// Degrees, clockwise, as in CSS `rotate()`.
    pub rotation: f32,
    pub scale_x: f32,
    pub scale_y: f32,
    /// Degrees, as in CSS `skewX()` and `skewY()`.
    pub skew_x: f32,
    pub skew_y: f32,
    /// The pivot, in pixels from the node's top-left corner.
    pub origin_x: f32,
    pub origin_y: f32,
}

impl Transform2D {
    /// Whether the transform would move anything.
    pub fn is_identity(&self) -> bool {
        self.rotation == 0.0
            && self.scale_x == 1.0
            && self.scale_y == 1.0
            && self.skew_x == 0.0
            && self.skew_y == 0.0
    }
}

/// An image used as a mask, from `mask-src` and friends.
///
/// The source is drawn once, unscaled beyond its declared size and not
/// repeated, exactly as kit's `mask-repeat: no-repeat` does.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImageMask {
    pub src: String,
    pub x: f32,
    pub y: f32,
    /// Defaults to the node's own size when the document leaves it out.
    pub width: Option<f32>,
    pub height: Option<f32>,
    /// `alpha` (the default) or `luminance`.
    pub mode: String,
    /// `add`, `subtract`, `intersect` or `exclude`.
    pub composite: String,
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
    /// `line-height` as declared; `None` is `normal`, resolved by painting
    /// from the face's own metric.
    pub line_height: Option<f32>,
    pub letter_spacing: f32,
    pub color: Option<String>,
    pub font_stretch: Option<String>,
    pub font_optical_sizing: Option<String>,
    /// `font-variation`, as a CSS `font-variation-settings` string.
    pub font_variation: Option<String>,
    pub font_smoothing: Option<String>,
    /// Extra space added to each space character, from `word-spacing`.
    pub word_spacing: f32,
    /// Pixels this run's baseline moves up, from `baseline-shift`.
    pub baseline_shift: f32,
    /// The rule drawn through this run, if any.
    pub decoration: Option<TextDecoration>,
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
        /// `text-rendering`: which way the rasteriser is asked to lean when
        /// speed and fidelity disagree.
        text_rendering: Option<String>,
        /// `writing-mode`: `horizontal-tb`, or one of the two vertical modes,
        /// which turn the block on its side.
        writing_mode: Option<String>,
        /// `white-space`, `text-wrap` and `word-break`, which decide where
        /// lines may break.
        white_space: Option<String>,
        text_wrap: Option<String>,
        word_break: Option<String>,
        /// First-line indent, from `paragraph-indent`.
        paragraph_indent: f32,
        /// The marker drawn before the first line, from `list`, `list-level`
        /// and `list-marker`. Already resolved to the string to draw, so a
        /// decimal item carries its own number.
        list_marker: Option<String>,
        /// Left indent for the whole block, from `list-level`.
        list_indent: f32,
        /// `top` (the default), `center` or `bottom`: where a text block that
        /// is shorter than its box sits inside it.
        vertical_align: Option<String>,
        /// `leading-trim`: `cap-height` pulls the block's top edge down to the
        /// cap height and its bottom up to the baseline.
        leading_trim: Option<String>,
    },
    Image {
        src: String,
        fit: Option<String>,
        /// Where the image sits in its box when `fit` leaves it a choice.
        object_position: Option<String>,
        /// `auto`, `smooth`, `pixelated` or `crisp-edges`.
        image_rendering: Option<String>,
    },
}

pub fn build_scene(document: &GuiDocument, layout: &LayoutBox) -> Scene {
    Scene {
        name: document.name.clone(),
        root: build_scene_node(layout, &document.metadata, 1, true),
    }
}

/// Builds one node.
///
/// `ordinal` is the node's position among its list-item siblings, counting
/// from 1. Only a `list="decimal"` node uses it, but it can only be known
/// from the parent, so it is passed down rather than looked up.
///
/// `inherited_visible` is whether the parent chain paints. `visible` inherits
/// like the CSS property it names, so it is resolved here rather than at paint
/// time, where the ancestors are no longer in hand.
fn build_scene_node(
    layout: &LayoutBox,
    metadata: &GuiMetadata,
    ordinal: usize,
    inherited_visible: bool,
) -> SceneNode {
    let visible = visibility_of(layout, inherited_visible);
    SceneNode {
        tag: layout.tag.clone(),
        name: attr(layout, "name").map(ToOwned::to_owned),
        bounds: layout.rect,
        fills: fills_for(layout, metadata),
        borders: borders_for(layout, metadata),
        outline: outline_for(layout, metadata),
        radius: attr(layout, "radius")
            .map(|value| resolve_token(value, metadata))
            .and_then(|value| parse_number(&value)),
        corner_smoothing: corner_smoothing_for(layout, metadata),
        blend: attr(layout, "blend")
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty() && value != "normal"),
        isolation: attr(layout, "isolation").is_some_and(|value| value != "false"),
        filter: attr(layout, "filter")
            .map(|value| resolve_token(value, metadata))
            .filter(|value| !value.trim().is_empty() && value.trim() != "none"),
        href: attr(layout, "href").map(ToOwned::to_owned),
        border_image: attr(layout, "border-image")
            .map(|value| resolve_token(value, metadata))
            .filter(|value| !value.trim().is_empty() && value.trim() != "none"),
        fill_rule: match attr(layout, "fill-rule").map(str::trim) {
            Some("evenodd") => ShapeFillRule::EvenOdd,
            _ => ShapeFillRule::NonZero,
        },
        transform: transform_for(layout, metadata),
        mask: attr(layout, "mask").is_some_and(|value| value != "false"),
        image_mask: image_mask_for(layout, metadata),
        clip_path: attr(layout, "clip-path")
            .map(|value| resolve_token(value, metadata))
            .filter(|value| !value.trim().is_empty() && value.trim() != "none"),
        opacity: attr(layout, "opacity")
            .and_then(parse_number)
            .unwrap_or(1.0),
        visible,
        clip_x: clips_axis(layout, "overflow-x"),
        clip_y: clips_axis(layout, "overflow-y"),
        effects: effects_for(layout, metadata),
        content: content_for(layout, metadata, ordinal),
        children: paint_ordered_children(layout, metadata, visible),
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

    // A named `fill-style` resolves to a colour, so it stands in for the
    // shorthand rather than sitting alongside it.
    attr(layout, "fill")
        .or_else(|| attr(layout, "color"))
        .or_else(|| fill_style_value(layout, metadata))
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

    // An `<appearance>` border is a complete description, which is why it
    // returns above without consulting anything else. What is left is the
    // node's own border: the `border` shorthand, the four longhands, or both.
    node_border(layout, metadata).into_iter().collect()
}

/// The border a node declares on itself.
///
/// `border` sets every part at once and each longhand replaces one of them,
/// so the shorthand is read first and then overridden — the same way the
/// shorthand and longhands relate in CSS, and the same way `gap` and
/// `row-gap`/`col-gap` relate a few properties over.
///
/// A border needs a colour to be drawn at all, so a node that gives a width
/// and no colour paints nothing, exactly as an `<appearance><border>` without
/// one does. A node that gives a colour and no width gets a 1px border, which
/// is the width an `<appearance><border>` assumes when it omits `w`.
fn node_border(layout: &LayoutBox, metadata: &GuiMetadata) -> Option<Border> {
    let value = |name: &str| attr(layout, name).map(|value| resolve_token(value, metadata));
    let shorthand = value("border").and_then(|value| parse_border(&value));

    let longhand_color = value("border-color");
    let longhand_width = value("border-width").and_then(|value| parse_number(&value));
    let longhand_style = value("border-style");
    let longhand_align = value("border-align");

    // Nothing declared, nothing to draw.
    if shorthand.is_none()
        && longhand_color.is_none()
        && longhand_width.is_none()
        && longhand_style.is_none()
        && longhand_align.is_none()
    {
        return None;
    }

    let color = longhand_color.or_else(|| shorthand.as_ref().map(|border| border.color.clone()))?;

    let widths = match longhand_width {
        Some(width) => BorderWidths::uniform(width),
        None => shorthand
            .as_ref()
            .map(|border| border.widths)
            .unwrap_or_else(|| BorderWidths::uniform(1.0)),
    };

    Some(Border {
        width: widths
            .top
            .max(widths.right)
            .max(widths.bottom)
            .max(widths.left),
        widths,
        color,
        style: longhand_style
            .or_else(|| shorthand.as_ref().map(|border| border.style.clone()))
            .unwrap_or_else(|| "solid".to_owned()),
        align: longhand_align
            .or_else(|| shorthand.as_ref().map(|border| border.align.clone()))
            .unwrap_or_else(|| "center".to_owned()),
    })
}

/// Where a `<text>` node's lines sit in its box.
///
/// A declared `align` is the answer. With none, the base `direction` decides,
/// which is what CSS does through the initial `text-align: start`: a run of
/// right-to-left text starts at the right edge of its box.
///
/// Note what this does *not* do. Setting `direction="rtl"` does not reorder
/// the characters within a line: that is the Unicode bidirectional algorithm,
/// this renderer does not run it, and the glyphs are drawn in the order the
/// document stores them. For text already stored in visual order — which is
/// what a design tool exports — the edge it starts from is the part that was
/// missing.
fn text_align_for(layout: &LayoutBox) -> Option<String> {
    if let Some(align) = attr(layout, "align") {
        return Some(align.to_owned());
    }

    match attr(layout, "direction").map(str::trim) {
        Some("rtl") => Some("right".to_owned()),
        _ => None,
    }
}

/// The colour a node picks up from `fill-style="name"`.
///
/// `<fill-style name="X" value="..." />` lands in `metadata.styles` with the
/// other named styles, because it is a bag of attributes like a text style is.
fn fill_style_value<'a>(layout: &'a LayoutBox, metadata: &'a GuiMetadata) -> Option<&'a str> {
    let name = attr(layout, "fill-style")?;
    metadata.styles.get(name)?.get("value").map(String::as_str)
}

/// Reads `rotation`, `flip`, the scales and the skews into one transform.
///
/// Returns `None` when the node declares nothing, or declares only values that
/// come to the identity, so the painter can keep such a node on its fast path.
fn transform_for(layout: &LayoutBox, metadata: &GuiMetadata) -> Option<Transform2D> {
    let number = |name: &str| {
        attr(layout, name)
            .map(|value| resolve_token(value, metadata))
            .and_then(|value| parse_number(&value))
    };

    // `flip` is a mirror, which is a scale of -1, so it folds into the scales
    // rather than needing a matrix of its own.
    let (flip_x, flip_y) = match attr(layout, "flip") {
        Some("h") => (-1.0, 1.0),
        Some("v") => (1.0, -1.0),
        Some("both") => (-1.0, -1.0),
        _ => (1.0, 1.0),
    };

    let (width, height) = (layout.rect.width, layout.rect.height);
    let (origin_x, origin_y) = transform_origin(
        attr(layout, "transform-origin").unwrap_or("center"),
        width,
        height,
    );

    let transform = Transform2D {
        rotation: number("rotation").unwrap_or(0.0),
        scale_x: number("scale-x").unwrap_or(1.0) * flip_x,
        scale_y: number("scale-y").unwrap_or(1.0) * flip_y,
        skew_x: number("skew-x").unwrap_or(0.0),
        skew_y: number("skew-y").unwrap_or(0.0),
        origin_x,
        origin_y,
    };

    (!transform.is_identity()).then_some(transform)
}

/// Resolves a `transform-origin` to pixels inside the node's box.
///
/// Takes the spec's hyphenated keywords, CSS's own keywords, percentages and
/// lengths. An unreadable value falls back to the centre, which is what CSS
/// uses when a transform is present and no origin is given.
fn transform_origin(value: &str, width: f32, height: f32) -> (f32, f32) {
    let value = value.trim();
    let keyword = match value {
        "top-left" => Some((0.0, 0.0)),
        "top-center" | "top" => Some((0.5, 0.0)),
        "top-right" => Some((1.0, 0.0)),
        "middle-left" | "left" => Some((0.0, 0.5)),
        "center" | "middle-center" => Some((0.5, 0.5)),
        "middle-right" | "right" => Some((1.0, 0.5)),
        "bottom-left" => Some((0.0, 1.0)),
        "bottom-center" | "bottom" => Some((0.5, 1.0)),
        "bottom-right" => Some((1.0, 1.0)),
        _ => None,
    };

    if let Some((x, y)) = keyword {
        return (width * x, height * y);
    }

    let mut parts = value.split_whitespace();
    let x = parts
        .next()
        .and_then(|part| origin_length(part, width))
        .unwrap_or(width / 2.0);
    let y = parts
        .next()
        .and_then(|part| origin_length(part, height))
        .unwrap_or(height / 2.0);

    (x, y)
}

fn origin_length(value: &str, extent: f32) -> Option<f32> {
    match value.trim().strip_suffix('%') {
        Some(percentage) => percentage
            .trim()
            .parse::<f32>()
            .ok()
            .map(|it| it / 100.0 * extent),
        None => parse_number(value),
    }
}

/// Reads `mask-src` and the geometry that positions it.
fn image_mask_for(layout: &LayoutBox, metadata: &GuiMetadata) -> Option<ImageMask> {
    let src = attr(layout, "mask-src")?;
    let number = |name: &str| {
        attr(layout, name)
            .map(|value| resolve_token(value, metadata))
            .and_then(|value| parse_number(&value))
    };

    Some(ImageMask {
        src: src.to_owned(),
        x: number("mask-x").unwrap_or(0.0),
        y: number("mask-y").unwrap_or(0.0),
        width: number("mask-width"),
        height: number("mask-height"),
        mode: attr(layout, "mask-mode").unwrap_or("alpha").to_owned(),
        composite: attr(layout, "mask-composite").unwrap_or("add").to_owned(),
    })
}

/// The node's children in the order they are painted.
///
/// The scene is a paint model, so `z-index` is resolved here rather than left
/// for the painter to re-derive. A node without one sorts as 0, and the sort
/// is stable, so document order still decides between equals.
fn paint_ordered_children(
    layout: &LayoutBox,
    metadata: &GuiMetadata,
    visible: bool,
) -> Vec<SceneNode> {
    // A decimal list item is numbered by its place among its list-item
    // siblings, which only the parent can count.
    let mut ordinal = 0usize;
    let mut children: Vec<(i32, SceneNode)> = layout
        .children
        .iter()
        .filter(|child| child.tag != "segment" && child.tag != "appearance")
        .map(|child| {
            if is_list_item(child) {
                ordinal += 1;
            }
            (
                z_index_of(child, metadata),
                build_scene_node(child, metadata, ordinal.max(1), visible),
            )
        })
        .collect();

    // `reverse-z` flips which sibling ends up on top without moving anything:
    // the layout was already computed from document order, and this list is
    // only the order they are painted in. Reversing before the sort rather
    // than after keeps `z-index` the stronger of the two, because the sort is
    // stable and so only decides between siblings that share a z-index.
    if reverses_z(layout) {
        children.reverse();
    }

    children.sort_by_key(|(z, _)| *z);
    children.into_iter().map(|(_, child)| child).collect()
}

/// Whether a container paints its children back to front.
///
/// A spec boolean is true by presence, so any value but `false` enables it.
fn reverses_z(layout: &LayoutBox) -> bool {
    attr(layout, "reverse-z").is_some_and(|value| value.trim() != "false")
}

/// Whether a node paints, given whether its ancestors do.
///
/// The spec defines `visible="false"` as CSS `visibility: hidden`, and that
/// property inherits: hiding a container hides everything inside it. It is
/// also the one CSS visibility value a descendant can undo, so an explicit
/// `visible="true"` under a hidden ancestor paints again. Anything else — the
/// attribute absent, or carrying a value that is neither — leaves the node
/// with whatever its ancestors decided.
fn visibility_of(layout: &LayoutBox, inherited: bool) -> bool {
    match attr(layout, "visible").map(str::trim) {
        Some("false") => false,
        Some("true") => true,
        _ => inherited,
    }
}

/// One indent step per `list-level`, matching kit.
const LIST_INDENT_STEP: f32 = 16.0;

fn is_list_item(layout: &LayoutBox) -> bool {
    attr(layout, "list").is_some_and(|value| value != "none")
}

pub(crate) fn list_indent(
    attributes: &std::collections::BTreeMap<String, String>,
    metadata: &GuiMetadata,
) -> f32 {
    attributes
        .get("list-level")
        .map(|value| resolve_token(value, metadata))
        .and_then(|value| value.trim().parse::<u32>().ok())
        .unwrap_or(0) as f32
        * LIST_INDENT_STEP
}

/// The marker text drawn before a list item's first line.
///
/// `list-marker` overrides the bullet entirely, which is how a document asks
/// for a dash or an emoji instead. Otherwise `decimal` numbers the item by its
/// place among its siblings and `disc` is a bullet.
pub(crate) fn list_marker_text(
    attributes: &std::collections::BTreeMap<String, String>,
    metadata: &GuiMetadata,
    ordinal: usize,
) -> Option<String> {
    let list = attributes.get("list")?;
    if list == "none" {
        return None;
    }

    if let Some(marker) = attributes.get("list-marker") {
        return Some(format!("{} ", resolve_token(marker, metadata).trim()));
    }

    Some(match list.as_str() {
        "decimal" => format!("{ordinal}. "),
        _ => "\u{2022} ".to_owned(),
    })
}

fn z_index_of(layout: &LayoutBox, metadata: &GuiMetadata) -> i32 {
    attr(layout, "z-index")
        .map(|value| resolve_token(value, metadata))
        .and_then(|value| value.trim().parse::<i32>().ok())
        .unwrap_or(0)
}

/// Whether one axis clips, from `clip` and the per-axis `overflow`.
///
/// `overflow-x` and `overflow-y` are the more specific of the two, so either
/// one overrides `clip` on its own axis, as it does in CSS.
///
/// Of the four overflow values only `visible` shows what escapes the box.
/// `scroll` and `auto` clip too: a still frame has no scrollbar to drag, so
/// what they reveal is the same first screenful `hidden` does.
fn clips_axis(layout: &LayoutBox, overflow: &str) -> bool {
    match attr(layout, overflow) {
        Some("visible") => false,
        Some(_) => true,
        None => attr(layout, "clip").is_some_and(|value| value != "false"),
    }
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
    // A named style sits under the node's own effects, so a node can add to
    // the style rather than only replace it.
    let mut effects = effect_style_effects(layout, metadata);
    effects.extend(appearance_effects(layout, metadata));

    if !effects.is_empty() {
        return effects;
    }

    shadow_shorthand(layout, metadata).into_iter().collect()
}

/// The effects a node picks up from `effect-style="name"`.
fn effect_style_effects(layout: &LayoutBox, metadata: &GuiMetadata) -> Vec<Effect> {
    let Some(name) = attr(layout, "effect-style") else {
        return Vec::new();
    };
    let Some(style) = metadata.effect_styles.get(name) else {
        return Vec::new();
    };

    style
        .iter()
        .filter(|effect| effect.get("visible").map(String::as_str) != Some("false"))
        .filter_map(|effect| {
            let get = |name: &str| effect.get(name).map(|value| resolve_token(value, metadata));
            let number = |name: &str, fallback: f32| {
                get(name)
                    .and_then(|value| parse_number(&value))
                    .unwrap_or(fallback)
            };

            Some(Effect {
                kind: effect.get("type")?.to_owned(),
                x: number("x", 0.0),
                y: number("y", 0.0),
                radius: number("radius", 0.0),
                spread: number("spread", 0.0),
                color: get("color"),
                opacity: number("opacity", 1.0),
                saturation: number("saturation", 180.0),
            })
        })
        .collect()
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

fn content_for(layout: &LayoutBox, metadata: &GuiMetadata, ordinal: usize) -> PaintContent {
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
                    font_stretch: run.style.font_stretch,
                    font_optical_sizing: run.style.font_optical_sizing,
                    font_variation: run.style.font_variation,
                    font_smoothing: run.style.font_smoothing,
                    word_spacing: run.style.word_spacing,
                    baseline_shift: run.style.baseline_shift,
                    decoration: run.style.decoration,
                })
                .collect();

            PaintContent::Text {
                value,
                segments,
                max_lines: max_text_lines(layout),
                truncate: truncates(layout),
                text_align: text_align_for(layout),
                text_rendering: attr(layout, "text-rendering").map(ToOwned::to_owned),
                writing_mode: attr(layout, "writing-mode").map(ToOwned::to_owned),
                white_space: attr(layout, "white-space").map(ToOwned::to_owned),
                text_wrap: attr(layout, "text-wrap").map(ToOwned::to_owned),
                word_break: attr(layout, "word-break").map(ToOwned::to_owned),
                paragraph_indent: attr(layout, "paragraph-indent")
                    .map(|value| resolve_token(value, metadata))
                    .and_then(|value| parse_number(&value))
                    .unwrap_or(0.0),
                list_marker: list_marker_text(&layout.attributes, metadata, ordinal),
                // One level is one indent step. 16px is what kit uses, and
                // nothing in the spec says otherwise.
                list_indent: list_indent(&layout.attributes, metadata),
                vertical_align: attr(layout, "vertical-align").map(ToOwned::to_owned),
                leading_trim: attr(layout, "leading-trim")
                    .map(ToOwned::to_owned)
                    .filter(|value| value != "normal"),
            }
        }
        "img" => layout
            .attributes
            .get("src")
            .cloned()
            .map(|src| PaintContent::Image {
                src,
                fit: layout.attributes.get("fit").cloned(),
                object_position: layout.attributes.get("object-position").cloned(),
                image_rendering: layout.attributes.get("image-rendering").cloned(),
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
        // `text-resize="truncate"` is a fixed box that cuts what overflows,
        // so it is the third spelling of the same instruction.
        || attr(layout, "text-resize").is_some_and(|value| value.trim() == "truncate")
}

#[cfg(test)]
mod tests {

    #[test]
    fn text_resize_truncate_asks_the_painter_to_truncate() {
        let truncating = |xml: &str| {
            let document = parse_gui_xml(xml).expect("valid gui");
            let layout = compute_taffy_layout(&document).expect("layout computes");
            let scene = build_scene(&document, &layout);
            match &scene.root.children[0].content {
                PaintContent::Text { truncate, .. } => *truncate,
                other => panic!("expected text, got {other:?}"),
            }
        };

        assert!(truncating(
            r##"
            <gui version="0.2">
              <col><text value="Hi" w="20" text-resize="truncate" /></col>
            </gui>
            "##
        ));
        assert!(
            !truncating(
                r##"
                <gui version="0.2">
                  <col><text value="Hi" w="20" text-resize="fixed" /></col>
                </gui>
                "##
            ),
            "a fixed box overflows rather than cutting"
        );
    }

    fn text_align_of(xml: &str) -> Option<String> {
        let document = parse_gui_xml(xml).expect("valid gui");
        let layout = compute_taffy_layout(&document).expect("layout computes");
        let scene = build_scene(&document, &layout);
        match &scene.root.children[0].content {
            PaintContent::Text { text_align, .. } => text_align.clone(),
            other => panic!("expected text content, got {other:?}"),
        }
    }

    #[test]
    fn rtl_text_starts_at_the_right_edge_of_its_box() {
        assert_eq!(
            text_align_of(
                r##"
                <gui version="0.2">
                  <col w="100">
                    <text value="שלום" direction="rtl" />
                  </col>
                </gui>
                "##,
            )
            .as_deref(),
            Some("right")
        );
    }

    #[test]
    fn ltr_text_keeps_the_left_edge() {
        assert_eq!(
            text_align_of(
                r##"
                <gui version="0.2">
                  <col w="100">
                    <text value="hello" direction="ltr" />
                  </col>
                </gui>
                "##,
            ),
            None,
            "left is the default, so there is nothing to record"
        );
    }

    #[test]
    fn a_declared_align_outranks_the_base_direction() {
        assert_eq!(
            text_align_of(
                r##"
                <gui version="0.2">
                  <col w="100">
                    <text value="שלום" direction="rtl" align="center" />
                  </col>
                </gui>
                "##,
            )
            .as_deref(),
            Some("center")
        );
    }

    #[test]
    fn a_shape_carries_the_fill_rule_it_declares() {
        let document = parse_gui_xml(
            r##"
            <gui version="0.2">
              <col>
                <rect w="10" h="10" fill="#000000" fill-rule="evenodd" />
                <rect w="10" h="10" fill="#000000" fill-rule="nonzero" />
                <ellipse w="10" h="10" fill="#000000" />
              </col>
            </gui>
            "##,
        )
        .expect("valid gui");
        let layout = compute_taffy_layout(&document).expect("layout computes");
        let scene = build_scene(&document, &layout);

        let kids = &scene.root.children;
        assert_eq!(kids[0].fill_rule, ShapeFillRule::EvenOdd);
        assert_eq!(kids[1].fill_rule, ShapeFillRule::NonZero);
        assert_eq!(
            kids[2].fill_rule,
            ShapeFillRule::NonZero,
            "nonzero is the default, as in SVG"
        );
    }

    fn border_of(xml: &str) -> Option<Border> {
        let document = parse_gui_xml(xml).expect("valid gui");
        let layout = compute_taffy_layout(&document).expect("layout computes");
        let scene = build_scene(&document, &layout);
        scene.root.children[0].borders.first().cloned()
    }

    #[test]
    fn the_border_longhands_can_declare_a_border_on_their_own() {
        let border = border_of(
            r##"
            <gui version="0.2">
              <col>
                <rect w="20" h="20" border-width="3" border-color="#ff0000"
                      border-style="dashed" border-align="inside" />
              </col>
            </gui>
            "##,
        )
        .expect("a border");

        assert_eq!(border.width, 3.0);
        assert_eq!(border.color, "#ff0000");
        assert_eq!(border.style, "dashed");
        assert_eq!(border.align, "inside");
    }

    #[test]
    fn a_longhand_overrides_only_its_own_part_of_the_shorthand() {
        let border = border_of(
            r##"
            <gui version="0.2">
              <col>
                <rect w="20" h="20" border="2 #000000 solid outside"
                      border-color="#00ff00" />
              </col>
            </gui>
            "##,
        )
        .expect("a border");

        assert_eq!(border.color, "#00ff00", "the longhand wins");
        assert_eq!(border.width, 2.0, "the shorthand still sets the width");
        assert_eq!(border.style, "solid");
        assert_eq!(border.align, "outside");
    }

    #[test]
    fn a_colour_with_no_width_gets_the_same_default_an_appearance_border_has() {
        let border = border_of(
            r##"
            <gui version="0.2">
              <col>
                <rect w="20" h="20" border-color="#0000ff" />
              </col>
            </gui>
            "##,
        )
        .expect("a border");

        assert_eq!(border.width, 1.0);
        assert_eq!(border.style, "solid");
        assert_eq!(border.align, "center");
    }

    #[test]
    fn a_width_with_no_colour_draws_nothing() {
        // Same rule the `<appearance><border>` path already follows: there is
        // no default border colour to fall back on.
        assert_eq!(
            border_of(
                r##"
                <gui version="0.2">
                  <col>
                    <rect w="20" h="20" border-width="4" />
                  </col>
                </gui>
                "##,
            ),
            None
        );
    }

    #[test]
    fn a_node_with_no_border_attributes_has_no_border() {
        assert_eq!(
            border_of(
                r##"
                <gui version="0.2">
                  <col>
                    <rect w="20" h="20" fill="#000000" />
                  </col>
                </gui>
                "##,
            ),
            None
        );
    }

    #[test]
    fn an_appearance_border_still_beats_the_longhands() {
        let border = border_of(
            r##"
            <gui version="0.2">
              <col>
                <rect w="20" h="20" border-color="#ff0000" border-width="9">
                  <appearance>
                    <border w="2" color="#00ff00" />
                  </appearance>
                </rect>
              </col>
            </gui>
            "##,
        )
        .expect("a border");

        assert_eq!(border.color, "#00ff00");
        assert_eq!(border.width, 2.0);
    }

    #[test]
    fn a_node_carries_the_layer_name_the_design_gave_it() {
        let document = parse_gui_xml(
            r##"
            <gui version="0.2">
              <col name="Card">
                <rect w="10" h="10" name="Thumbnail" />
                <rect w="10" h="10" />
              </col>
            </gui>
            "##,
        )
        .expect("valid gui");
        let layout = compute_taffy_layout(&document).expect("layout computes");
        let scene = build_scene(&document, &layout);

        assert_eq!(scene.root.name.as_deref(), Some("Card"));
        assert_eq!(scene.root.children[0].name.as_deref(), Some("Thumbnail"));
        assert_eq!(
            scene.root.children[1].name, None,
            "a node that was never named does not invent one"
        );
    }
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
        assert!(scene.root.clip_x && scene.root.clip_y);
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
                    line_height: None,
                    letter_spacing: 0.0,
                    color: None,
                    font_stretch: None,
                    font_optical_sizing: None,
                    font_variation: None,
                    font_smoothing: None,
                    word_spacing: 0.0,
                    decoration: None,
                    baseline_shift: 0.0,
                }],
                max_lines: None,
                truncate: false,
                text_align: None,
                text_rendering: None,
                writing_mode: None,
                white_space: None,
                text_wrap: None,
                word_break: None,
                paragraph_indent: 0.0,
                list_marker: None,
                list_indent: 0.0,
                vertical_align: None,
                leading_trim: None,
            }
        );
        assert_eq!(
            scene.root.children[1].content,
            PaintContent::Image {
                src: "assets/icon.svg".to_owned(),
                fit: None,
                object_position: None,
                image_rendering: None,
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
                    line_height: Some(28.0),
                    letter_spacing: 0.0,
                    color: None,
                    font_stretch: None,
                    font_optical_sizing: None,
                    font_variation: None,
                    font_smoothing: None,
                    word_spacing: 0.0,
                    decoration: None,
                    baseline_shift: 0.0,
                }],
                max_lines: None,
                truncate: false,
                text_align: None,
                text_rendering: None,
                writing_mode: None,
                white_space: None,
                text_wrap: None,
                word_break: None,
                paragraph_indent: 0.0,
                list_marker: None,
                list_indent: 0.0,
                vertical_align: None,
                leading_trim: None,
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
                    line_height: Some(20.0),
                    letter_spacing: 0.0,
                    color: None,
                    font_stretch: None,
                    font_optical_sizing: None,
                    font_variation: None,
                    font_smoothing: None,
                    word_spacing: 0.0,
                    decoration: None,
                    baseline_shift: 0.0,
                }],
                max_lines: Some(1),
                truncate: true,
                text_align: Some("right".to_owned()),
                text_rendering: None,
                writing_mode: None,
                white_space: None,
                text_wrap: None,
                word_break: None,
                paragraph_indent: 0.0,
                list_marker: None,
                list_indent: 0.0,
                vertical_align: None,
                leading_trim: None,
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
    fn overflow_clips_per_axis() {
        let scene = scene_of(
            r##"
            <gui version="0.2">
              <col w="100" h="50">
                <col w="10" h="10" overflow-x="hidden" />
                <col w="10" h="10" overflow-y="scroll" />
                <col w="10" h="10" overflow-x="hidden" overflow-y="hidden" />
                <col w="10" h="10" />
              </col>
            </gui>
            "##,
        );

        let axes: Vec<_> = scene
            .root
            .children
            .iter()
            .map(|child| (child.clip_x, child.clip_y))
            .collect();
        assert_eq!(
            axes,
            vec![(true, false), (false, true), (true, true), (false, false)]
        );
    }

    #[test]
    fn only_visible_overflow_shows_what_escapes_the_box() {
        // `scroll` and `auto` clip in a still frame: there is no scrollbar to
        // drag, so they show the same first screenful `hidden` does.
        for (value, clips) in [
            ("hidden", true),
            ("scroll", true),
            ("auto", true),
            ("visible", false),
        ] {
            let scene = scene_of(&format!(
                r##"
                <gui version="0.2">
                  <col w="100" h="50" overflow-x="{value}" overflow-y="{value}" />
                </gui>
                "##
            ));

            assert_eq!(scene.root.clip_x, clips, "overflow-x=\"{value}\"");
            assert_eq!(scene.root.clip_y, clips, "overflow-y=\"{value}\"");
        }
    }

    #[test]
    fn overflow_overrides_clip_on_its_own_axis() {
        let scene = scene_of(
            r##"
            <gui version="0.2">
              <col w="100" h="50" clip overflow-y="visible" />
            </gui>
            "##,
        );

        assert!(scene.root.clip_x, "clip still applies across x");
        assert!(
            !scene.root.clip_y,
            "overflow-y is the more specific of the two"
        );
    }

    #[test]
    fn z_index_decides_paint_order_and_ties_go_to_document_order() {
        let scene = scene_of(
            r##"
            <gui version="0.2">
              <stack w="100" h="100">
                <rect w="10" h="10" fill="#111111" z-index="2" />
                <rect w="10" h="10" fill="#222222" z-index="-1" />
                <rect w="10" h="10" fill="#333333" />
                <rect w="10" h="10" fill="#444444" />
              </stack>
            </gui>
            "##,
        );

        let order: Vec<_> = scene
            .root
            .children
            .iter()
            .filter_map(|child| child.fill_color())
            .collect();
        assert_eq!(
            order,
            vec!["#222222", "#333333", "#444444", "#111111"],
            "sorted by z-index, stable within a level"
        );
    }

    #[test]
    fn an_effect_style_lands_under_the_nodes_own_effects() {
        let scene = scene_of(
            r##"
            <gui version="0.2">
              <styles>
                <effect-style name="card">
                  <effect type="drop-shadow" x="0" y="1" radius="2" color="#0000001F" />
                  <effect type="drop-shadow" x="0" y="8" radius="24" color="#00000029" />
                </effect-style>
              </styles>
              <col w="100" h="50" effect-style="card">
                <appearance>
                  <effect type="inner-shadow" x="0" y="2" radius="4" color="#00000033" />
                </appearance>
              </col>
            </gui>
            "##,
        );

        let stack: Vec<_> = scene
            .root
            .effects
            .iter()
            .map(|effect| (effect.kind.as_str(), effect.y))
            .collect();
        assert_eq!(
            stack,
            vec![
                ("drop-shadow", 1.0),
                ("drop-shadow", 8.0),
                ("inner-shadow", 2.0)
            ]
        );
    }

    #[test]
    fn an_effect_style_resolves_tokens_and_honours_visible() {
        let scene = scene_of(
            r##"
            <gui version="0.2">
              <tokens>
                <token name="shadow-soft" value="#0000001F" />
              </tokens>
              <styles>
                <effect-style name="card">
                  <effect type="drop-shadow" x="0" y="4" radius="8" color="$shadow-soft" />
                  <effect type="drop-shadow" x="0" y="9" radius="9" visible="false" />
                </effect-style>
              </styles>
              <col w="100" h="50" effect-style="card" />
            </gui>
            "##,
        );

        assert_eq!(scene.root.effects.len(), 1);
        assert_eq!(scene.root.effects[0].color.as_deref(), Some("#0000001F"));
    }

    #[test]
    fn an_unknown_effect_style_is_ignored() {
        let scene = scene_of(
            r##"
            <gui version="0.2">
              <col w="100" h="50" effect-style="missing" shadow="0 2 6 #0000001F" />
            </gui>
            "##,
        );

        // Nothing came from the style, so the shorthand still applies.
        assert_eq!(scene.root.effects.len(), 1);
        assert_eq!(scene.root.effects[0].y, 2.0);
    }

    #[test]
    fn blend_isolation_and_filter_reach_the_scene() {
        let scene = scene_of(
            r##"
            <gui version="0.2">
              <col w="100" h="50" blend="multiply" isolation filter="brightness(1.2)">
                <rect w="10" h="10" blend="normal" filter="none" />
              </col>
            </gui>
            "##,
        );

        assert_eq!(scene.root.blend.as_deref(), Some("multiply"));
        assert!(scene.root.isolation);
        assert_eq!(scene.root.filter.as_deref(), Some("brightness(1.2)"));

        // `normal` and `none` are the defaults spelled out; they carry nothing.
        assert_eq!(scene.root.children[0].blend, None);
        assert_eq!(scene.root.children[0].filter, None);
        assert!(!scene.root.children[0].isolation);
    }

    #[test]
    fn an_image_mask_reads_its_geometry_and_defaults() {
        let scene = scene_of(
            r##"
            <gui version="0.2">
              <frame w="200" h="100">
                <group w="200" h="100" mask-src="assets/mask.svg" mask-x="4" mask-y="6"
                       mask-mode="luminance" mask-composite="subtract" />
              </frame>
            </gui>
            "##,
        );

        let group = &scene.root.children[0];
        let mask = group.image_mask.as_ref().expect("mask is read");
        assert_eq!(mask.src, "assets/mask.svg");
        assert_eq!((mask.x, mask.y), (4.0, 6.0));
        // Left out, so the node's own size stands in at paint time.
        assert_eq!((mask.width, mask.height), (None, None));
        assert_eq!(mask.mode, "luminance");
        assert_eq!(mask.composite, "subtract");
    }

    #[test]
    fn an_image_mask_defaults_to_alpha_and_add() {
        let scene = scene_of(
            r##"
            <gui version="0.2">
              <frame w="200" h="100">
                <group w="200" h="100" mask-src="assets/mask.svg" mask-width="50"
                       mask-height="25" />
              </frame>
            </gui>
            "##,
        );

        let group = &scene.root.children[0];
        let mask = group.image_mask.as_ref().expect("mask is read");
        assert_eq!((mask.width, mask.height), (Some(50.0), Some(25.0)));
        assert_eq!(mask.mode, "alpha");
        assert_eq!(mask.composite, "add");
    }

    #[test]
    fn a_masking_child_is_flagged_and_kept_in_the_tree() {
        let scene = scene_of(
            r##"
            <gui version="0.2">
              <frame w="100" h="100">
                <group w="100" h="100">
                  <rect abs x="0" y="0" w="40" h="40" fill="#00ff00" mask="true" />
                  <rect abs x="0" y="0" w="100" h="100" fill="#ff0000" />
                </group>
              </frame>
            </gui>
            "##,
        );

        let group = &scene.root.children[0];
        assert_eq!(
            group.children.len(),
            2,
            "the shape stays visible to a consumer"
        );
        assert!(group.children[0].mask);
        assert!(!group.children[1].mask);
    }

    #[test]
    fn a_fill_style_stands_in_for_the_fill_shorthand() {
        let scene = scene_of(
            r##"
            <gui version="0.2">
              <tokens>
                <token name="brand" value="#0D99FF" />
              </tokens>
              <styles>
                <fill-style name="surface" value="$brand" />
              </styles>
              <col w="100" h="50">
                <rect w="10" h="10" fill-style="surface" />
                <rect w="10" h="10" fill-style="surface" fill="#112233" />
                <rect w="10" h="10" fill-style="missing" />
              </col>
            </gui>
            "##,
        );

        assert_eq!(scene.root.children[0].fill_color(), Some("#0D99FF"));
        assert_eq!(
            scene.root.children[1].fill_color(),
            Some("#112233"),
            "a direct fill wins over the named style"
        );
        assert_eq!(scene.root.children[2].fill_color(), None);
    }

    #[test]
    fn clip_path_reaches_the_scene_and_none_reads_as_absent() {
        let scene = scene_of(
            r##"
            <gui version="0.2">
              <col w="100" h="50" clip-path="circle(40% at 50% 50%)">
                <rect w="10" h="10" clip-path="none" />
              </col>
            </gui>
            "##,
        );

        assert_eq!(
            scene.root.clip_path.as_deref(),
            Some("circle(40% at 50% 50%)")
        );
        assert_eq!(scene.root.children[0].clip_path, None);
    }

    #[test]
    fn a_transform_reads_its_parts_and_pivots_on_the_centre() {
        let scene = scene_of(
            r##"
            <gui version="0.2">
              <col w="100" h="50" rotation="45" scale-x="1.5" skew-y="10" />
            </gui>
            "##,
        );

        let transform = scene.root.transform.expect("transform is read");
        assert_eq!(transform.rotation, 45.0);
        assert_eq!(transform.scale_x, 1.5);
        assert_eq!(transform.scale_y, 1.0);
        assert_eq!(transform.skew_y, 10.0);
        assert_eq!((transform.origin_x, transform.origin_y), (50.0, 25.0));
    }

    #[test]
    fn flip_is_a_scale_of_minus_one() {
        for (flip, expected) in [
            ("h", (-1.0, 1.0)),
            ("v", (1.0, -1.0)),
            ("both", (-1.0, -1.0)),
        ] {
            let scene = scene_of(&format!(
                r##"
                <gui version="0.2">
                  <col w="100" h="50" flip="{flip}" />
                </gui>
                "##
            ));

            let transform = scene.root.transform.expect("transform is read");
            assert_eq!((transform.scale_x, transform.scale_y), expected, "{flip}");
        }
    }

    #[test]
    fn flip_and_scale_multiply_rather_than_replace() {
        let scene = scene_of(
            r##"
            <gui version="0.2">
              <col w="100" h="50" flip="h" scale-x="2" />
            </gui>
            "##,
        );

        assert_eq!(scene.root.transform.expect("read").scale_x, -2.0);
    }

    #[test]
    fn a_node_without_a_transform_carries_none() {
        let scene = scene_of(
            r##"
            <gui version="0.2">
              <col w="100" h="50">
                <rect w="10" h="10" rotation="0" scale-x="1" />
              </col>
            </gui>
            "##,
        );

        assert_eq!(scene.root.transform, None);
        assert_eq!(
            scene.root.children[0].transform, None,
            "values that come to the identity are not a transform"
        );
    }

    #[test]
    fn transform_origin_takes_keywords_percentages_and_lengths() {
        for (origin, expected) in [
            ("top-left", (0.0, 0.0)),
            ("bottom-right", (100.0, 50.0)),
            ("middle-left", (0.0, 25.0)),
            ("0% 100%", (0.0, 50.0)),
            ("25 10", (25.0, 10.0)),
            ("nonsense", (50.0, 25.0)),
        ] {
            let scene = scene_of(&format!(
                r##"
                <gui version="0.2">
                  <col w="100" h="50" rotation="10" transform-origin="{origin}" />
                </gui>
                "##
            ));

            let transform = scene.root.transform.expect("transform is read");
            assert_eq!(
                (transform.origin_x, transform.origin_y),
                expected,
                "{origin}"
            );
        }
    }

    #[test]
    fn href_is_carried_into_the_scene() {
        let scene = scene_of(
            r##"
            <gui version="0.2">
              <col w="100" h="50">
                <text value="Docs" href="https://example.com/docs" />
                <text value="Plain" />
              </col>
            </gui>
            "##,
        );

        assert_eq!(
            scene.root.children[0].href.as_deref(),
            Some("https://example.com/docs")
        );
        assert_eq!(scene.root.children[1].href, None);
    }

    #[test]
    fn image_rendering_and_object_position_reach_the_scene() {
        let scene = scene_of(
            r##"
            <gui version="0.2">
              <col w="100" h="50">
                <img src="assets/a.png" w="40" h="40" fit="cover"
                     object-position="left top" image-rendering="pixelated" />
              </col>
            </gui>
            "##,
        );

        assert_eq!(
            scene.root.children[0].content,
            PaintContent::Image {
                src: "assets/a.png".to_owned(),
                fit: Some("cover".to_owned()),
                object_position: Some("left top".to_owned()),
                image_rendering: Some("pixelated".to_owned()),
            }
        );
    }

    #[test]
    fn font_attributes_reach_the_segments_and_are_inherited() {
        let scene = scene_of(
            r##"
            <gui version="0.2">
              <col w="200" h="50">
                <text font-stretch="condensed" font-optical-sizing="auto"
                      font-smoothing="none" font-variation="&quot;wght&quot; 350"
                      value="Outer">
                  <segment value="Inner" font-stretch="expanded"
                           font-variation="&quot;wght&quot; 650" />
                  <segment value="Plain" />
                </text>
              </col>
            </gui>
            "##,
        );

        let PaintContent::Text { segments, .. } = &scene.root.children[0].content else {
            panic!("expected text content");
        };

        assert_eq!(segments[0].font_stretch.as_deref(), Some("condensed"));
        assert_eq!(segments[0].font_optical_sizing.as_deref(), Some("auto"));
        assert_eq!(segments[0].font_smoothing.as_deref(), Some("none"));
        assert_eq!(segments[0].font_variation.as_deref(), Some("\"wght\" 350"));

        assert_eq!(
            segments[1].font_stretch.as_deref(),
            Some("expanded"),
            "a segment overrides what it declares"
        );
        assert_eq!(
            segments[1].font_variation.as_deref(),
            Some("\"wght\" 650"),
            "a segment overrides its own axes"
        );
        assert_eq!(
            segments[1].font_smoothing.as_deref(),
            Some("none"),
            "and inherits what it does not"
        );
        assert_eq!(
            segments[2].font_variation.as_deref(),
            Some("\"wght\" 350"),
            "axes carry down to a segment that names none of its own"
        );
    }

    #[test]
    fn list_items_are_numbered_by_their_place_among_siblings() {
        let scene = scene_of(
            r##"
            <gui version="0.2">
              <col w="200" h="200">
                <text value="First" list="decimal" />
                <text value="Not a list item" />
                <text value="Second" list="decimal" />
                <text value="Bulleted" list="disc" />
                <text value="Custom" list="disc" list-marker="—" />
              </col>
            </gui>
            "##,
        );

        let marker = |index: usize| match &scene.root.children[index].content {
            PaintContent::Text { list_marker, .. } => list_marker.clone(),
            other => panic!("expected text, got {other:?}"),
        };

        assert_eq!(marker(0).as_deref(), Some("1. "));
        assert_eq!(marker(1), None, "a plain text node is not numbered");
        assert_eq!(
            marker(2).as_deref(),
            Some("2. "),
            "the plain node between does not take a number, as in CSS"
        );
        assert_eq!(marker(3).as_deref(), Some("\u{2022} "));
        assert_eq!(marker(4).as_deref(), Some("— "), "a custom marker wins");
    }

    #[test]
    fn list_level_indents_the_block() {
        let scene = scene_of(
            r##"
            <gui version="0.2">
              <col w="200" h="200">
                <text value="Top" list="disc" />
                <text value="Nested" list="disc" list-level="2" />
              </col>
            </gui>
            "##,
        );

        let indent = |index: usize| match &scene.root.children[index].content {
            PaintContent::Text { list_indent, .. } => *list_indent,
            other => panic!("expected text, got {other:?}"),
        };

        assert_eq!(indent(0), 0.0);
        assert_eq!(indent(1), 32.0);
    }

    #[test]
    fn vertical_align_and_leading_trim_reach_the_scene() {
        let scene = scene_of(
            r##"
            <gui version="0.2">
              <col w="200" h="200">
                <text value="A" vertical-align="center" leading-trim="cap-height" />
                <text value="B" leading-trim="normal" />
              </col>
            </gui>
            "##,
        );

        let PaintContent::Text {
            vertical_align,
            leading_trim,
            ..
        } = &scene.root.children[0].content
        else {
            panic!("expected text content");
        };
        assert_eq!(vertical_align.as_deref(), Some("center"));
        assert_eq!(leading_trim.as_deref(), Some("cap-height"));

        let PaintContent::Text { leading_trim, .. } = &scene.root.children[1].content else {
            panic!("expected text content");
        };
        assert_eq!(*leading_trim, None, "`normal` is no trim at all");
    }

    #[test]
    fn baseline_shift_belongs_to_the_run_that_declares_it() {
        let scene = scene_of(
            r##"
            <gui version="0.2">
              <col w="200" h="50">
                <text value="x" baseline-shift="4">
                  <segment value="2" baseline-shift="6" />
                  <segment value="3" />
                </text>
              </col>
            </gui>
            "##,
        );

        let PaintContent::Text { segments, .. } = &scene.root.children[0].content else {
            panic!("expected text content");
        };

        assert_eq!(segments[0].baseline_shift, 4.0);
        assert_eq!(segments[1].baseline_shift, 6.0);
        assert_eq!(
            segments[2].baseline_shift, 0.0,
            "a shift is not inherited, or a nested run would double it"
        );
    }

    #[test]
    fn every_transform_part_is_wired_to_its_own_axis() {
        // Four near-identical readers, which is where a copy-paste slip would
        // sit unnoticed — `scale-x` and `skew-y` alone would not catch it.
        let scene = scene_of(
            r##"
            <gui version="0.2">
              <col w="100" h="50" scale-x="2" scale-y="3" skew-x="10" skew-y="20" />
            </gui>
            "##,
        );

        let transform = scene.root.transform.expect("transform is read");
        assert_eq!(transform.scale_x, 2.0);
        assert_eq!(transform.scale_y, 3.0);
        assert_eq!(transform.skew_x, 10.0);
        assert_eq!(transform.skew_y, 20.0);
    }

    #[test]
    fn each_transform_part_can_stand_alone() {
        // Declared on its own, each part still has to make a transform — a
        // reader wired to the wrong field would fall back to the identity and
        // silently do nothing.
        let alone = |attribute: &str| {
            scene_of(&format!(
                r##"
                <gui version="0.2">
                  <col w="100" h="50" {attribute}="2" />
                </gui>
                "##
            ))
            .root
            .transform
            .unwrap_or_else(|| panic!("{attribute} alone should be a transform"))
        };

        assert_eq!(alone("scale-y").scale_y, 2.0);
        assert_eq!(alone("skew-x").skew_x, 2.0);
    }

    #[test]
    fn letter_spacing_reaches_the_segments_and_is_inherited() {
        let scene = scene_of(
            r##"
            <gui version="0.2">
              <col w="200" h="50">
                <text letter-spacing="3" value="Outer">
                  <segment value="Inner" />
                  <segment value="Wide" letter-spacing="6" />
                </text>
              </col>
            </gui>
            "##,
        );

        let PaintContent::Text { segments, .. } = &scene.root.children[0].content else {
            panic!("expected text content");
        };

        assert_eq!(segments[0].letter_spacing, 3.0);
        assert_eq!(segments[1].letter_spacing, 3.0, "inherited by a segment");
        assert_eq!(segments[2].letter_spacing, 6.0, "and overridable");
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
