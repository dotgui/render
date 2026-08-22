use crate::{
    blur,
    clip_path::{self, ClipBox},
    filter::apply_filter,
    fonts::FontAxes,
    gradient, text, AssetCache, Border, BorderWidths, Effect, Fill, FontFace, FontStore, ImageMask,
    PaintContent, Scene, SceneNode, TextSegment, Transform2D,
};
use fontdue::{Font, FontSettings};
use std::{fs, path::Path};
use thiserror::Error;
use tiny_skia::{
    BlendMode, Color, FillRule, Paint, PathBuilder, Pixmap, PixmapPaint, PixmapRef, Rect, Stroke,
    Transform,
};
use ttf_parser::OutlineBuilder;

#[derive(Debug, Error)]
pub enum PaintError {
    #[error("scene root has invalid bitmap size {width}x{height}")]
    InvalidSize { width: f32, height: f32 },

    #[error("failed to allocate bitmap {width}x{height}")]
    Allocation { width: u32, height: u32 },

    #[error("failed to write PNG: {0}")]
    Png(String),

    #[error("asset error: {0}")]
    Asset(String),

    #[error("failed to render SVG: {0}")]
    Svg(String),
}

pub fn paint_scene_to_png(scene: &Scene, path: impl AsRef<Path>) -> Result<(), PaintError> {
    paint_scene(scene, path, None, None)
}

pub fn paint_scene_to_png_with_assets(
    scene: &Scene,
    path: impl AsRef<Path>,
    cache: &AssetCache,
) -> Result<(), PaintError> {
    paint_scene(scene, path, Some(cache), None)
}

pub fn paint_scene_to_png_with_assets_and_fonts(
    scene: &Scene,
    path: impl AsRef<Path>,
    cache: &AssetCache,
    fonts: &FontStore,
) -> Result<(), PaintError> {
    paint_scene(scene, path, Some(cache), Some(fonts))
}

pub fn paint_scene_to_png_bytes(
    scene: &Scene,
    asset_cache: Option<&AssetCache>,
    fonts: Option<&FontStore>,
) -> Result<Vec<u8>, PaintError> {
    let width = scene.root.bounds.width.ceil();
    let height = scene.root.bounds.height.ceil();
    if width <= 0.0 || height <= 0.0 || !width.is_finite() || !height.is_finite() {
        return Err(PaintError::InvalidSize { width, height });
    }

    let width = width as u32;
    let height = height as u32;
    let mut pixmap = Pixmap::new(width, height).ok_or(PaintError::Allocation { width, height })?;
    let font = load_default_font();
    paint_node(&mut pixmap, &scene.root, font.as_ref(), asset_cache, fonts);
    pixmap
        .encode_png()
        .map_err(|err| PaintError::Png(err.to_string()))
}

fn paint_scene(
    scene: &Scene,
    path: impl AsRef<Path>,
    asset_cache: Option<&AssetCache>,
    fonts: Option<&FontStore>,
) -> Result<(), PaintError> {
    let bytes = paint_scene_to_png_bytes(scene, asset_cache, fonts)?;
    fs::write(path, bytes).map_err(|err| PaintError::Png(err.to_string()))
}

/// Paints a node, through its own layer when it needs one.
///
/// A blend mode, a filter or `isolation` all mean the subtree has to be
/// finished before it can be composited, so it is painted onto a transparent
/// layer of its own and drawn back in one go. Everything else paints straight
/// onto the canvas, which is the common case and keeps the cost off it.
fn paint_node(
    pixmap: &mut Pixmap,
    node: &SceneNode,
    font: Option<&Font>,
    asset_cache: Option<&AssetCache>,
    fonts: Option<&FontStore>,
) {
    if !needs_layer(node) {
        paint_node_direct(pixmap, node, font, asset_cache, fonts);
        return;
    }

    let Some(mut layer) = Pixmap::new(pixmap.width(), pixmap.height()) else {
        paint_node_direct(pixmap, node, font, asset_cache, fonts);
        return;
    };

    // Backdrop effects read what is behind the node, and inside a layer that
    // is nothing. Copying the canvas in first would then be blended twice, so
    // a node that both isolates and blurs its backdrop is a known gap.
    paint_node_direct(&mut layer, node, font, asset_cache, fonts);

    // `layer-blur` blurs the finished subtree, which is what makes it the
    // opposite of `background-blur`: one softens the node, the other softens
    // what the node sits on. Several stack, in document order.
    for effect in &node.effects {
        if effect.kind == "layer-blur" {
            // The radius is CSS's, which is twice the Gaussian sigma, as it is
            // for shadows and backdrop blurs.
            blur::blur(&mut layer, effect.radius / 2.0);
        }
    }

    if let Some(filter) = node.filter.as_deref() {
        apply_filter(&mut layer, filter);
    }

    // A mask cuts the finished layer down before it is composited, so it
    // shapes the node's own paint as well as its children — which is what
    // `mask-image` does in CSS, unlike `clip`, which only holds children in.
    //
    // It is applied to the layer rather than handed to `draw_pixmap`, because
    // a clip mask there would be in canvas space: a node that both masks and
    // transforms would have its mask left behind by the transform.
    if let Some(mask) = node_mask(pixmap.width(), pixmap.height(), node, asset_cache) {
        mask_layer(&mut layer, &mask);
    }

    pixmap.draw_pixmap(
        0,
        0,
        layer.as_ref(),
        &PixmapPaint {
            blend_mode: node
                .blend
                .as_deref()
                .and_then(blend_mode)
                .unwrap_or(BlendMode::SourceOver),
            // The subtree was drawn solid, so the whole group fades here.
            opacity: node.opacity.clamp(0.0, 1.0),
            // Resampling a finished layer is softer than drawing the geometry
            // transformed would be. It is the price of applying one matrix to
            // a whole subtree instead of threading it through every draw.
            quality: tiny_skia::FilterQuality::Bicubic,
        },
        node.transform
            .map(|transform| node_matrix(node, transform))
            .unwrap_or_default(),
        None,
    );
}

/// Multiplies a mask into a layer's alpha.
fn mask_layer(layer: &mut Pixmap, mask: &tiny_skia::Mask) {
    for (pixel, coverage) in layer.data_mut().chunks_exact_mut(4).zip(mask.data()) {
        // Premultiplied, so every channel scales with the alpha.
        for channel in pixel {
            *channel = ((*channel as u16 * *coverage as u16 + 127) / 255) as u8;
        }
    }
}

/// The node's transform as a matrix, pivoted on its `transform-origin`.
///
/// CSS applies the functions left to right — rotate, then flip and the scales,
/// then the skews — and each is a further multiplication, so the skews compose
/// as two matrices rather than one combined `from_skew`, which would drop the
/// cross term `skewX(a) skewY(b)` produces.
///
/// The origin is stored relative to the node's own box, and the layer this
/// matrix is applied to is the whole canvas, so the node's position has to be
/// added back or every node would pivot near the canvas corner.
fn node_matrix(node: &SceneNode, transform: Transform2D) -> Transform {
    let pivot_x = node.bounds.x + transform.origin_x;
    let pivot_y = node.bounds.y + transform.origin_y;
    let skew_x = transform.skew_x.to_radians().tan();
    let skew_y = transform.skew_y.to_radians().tan();

    Transform::from_translate(pivot_x, pivot_y)
        .pre_concat(Transform::from_rotate(transform.rotation))
        .pre_concat(Transform::from_scale(transform.scale_x, transform.scale_y))
        .pre_concat(Transform::from_skew(skew_x, 0.0))
        .pre_concat(Transform::from_skew(0.0, skew_y))
        .pre_concat(Transform::from_translate(-pivot_x, -pivot_y))
}

/// The alpha a node's own draws use.
///
/// A node painted onto its own layer has its `opacity` applied when that layer
/// is composited, so applying it per-colour as well would square it.
fn draw_opacity(node: &SceneNode) -> f32 {
    if needs_layer(node) {
        1.0
    } else {
        node.opacity
    }
}

/// Whether the node has to be finished on its own layer before compositing.
fn needs_layer(node: &SceneNode) -> bool {
    // `opacity` is a group property: the subtree is drawn solid and the whole
    // thing is faded once. Fading each draw instead makes overlapping children
    // compound, and leaves a container's children untouched entirely.
    node.opacity < 1.0
        || node.isolation
        || node
            .effects
            .iter()
            .any(|effect| effect.kind == "layer-blur")
        || node.transform.is_some()
        || node.filter.is_some()
        || node.clip_path.is_some()
        || node.image_mask.is_some()
        || node.children.iter().any(|child| child.mask)
        || node
            .blend
            .as_deref()
            .is_some_and(|mode| blend_mode(mode).is_some_and(|mode| mode != BlendMode::SourceOver))
}

/// The spec's blend names are CSS's, and tiny-skia's are the Skia ones.
fn blend_mode(value: &str) -> Option<BlendMode> {
    Some(match value.trim() {
        "normal" => BlendMode::SourceOver,
        "multiply" => BlendMode::Multiply,
        "screen" => BlendMode::Screen,
        "overlay" => BlendMode::Overlay,
        "darken" => BlendMode::Darken,
        "lighten" => BlendMode::Lighten,
        "color-dodge" => BlendMode::ColorDodge,
        "color-burn" => BlendMode::ColorBurn,
        "hard-light" => BlendMode::HardLight,
        "soft-light" => BlendMode::SoftLight,
        "difference" => BlendMode::Difference,
        "exclusion" => BlendMode::Exclusion,
        "hue" => BlendMode::Hue,
        "saturation" => BlendMode::Saturation,
        "color" => BlendMode::Color,
        "luminosity" => BlendMode::Luminosity,
        // `plus-lighter` and the rest of CSS's list have no Skia equivalent
        // here; leaving them unmapped paints normally rather than wrongly.
        _ => return None,
    })
}

fn paint_node_direct(
    pixmap: &mut Pixmap,
    node: &SceneNode,
    font: Option<&Font>,
    asset_cache: Option<&AssetCache>,
    fonts: Option<&FontStore>,
) {
    paint_backdrop_effects(pixmap, node);
    paint_drop_shadows(pixmap, node);
    paint_fill(pixmap, node, asset_cache);
    paint_inner_shadows(pixmap, node);
    paint_content(pixmap, node, font, asset_cache, fonts);

    if (node.clip_x || node.clip_y) && !node.children.is_empty() {
        if let Some(mut child_pixmap) = Pixmap::new(pixmap.width(), pixmap.height()) {
            for child in &node.children {
                if child.mask {
                    continue;
                }
                paint_node(&mut child_pixmap, child, font, asset_cache, fonts);
            }
            if let Some(mask) = create_clip_mask(pixmap.width(), pixmap.height(), node) {
                pixmap.draw_pixmap(
                    0,
                    0,
                    child_pixmap.as_ref(),
                    &PixmapPaint::default(),
                    Transform::identity(),
                    Some(&mask),
                );
            } else {
                pixmap.draw_pixmap(
                    0,
                    0,
                    child_pixmap.as_ref(),
                    &PixmapPaint::default(),
                    Transform::identity(),
                    None,
                );
            }
        }
    } else {
        for child in &node.children {
            // A masking child shapes its parent rather than being drawn.
            if child.mask {
                continue;
            }
            paint_node(pixmap, child, font, asset_cache, fonts);
        }
    }

    paint_border(pixmap, node);
    paint_outline(pixmap, node);
}

/// The mask a node composites through, from whichever source it declares.
///
/// A `clip-path`, an image mask and a masking child are all one thing by the
/// time they reach here: coverage per pixel. They are intersected when a node
/// carries more than one, because each is a further restriction.
fn node_mask(
    width: u32,
    height: u32,
    node: &SceneNode,
    asset_cache: Option<&AssetCache>,
) -> Option<tiny_skia::Mask> {
    let mut mask: Option<tiny_skia::Mask> = None;

    if let Some(clip_path) = node.clip_path.as_deref() {
        mask = intersect(mask, clip_path_mask(width, height, node, clip_path));
    }
    if let Some(image_mask) = node.image_mask.as_ref() {
        mask = intersect(
            mask,
            image_mask_of(width, height, node, image_mask, asset_cache),
        );
    }
    if let Some(child) = node.children.iter().find(|child| child.mask) {
        mask = intersect(mask, child_shape_mask(width, height, child));
    }

    mask
}

/// Keeps only what both masks cover.
///
/// A missing mask means "nothing to add", not "hide everything", so a source
/// that failed to resolve leaves the others in charge rather than blanking the
/// node.
fn intersect(
    current: Option<tiny_skia::Mask>,
    next: Option<tiny_skia::Mask>,
) -> Option<tiny_skia::Mask> {
    match (current, next) {
        (Some(mut current), Some(next)) => {
            for (a, b) in current.data_mut().iter_mut().zip(next.data()) {
                *a = ((*a as u16 * *b as u16 + 127) / 255) as u8;
            }
            Some(current)
        }
        (current, next) => current.or(next),
    }
}

fn clip_path_mask(
    width: u32,
    height: u32,
    node: &SceneNode,
    value: &str,
) -> Option<tiny_skia::Mask> {
    let area = ClipBox {
        x: node.bounds.x,
        y: node.bounds.y,
        width: node.bounds.width,
        height: paint_height(node),
    };

    if let Some(path) = clip_path::clip_path(value, area) {
        let mut mask = tiny_skia::Mask::new(width, height)?;
        mask.fill_path(&path, FillRule::Winding, true, Transform::identity());
        return Some(mask);
    }

    // `path()` carries an SVG `d`, so it goes through the SVG parser rather
    // than a second implementation of that grammar.
    let data = clip_path::svg_path_data(value)?;
    let rule = if value.contains("evenodd") {
        "evenodd"
    } else {
        "nonzero"
    };
    let svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width}\" height=\"{height}\">\
         <path d=\"{data}\" fill=\"#ffffff\" fill-rule=\"{rule}\" \
         transform=\"translate({x} {y})\"/></svg>",
        x = area.x,
        y = area.y,
    );

    let rendered = render_svg_to_pixmap(svg.as_bytes(), width, height)?;
    Some(alpha_mask(&rendered))
}

/// The mask a `mask-src` image gives, drawn once at its declared box.
fn image_mask_of(
    width: u32,
    height: u32,
    node: &SceneNode,
    image_mask: &ImageMask,
    asset_cache: Option<&AssetCache>,
) -> Option<tiny_skia::Mask> {
    let asset = asset_cache?.resolve(&image_mask.src).ok()?;
    let target_width = image_mask.width.unwrap_or(node.bounds.width);
    let target_height = image_mask.height.unwrap_or(paint_height(node));
    if target_width <= 0.0 || target_height <= 0.0 {
        return None;
    }

    // The mask box is relative to the node, and `mask-repeat` is always
    // `no-repeat`, so this is one draw at one place.
    let x = node.bounds.x + image_mask.x;
    let y = node.bounds.y + image_mask.y;
    let source = decode_mask_source(
        &asset.bytes,
        target_width.ceil() as u32,
        target_height.ceil() as u32,
    )?;

    let mut canvas = Pixmap::new(width, height)?;
    canvas.draw_pixmap(
        x.round() as i32,
        y.round() as i32,
        source.as_ref(),
        &PixmapPaint::default(),
        Transform::identity(),
        None,
    );

    let mut mask = if image_mask.mode == "luminance" {
        luminance_mask(&canvas)
    } else {
        alpha_mask(&canvas)
    };

    // With a single mask layer CSS's compositing operators have nothing to
    // combine against. Figma's do: `mask-src` is hoisted off a Figma group
    // mask, and there `subtract` and `exclude` mean "cut this shape out".
    if image_mask.composite == "subtract" || image_mask.composite == "exclude" {
        for coverage in mask.data_mut() {
            *coverage = 255 - *coverage;
        }
    }

    Some(mask)
}

/// The mask a `mask="true"` child gives: its own outline.
fn child_shape_mask(width: u32, height: u32, child: &SceneNode) -> Option<tiny_skia::Mask> {
    let mut mask = tiny_skia::Mask::new(width, height)?;
    let path = if child.tag == "ellipse" {
        ellipse_path(
            child.bounds.x,
            child.bounds.y,
            child.bounds.width,
            paint_height(child),
        )?
    } else {
        smoothed_rect_path(
            child.bounds.x,
            child.bounds.y,
            child.bounds.width,
            paint_height(child),
            child.radius.unwrap_or(0.0),
            child.corner_smoothing,
        )?
    };

    mask.fill_path(&path, FillRule::Winding, true, Transform::identity());
    Some(mask)
}

fn alpha_mask(pixmap: &Pixmap) -> tiny_skia::Mask {
    tiny_skia::Mask::from_pixmap(pixmap.as_ref(), tiny_skia::MaskType::Alpha)
}

fn luminance_mask(pixmap: &Pixmap) -> tiny_skia::Mask {
    tiny_skia::Mask::from_pixmap(pixmap.as_ref(), tiny_skia::MaskType::Luminance)
}

/// Decodes a mask source — SVG or raster — at the size it is drawn.
fn decode_mask_source(bytes: &[u8], width: u32, height: u32) -> Option<Pixmap> {
    if looks_like_svg(bytes) {
        return render_svg_to_pixmap(bytes, width, height);
    }

    let decoded = image::load_from_memory(bytes).ok()?.to_rgba8();
    let source = Pixmap::from_vec(
        premultiply_rgba(decoded.as_raw()),
        tiny_skia::IntSize::from_wh(decoded.width(), decoded.height())?,
    )?;

    let mut scaled = Pixmap::new(width, height)?;
    scaled.draw_pixmap(
        0,
        0,
        source.as_ref(),
        &PixmapPaint::default(),
        Transform::from_scale(
            width as f32 / decoded.width() as f32,
            height as f32 / decoded.height() as f32,
        ),
        None,
    );
    Some(scaled)
}

fn render_svg_to_pixmap(bytes: &[u8], width: u32, height: u32) -> Option<Pixmap> {
    let tree = resvg::usvg::Tree::from_data(bytes, &resvg::usvg::Options::default()).ok()?;
    let size = tree.size();
    let mut pixmap = Pixmap::new(width.max(1), height.max(1))?;
    resvg::render(
        &tree,
        Transform::from_scale(width as f32 / size.width(), height as f32 / size.height()),
        &mut pixmap.as_mut(),
    );
    Some(pixmap)
}

fn create_clip_mask(width: u32, height: u32, node: &SceneNode) -> Option<tiny_skia::Mask> {
    let mut mask = tiny_skia::Mask::new(width, height)?;

    // One axis on its own clips to a band across the canvas: the node's shape
    // says nothing about where the unclipped direction should stop, and a
    // rounded corner needs both edges to curve between.
    if node.clip_x != node.clip_y {
        let band = if node.clip_x {
            Rect::from_xywh(node.bounds.x, 0.0, node.bounds.width, height as f32)
        } else {
            Rect::from_xywh(0.0, node.bounds.y, width as f32, paint_height(node))
        }?;

        let mut pb = PathBuilder::new();
        pb.push_rect(band);
        let path = pb.finish()?;
        mask.fill_path(&path, FillRule::Winding, true, Transform::identity());
        return Some(mask);
    }

    let path = if node.tag == "ellipse" {
        ellipse_path(
            node.bounds.x,
            node.bounds.y,
            node.bounds.width,
            node.bounds.height,
        )?
    } else if let Some(radius) = node.radius.filter(|r| *r > 0.0) {
        smoothed_rect_path(
            node.bounds.x,
            node.bounds.y,
            node.bounds.width,
            paint_height(node),
            radius,
            node.corner_smoothing,
        )?
    } else {
        let mut pb = PathBuilder::new();
        pb.move_to(node.bounds.x, node.bounds.y);
        pb.line_to(node.bounds.x + node.bounds.width, node.bounds.y);
        pb.line_to(
            node.bounds.x + node.bounds.width,
            node.bounds.y + paint_height(node),
        );
        pb.line_to(node.bounds.x, node.bounds.y + paint_height(node));
        pb.close();
        pb.finish()?
    };
    mask.fill_path(&path, FillRule::Winding, true, Transform::identity());
    Some(mask)
}

/// The node's own outline, optionally grown and moved.
///
/// Shadows are the same shape as the thing casting them, so they reuse this
/// rather than re-deriving rounded corners and ellipses.
fn node_shape_path(
    node: &SceneNode,
    inflate: f32,
    offset_x: f32,
    offset_y: f32,
) -> Option<tiny_skia::Path> {
    let x = node.bounds.x - inflate + offset_x;
    let y = node.bounds.y - inflate + offset_y;
    let width = node.bounds.width + inflate * 2.0;
    let height = paint_height(node) + inflate * 2.0;
    if width <= 0.0 || height <= 0.0 {
        return None;
    }

    if node.tag == "ellipse" {
        return ellipse_path(x, y, width, height);
    }

    // Growing a rounded rectangle grows its corners with it, which is what
    // keeps a spread shadow concentric with its box.
    let radius = node.radius.unwrap_or(0.0);
    smoothed_rect_path(
        x,
        y,
        width,
        height,
        (radius + inflate).max(0.0),
        node.corner_smoothing,
    )
}

/// Effects that read what is already on the canvas, before this node covers it.
fn paint_backdrop_effects(pixmap: &mut Pixmap, node: &SceneNode) {
    for effect in &node.effects {
        match effect.kind.as_str() {
            "background-blur" | "glass" => {
                let Some(mask) = create_clip_mask(pixmap.width(), pixmap.height(), node) else {
                    continue;
                };

                // Blur a copy of the backdrop, then let it back through the
                // node's own outline — the same thing `backdrop-filter` does.
                let mut backdrop = pixmap.clone();
                blur::blur(&mut backdrop, effect.radius / 2.0);
                if effect.kind == "glass" {
                    saturate(&mut backdrop, effect.saturation / 100.0);
                }

                pixmap.draw_pixmap(
                    0,
                    0,
                    backdrop.as_ref(),
                    &PixmapPaint::default(),
                    Transform::identity(),
                    Some(&mask),
                );
            }
            // `layer-blur` blurs the node itself rather than what is behind
            // it, so it belongs on the node's own layer, not here.
            _ => {}
        }
    }
}

fn paint_drop_shadows(pixmap: &mut Pixmap, node: &SceneNode) {
    for effect in &node.effects {
        if effect.kind != "drop-shadow" {
            continue;
        }
        let Some(color) = effect_color(effect, node) else {
            continue;
        };
        let Some(path) = node_shape_path(node, effect.spread, effect.x, effect.y) else {
            continue;
        };
        let Some(mut shadow) = Pixmap::new(pixmap.width(), pixmap.height()) else {
            continue;
        };

        let mut paint = Paint::default();
        paint.set_color(color);
        paint.anti_alias = true;
        shadow.fill_path(
            &path,
            &paint,
            FillRule::Winding,
            Transform::identity(),
            None,
        );
        // CSS states shadow blur as a radius; the Gaussian sigma is half it.
        blur::blur(&mut shadow, effect.radius / 2.0);

        pixmap.draw_pixmap(
            0,
            0,
            shadow.as_ref(),
            &PixmapPaint::default(),
            Transform::identity(),
            None,
        );
    }
}

/// An inset shadow: the blurred *outside* of a shrunken outline, showing only
/// within the node.
fn paint_inner_shadows(pixmap: &mut Pixmap, node: &SceneNode) {
    for effect in &node.effects {
        if effect.kind != "inner-shadow" {
            continue;
        }
        let Some(color) = effect_color(effect, node) else {
            continue;
        };
        let Some(mask) = create_clip_mask(pixmap.width(), pixmap.height(), node) else {
            continue;
        };
        let Some(mut shadow) = Pixmap::new(pixmap.width(), pixmap.height()) else {
            continue;
        };

        // Flood the area, then punch out the offset shape. What survives is
        // the ring of colour that the blur turns into an inner edge.
        shadow.fill(color);

        if let Some(hole) = node_shape_path(node, -effect.spread, effect.x, effect.y) {
            let mut cut = Paint::default();
            cut.set_color(Color::TRANSPARENT);
            cut.blend_mode = tiny_skia::BlendMode::Clear;
            cut.anti_alias = true;
            shadow.fill_path(&hole, &cut, FillRule::Winding, Transform::identity(), None);
        }

        blur::blur(&mut shadow, effect.radius / 2.0);

        pixmap.draw_pixmap(
            0,
            0,
            shadow.as_ref(),
            &PixmapPaint::default(),
            Transform::identity(),
            Some(&mask),
        );
    }
}

fn effect_color(effect: &Effect, node: &SceneNode) -> Option<Color> {
    let color = effect.color.as_deref().unwrap_or("#00000040");
    parse_color(color, node.opacity * effect.opacity)
}

/// Scales colour away from (or past) grey, as CSS `saturate()` does.
fn saturate(pixmap: &mut Pixmap, amount: f32) {
    if (amount - 1.0).abs() < f32::EPSILON {
        return;
    }

    for pixel in pixmap.pixels_mut() {
        let (r, g, b, a) = (
            f32::from(pixel.red()),
            f32::from(pixel.green()),
            f32::from(pixel.blue()),
            pixel.alpha(),
        );
        // Rec. 601 luma, the same weights CSS filters use.
        let luma = 0.299 * r + 0.587 * g + 0.114 * b;
        let mix = |channel: f32| (luma + (channel - luma) * amount).clamp(0.0, f32::from(a)) as u8;
        *pixel =
            tiny_skia::PremultipliedColorU8::from_rgba(mix(r), mix(g), mix(b), a).unwrap_or(*pixel);
    }
}

/// Paints the node's fill stack, bottom entry first.
fn paint_fill(pixmap: &mut Pixmap, node: &SceneNode, asset_cache: Option<&AssetCache>) {
    if matches!(node.content, PaintContent::Text { .. }) {
        return;
    }

    for fill in &node.fills {
        paint_one_fill(pixmap, node, fill, asset_cache);
    }
}

fn paint_one_fill(
    pixmap: &mut Pixmap,
    node: &SceneNode,
    fill: &Fill,
    asset_cache: Option<&AssetCache>,
) {
    if fill.kind == "image" {
        paint_image_fill(pixmap, node, fill, asset_cache);
        return;
    }

    let Some(value) = fill.value.as_deref() else {
        return;
    };

    // A gradient is a shader over the node's shape rather than a colour, and
    // it can arrive either as `type="linear-gradient"` or as a `fill`
    // attribute whose value happens to be one, so the value decides.
    if gradient::is_gradient(value) {
        paint_gradient_fill(pixmap, node, value);
        return;
    }

    let Some(color) = parse_color(value, draw_opacity(node)) else {
        return;
    };

    if node.tag == "ellipse" {
        if let Some(path) = ellipse_path(
            node.bounds.x,
            node.bounds.y,
            node.bounds.width,
            node.bounds.height,
        ) {
            let mut paint = Paint::default();
            paint.set_color(color);
            paint.anti_alias = true;
            pixmap.fill_path(
                &path,
                &paint,
                FillRule::Winding,
                Transform::identity(),
                None,
            );
        }
        return;
    }

    fill_smoothed_rect(
        pixmap,
        node.bounds.x,
        node.bounds.y,
        node.bounds.width,
        paint_height(node),
        node.radius.unwrap_or(0.0),
        node.corner_smoothing,
        color,
    );
}

/// The node's own outline, as the path a fill covers.
fn node_fill_path(node: &SceneNode) -> Option<tiny_skia::Path> {
    if node.tag == "ellipse" {
        return ellipse_path(
            node.bounds.x,
            node.bounds.y,
            node.bounds.width,
            node.bounds.height,
        );
    }

    smoothed_rect_path(
        node.bounds.x,
        node.bounds.y,
        node.bounds.width,
        paint_height(node),
        node.radius.unwrap_or(0.0),
        node.corner_smoothing,
    )
}

fn paint_gradient_fill(pixmap: &mut Pixmap, node: &SceneNode, value: &str) {
    let area = gradient::GradientBox {
        x: node.bounds.x,
        y: node.bounds.y,
        width: node.bounds.width,
        height: paint_height(node),
    };
    // Stops resolve through the same colour parser every other paint uses, so
    // a hex with alpha means the same thing in a gradient as out of one.
    let Some(shader) = gradient::gradient_shader(value, area, draw_opacity(node), &parse_color)
    else {
        return;
    };
    let Some(path) = node_fill_path(node) else {
        return;
    };

    let paint = Paint {
        shader,
        anti_alias: true,
        ..Default::default()
    };
    pixmap.fill_path(
        &path,
        &paint,
        FillRule::Winding,
        Transform::identity(),
        None,
    );
}

/// Paints `<fill type="image" src="..." fit="...">`.
///
/// The image is drawn onto a layer and composited through the node's own
/// outline, so it is held inside a rounded or elliptical node the way a colour
/// fill would be — whatever `fit` mode it uses.
fn paint_image_fill(
    pixmap: &mut Pixmap,
    node: &SceneNode,
    fill: &Fill,
    asset_cache: Option<&AssetCache>,
) {
    let (Some(cache), Some(src)) = (asset_cache, fill.src.as_deref()) else {
        return;
    };
    let Ok(asset) = cache.resolve(src) else {
        return;
    };
    let Some(mut layer) = Pixmap::new(pixmap.width(), pixmap.height()) else {
        return;
    };

    let drawn =
        if asset.media_type.as_deref() == Some("image/svg+xml") || looks_like_svg(&asset.bytes) {
            render_svg_asset(&mut layer, node, &asset.bytes)
        } else {
            render_raster_asset(
                &mut layer,
                node,
                &asset.bytes,
                ImageStyle {
                    fit: fill.fit.as_deref(),
                    ..Default::default()
                },
            )
        };

    if drawn.is_err() {
        return;
    }

    let mask = create_clip_mask(pixmap.width(), pixmap.height(), node);
    pixmap.draw_pixmap(
        0,
        0,
        layer.as_ref(),
        &PixmapPaint::default(),
        Transform::identity(),
        mask.as_ref(),
    );
}

fn paint_height(node: &SceneNode) -> f32 {
    // A `<line>` is a divider: it has no height of its own, so `thickness`
    // gives it one. The spec's default is 1px, which is what it drew before
    // the attribute was read.
    if node.tag == "line" && node.bounds.height <= 0.0 {
        node.thickness.filter(|it| *it > 0.0).unwrap_or(1.0)
    } else {
        node.bounds.height
    }
}

fn paint_content(
    pixmap: &mut Pixmap,
    node: &SceneNode,
    font: Option<&Font>,
    asset_cache: Option<&AssetCache>,
    fonts: Option<&FontStore>,
) {
    match &node.content {
        PaintContent::None => {}
        PaintContent::Text {
            segments,
            max_lines,
            truncate,
            text_align,
            white_space,
            text_wrap,
            word_break,
            paragraph_indent,
            list_marker,
            list_indent,
            vertical_align,
            leading_trim,
            ..
        } => {
            let styles = segments
                .iter()
                .map(|segment| RunStyle::resolve(segment, node, font, fonts))
                .collect::<Vec<_>>();

            if styles.iter().all(|style| !style.is_drawable()) {
                paint_text_placeholder(pixmap, node);
                return;
            }

            let runs = segments
                .iter()
                .enumerate()
                .map(|(index, segment)| text::Run {
                    text: segment.value.clone(),
                    style: index,
                })
                .collect::<Vec<_>>();

            paint_text(
                pixmap,
                node,
                &styles,
                &runs,
                *max_lines,
                *truncate,
                text_align.as_deref(),
                text::WrapOptions::new(
                    white_space.as_deref(),
                    text_wrap.as_deref(),
                    word_break.as_deref(),
                    *paragraph_indent,
                ),
                BlockStyle {
                    list_marker: list_marker.as_deref(),
                    list_indent: *list_indent,
                    vertical_align: vertical_align.as_deref(),
                    leading_trim: leading_trim.is_some(),
                },
            );
        }
        PaintContent::Image {
            src,
            fit,
            object_position,
            image_rendering,
        } => paint_image(
            pixmap,
            node,
            src,
            ImageStyle {
                fit: fit.as_deref(),
                object_position: object_position.as_deref(),
                rendering: image_rendering.as_deref(),
            },
            asset_cache,
        ),
    }
}

/// The text properties every painter needs, kept together so the painting
/// entry points do not thread seven parallel arguments each.
/// One text style resolved to everything painting needs: a face to draw with,
/// a colour, and the metrics that position it.
struct RunStyle<'a> {
    /// The declared face, when the document's fonts resolved.
    face: Option<&'a FontFace>,
    /// The host's default font, used when the declared face is missing.
    fallback: Option<&'a Font>,
    color: Option<Color>,
    font_size: f32,
    line_height: f32,
    letter_spacing: f32,
    axes: FontAxes,
    /// Whether glyphs are antialiased, from `font-smoothing`.
    anti_alias: bool,
    /// Extra advance on each space, from `word-spacing`.
    word_spacing: f32,
    /// Pixels this run rides above the line's shared baseline.
    baseline_shift: f32,
}

impl<'a> RunStyle<'a> {
    fn resolve(
        segment: &TextSegment,
        node: &SceneNode,
        fallback: Option<&'a Font>,
        fonts: Option<&'a FontStore>,
    ) -> Self {
        let face = fonts.and_then(|fonts| {
            fonts.get(
                segment.font_family.as_deref(),
                segment.font_weight.as_deref(),
                segment.font_style.as_deref(),
            )
        });

        Self {
            face,
            fallback,
            // A segment inherits the node's fill when it declares no colour of
            // its own; `node.opacity` applies either way.
            color: segment
                .color
                .as_deref()
                .or(node.fill_color())
                .and_then(|fill| parse_color(fill, draw_opacity(node))),
            font_size: segment.font_size,
            line_height: segment.line_height,
            letter_spacing: segment.letter_spacing,
            axes: FontAxes::from_style(
                segment.font_stretch.as_deref(),
                segment.font_optical_sizing.as_deref(),
                segment.font_size,
            ),
            // `none` asks for hard edges. The other two values are both
            // grayscale antialiasing here; subpixel needs the target's own
            // stripe order, which a PNG has no business assuming.
            anti_alias: segment.font_smoothing.as_deref() != Some("none"),
            word_spacing: segment.word_spacing,
            baseline_shift: segment.baseline_shift,
        }
    }

    fn is_drawable(&self) -> bool {
        self.color.is_some() && (self.face.is_some() || self.fallback.is_some())
    }

    fn width(&self, text: &str) -> f32 {
        let base = match (self.face, self.fallback) {
            (Some(face), _) => face.text_width(text, self.font_size, self.axes),
            (None, Some(font)) => text_width(font, text, self.font_size),
            (None, None) => text.chars().count() as f32 * self.font_size * 0.55,
        };
        base + text.chars().count() as f32 * self.letter_spacing
            + self.spaces(text) * self.word_spacing
    }

    /// How many space characters a string carries, for `word-spacing`.
    fn spaces(&self, text: &str) -> f32 {
        text.chars().filter(|ch| *ch == ' ').count() as f32
    }

    /// The extra advance a single character earns from `word-spacing`.
    fn space_advance(&self, ch: char) -> f32 {
        if ch == ' ' {
            self.word_spacing
        } else {
            0.0
        }
    }

    /// How much `leading-trim` takes off the top of the block.
    ///
    /// This is `FontFace::leading_trim`, the same one `TextMeasurer` reaches,
    /// so a trimmed box is sized and drawn to the same edge.
    fn leading_trim(&self, line_height: f32) -> f32 {
        match (self.face, self.fallback) {
            (Some(face), _) => face.leading_trim(self.font_size, line_height),
            // The host's default font has no `capHeight` reachable through
            // fontdue, so the trim uses the same ratio the approximate
            // measurer does — and stays a trim rather than silently nothing.
            (None, Some(font)) => (fontdue_baseline_offset(font, self.font_size, line_height)
                - self.font_size * crate::layout::CAP_HEIGHT_RATIO)
                .max(0.0),
            (None, None) => 0.0,
        }
    }

    fn baseline_offset(&self, line_height: f32) -> f32 {
        match (self.face, self.fallback) {
            (Some(face), _) => face.baseline_offset(self.font_size, line_height, self.axes),
            (None, Some(font)) => fontdue_baseline_offset(font, self.font_size, line_height),
            (None, None) => self.font_size,
        }
    }

    /// Draws `text` from `cursor_x`, advancing it past the run.
    fn draw(&self, pixmap: &mut Pixmap, text: &str, cursor_x: &mut f32, baseline: f32) {
        let Some(color) = self.color else {
            return;
        };

        if let Some(face) = self.face {
            if let Some(ttf_face) = face.variable_face(self.axes) {
                self.draw_outlines(pixmap, &ttf_face, face, text, cursor_x, baseline, color);
                return;
            }
        }

        let font = self.face.map(FontFace::fallback).or(self.fallback);
        if let Some(font) = font {
            self.draw_bitmaps(pixmap, font, text, cursor_x, baseline, color);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_outlines(
        &self,
        pixmap: &mut Pixmap,
        ttf_face: &ttf_parser::Face<'_>,
        face: &FontFace,
        text: &str,
        cursor_x: &mut f32,
        baseline: f32,
        color: Color,
    ) {
        let scale = self.font_size / ttf_face.units_per_em() as f32;
        let mut paint = Paint::default();
        paint.set_color(color);
        paint.anti_alias = self.anti_alias;

        for ch in text.chars() {
            let Some(glyph) = ttf_face.glyph_index(ch) else {
                *cursor_x += face.fallback().metrics(ch, self.font_size).advance_width
                    + self.letter_spacing
                    + self.space_advance(ch);
                continue;
            };

            let mut builder = GlyphPathBuilder::new(*cursor_x, baseline, scale);
            ttf_face.outline_glyph(glyph, &mut builder);
            if let Some(path) = builder.finish() {
                pixmap.fill_path(
                    &path,
                    &paint,
                    FillRule::Winding,
                    Transform::identity(),
                    None,
                );
            }

            *cursor_x += ttf_face
                .glyph_hor_advance(glyph)
                .map(|advance| advance as f32 * scale)
                .unwrap_or_else(|| face.fallback().metrics(ch, self.font_size).advance_width)
                + self.letter_spacing
                + self.space_advance(ch);
        }
    }

    fn draw_bitmaps(
        &self,
        pixmap: &mut Pixmap,
        font: &Font,
        text: &str,
        cursor_x: &mut f32,
        baseline: f32,
        color: Color,
    ) {
        for ch in text.chars() {
            let (metrics, bitmap) = font.rasterize(ch, self.font_size);
            draw_glyph_bitmap(
                pixmap,
                *cursor_x + metrics.xmin as f32,
                baseline - metrics.ymin as f32 - metrics.height as f32,
                metrics.width,
                metrics.height,
                &bitmap,
                color,
                self.anti_alias,
            );
            *cursor_x += metrics.advance_width + self.letter_spacing + self.space_advance(ch);
        }
    }
}

/// The block-level text properties, kept together rather than threaded as
/// four more arguments through a function that already takes eight.
#[derive(Debug, Clone, Copy, Default)]
struct BlockStyle<'a> {
    list_marker: Option<&'a str>,
    list_indent: f32,
    vertical_align: Option<&'a str>,
    leading_trim: bool,
}

#[allow(clippy::too_many_arguments)]
fn paint_text(
    pixmap: &mut Pixmap,
    node: &SceneNode,
    styles: &[RunStyle<'_>],
    runs: &[text::Run],
    max_lines: Option<usize>,
    truncate: bool,
    text_align: Option<&str>,
    mut wrap: text::WrapOptions,
    block: BlockStyle<'_>,
) {
    let measure = |text: &str, style: usize| styles[style].width(text);

    // The marker sits on the first line ahead of the text, so it takes room
    // there exactly as an indent does — and is drawn in that room below.
    let marker_width = block.list_marker.map_or(0.0, |marker| measure(marker, 0));
    wrap.indent += marker_width;

    // The whole block moves right by the list indent, and has that much less
    // room to wrap in.
    let left = node.bounds.x + block.list_indent;
    let width = (node.bounds.width - block.list_indent).max(0.0);

    let mut lines = text::wrap_runs(runs, Some(width), &measure, wrap);
    text::apply_line_limit_and_ellipsis(&mut lines, max_lines, truncate, width, &measure);

    // `vertical-align` places a block shorter than its box, so the block's own
    // height has to be known before the first line is drawn.
    let line_heights: Vec<f32> = lines
        .iter()
        .map(|line| {
            line.iter()
                .map(|run| styles[run.style].line_height)
                .fold(0.0_f32, f32::max)
                .max(styles.first().map_or(0.0, |style| style.line_height))
        })
        .collect();
    let block_height: f32 = line_heights.iter().sum();
    let slack = (node.bounds.height - block_height).max(0.0);

    let max_y = node.bounds.y + node.bounds.height;
    let mut line_top = node.bounds.y
        + match block.vertical_align {
            Some("center") | Some("middle") => slack / 2.0,
            Some("bottom") => slack,
            _ => 0.0,
        };

    for (index, line) in lines.into_iter().enumerate() {
        // The tallest run on the line sets the line box and the shared
        // baseline, so a larger segment is not clipped by its neighbours.
        let tallest = line
            .iter()
            .map(|run| run.style)
            .max_by(|a, b| styles[*a].font_size.total_cmp(&styles[*b].font_size))
            .unwrap_or(0);
        let line_height = line
            .iter()
            .map(|run| styles[run.style].line_height)
            .fold(0.0_f32, f32::max)
            .max(styles[tallest].line_height);
        // `leading-trim` takes the half-leading off the top of the block, so
        // the first line's cap height sits on the box's top edge instead of
        // half a line below it.
        let trim = if block.leading_trim && index == 0 {
            styles[tallest].leading_trim(line_height)
        } else {
            0.0
        };
        let baseline = line_top + styles[tallest].baseline_offset(line_height) - trim;

        if baseline - styles[tallest].font_size > max_y {
            break;
        }

        // The indent is part of the first line, so alignment sees a line that
        // is that much wider and the text starts that much further in.
        let indent = if index == 0 { wrap.indent } else { 0.0 };
        let line_width = text::line_width(&line, &measure) + indent;
        let start = aligned_text_x(left, width, line_width, text_align);
        let mut cursor_x = start + indent;

        // The marker is drawn in the room the indent reserved for it, ahead of
        // the first line's own indent.
        if index == 0 {
            if let Some(marker) = block.list_marker {
                let mut marker_x = start + indent - marker_width;
                styles[0].draw(pixmap, marker, &mut marker_x, baseline);
            }
        }

        for run in &line {
            let style = &styles[run.style];
            // A shifted run rides above the line's shared baseline without
            // moving anything else on it.
            styles[run.style].draw(
                pixmap,
                &run.text,
                &mut cursor_x,
                baseline - style.baseline_shift,
            );
        }

        line_top += line_height - trim;
    }
}

fn load_default_font() -> Option<Font> {
    let candidates = [
        "/System/Library/Fonts/SFNS.ttf",
        "/System/Library/Fonts/SFCompact.ttf",
        "/System/Library/Fonts/Supplemental/Arial.ttf",
        "/System/Library/Fonts/Supplemental/Verdana.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
    ];

    candidates.iter().find_map(|path| {
        let bytes = fs::read(path).ok()?;
        Font::from_bytes(bytes, FontSettings::default()).ok()
    })
}

fn fontdue_baseline_offset(font: &Font, font_size: f32, line_height: f32) -> f32 {
    font.horizontal_line_metrics(font_size)
        .map(|metrics| {
            let content_height = metrics.ascent - metrics.descent;
            ((line_height - content_height) / 2.0) + metrics.ascent
        })
        .unwrap_or(font_size)
}

fn aligned_text_x(x: f32, width: f32, text_width: f32, align: Option<&str>) -> f32 {
    match align {
        Some("right") => x + (width - text_width).max(0.0),
        Some("center") | Some("middle") => x + ((width - text_width).max(0.0) / 2.0),
        _ => x,
    }
}

struct GlyphPathBuilder {
    builder: PathBuilder,
    offset_x: f32,
    baseline: f32,
    scale: f32,
}

impl GlyphPathBuilder {
    fn new(offset_x: f32, baseline: f32, scale: f32) -> Self {
        Self {
            builder: PathBuilder::new(),
            offset_x,
            baseline,
            scale,
        }
    }

    fn finish(self) -> Option<tiny_skia::Path> {
        self.builder.finish()
    }

    fn x(&self, x: f32) -> f32 {
        self.offset_x + x * self.scale
    }

    fn y(&self, y: f32) -> f32 {
        self.baseline - y * self.scale
    }
}

impl OutlineBuilder for GlyphPathBuilder {
    fn move_to(&mut self, x: f32, y: f32) {
        self.builder.move_to(self.x(x), self.y(y));
    }

    fn line_to(&mut self, x: f32, y: f32) {
        self.builder.line_to(self.x(x), self.y(y));
    }

    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        self.builder
            .quad_to(self.x(x1), self.y(y1), self.x(x), self.y(y));
    }

    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        self.builder.cubic_to(
            self.x(x1),
            self.y(y1),
            self.x(x2),
            self.y(y2),
            self.x(x),
            self.y(y),
        );
    }

    fn close(&mut self) {
        self.builder.close();
    }
}

fn text_width(font: &Font, value: &str, font_size: f32) -> f32 {
    value
        .chars()
        .map(|ch| font.metrics(ch, font_size).advance_width)
        .sum()
}

#[allow(clippy::too_many_arguments)]
fn draw_glyph_bitmap(
    pixmap: &mut Pixmap,
    x: f32,
    y: f32,
    width: usize,
    height: usize,
    bitmap: &[u8],
    color: Color,
    anti_alias: bool,
) {
    let Some((r, g, b, base_a)) = color_to_rgba8(color) else {
        return;
    };

    for row in 0..height {
        for col in 0..width {
            let raw = bitmap[row * width + col];
            // Without smoothing a pixel is either in the glyph or out of it,
            // so the rasteriser's partial coverage is rounded to one or other.
            let coverage = if anti_alias {
                raw
            } else if raw >= 128 {
                255
            } else {
                0
            };
            if coverage == 0 {
                continue;
            }
            let alpha = ((coverage as u16 * base_a as u16) / 255) as u8;
            let mut paint = Paint::default();
            paint.set_color(Color::from_rgba8(r, g, b, alpha));
            paint.anti_alias = false;
            if let Some(rect) = Rect::from_xywh(x + col as f32, y + row as f32, 1.0, 1.0) {
                pixmap.fill_rect(rect, &paint, Transform::identity(), None);
            }
        }
    }
}

fn color_to_rgba8(color: Color) -> Option<(u8, u8, u8, u8)> {
    Some((
        (color.red() * 255.0).round() as u8,
        (color.green() * 255.0).round() as u8,
        (color.blue() * 255.0).round() as u8,
        (color.alpha() * 255.0).round() as u8,
    ))
}

fn paint_text_placeholder(pixmap: &mut Pixmap, node: &SceneNode) {
    let color = node
        .fill_color()
        .and_then(|fill| parse_color(fill, draw_opacity(node)))
        .unwrap_or_else(|| Color::from_rgba8(20, 20, 20, 180));
    let h = (node.bounds.height * 0.42).clamp(2.0, node.bounds.height);
    let y = node.bounds.y + (node.bounds.height - h) / 2.0;
    fill_rounded_rect(
        pixmap,
        node.bounds.x,
        y,
        node.bounds.width,
        h,
        h / 2.0,
        color,
    );
}

/// How an `<img>` is drawn into its box, beyond which bytes to draw.
#[derive(Debug, Clone, Copy, Default)]
struct ImageStyle<'a> {
    fit: Option<&'a str>,
    object_position: Option<&'a str>,
    rendering: Option<&'a str>,
}

impl ImageStyle<'_> {
    /// Where the image sits in the leftover space, as a fraction per axis.
    ///
    /// CSS defaults `object-position` to `50% 50%`, which is the centring the
    /// fit modes did before this was read.
    fn alignment(&self) -> (f32, f32) {
        let Some(value) = self.object_position else {
            return (0.5, 0.5);
        };

        let mut parts = value.split_whitespace();
        let x = parts
            .next()
            .and_then(|part| position_fraction(part, Axis::Horizontal))
            .unwrap_or(0.5);
        let y = parts
            .next()
            .and_then(|part| position_fraction(part, Axis::Vertical))
            .unwrap_or(0.5);

        (x, y)
    }

    /// Resampling quality, from `image-rendering`.
    fn quality(&self) -> tiny_skia::FilterQuality {
        match self.rendering {
            // Both CSS values mean "do not smooth the pixels".
            Some("pixelated") | Some("crisp-edges") => tiny_skia::FilterQuality::Nearest,
            _ => tiny_skia::FilterQuality::Bilinear,
        }
    }
}

#[derive(Clone, Copy)]
enum Axis {
    Horizontal,
    Vertical,
}

/// One half of an `object-position`, as a fraction of the leftover space.
///
/// Only percentages and keywords are a fraction; a length has no denominator
/// here, so it is left to the caller's default rather than guessed at.
fn position_fraction(value: &str, axis: Axis) -> Option<f32> {
    match (value.trim(), axis) {
        ("left", Axis::Horizontal) | ("top", Axis::Vertical) => Some(0.0),
        ("right", Axis::Horizontal) | ("bottom", Axis::Vertical) => Some(1.0),
        ("center", _) => Some(0.5),
        (value, _) => value
            .strip_suffix('%')
            .and_then(|percentage| percentage.trim().parse::<f32>().ok())
            .map(|percentage| percentage / 100.0),
    }
}

fn paint_image(
    pixmap: &mut Pixmap,
    node: &SceneNode,
    src: &str,
    style: ImageStyle<'_>,
    asset_cache: Option<&AssetCache>,
) {
    if let Some(cache) = asset_cache {
        match cache.resolve(src) {
            Ok(asset) => {
                if asset.media_type.as_deref() == Some("image/svg+xml")
                    || looks_like_svg(&asset.bytes)
                {
                    match render_svg_asset(pixmap, node, &asset.bytes) {
                        Ok(_) => return,
                        Err(err) => {
                            eprintln!("warning: failed to render SVG asset '{}': {}", src, err)
                        }
                    }
                } else {
                    match render_raster_asset(pixmap, node, &asset.bytes, style) {
                        Ok(_) => return,
                        Err(err) => eprintln!(
                            "warning: failed to decode/render raster asset '{}': {}",
                            src, err
                        ),
                    }
                }
            }
            Err(err) => {
                eprintln!("warning: failed to resolve asset '{}': {}", src, err);
            }
        }
    }

    if let Some(icon_name) = material_icon_name(src) {
        let color = icon_color(src).unwrap_or_else(|| Color::from_rgba8(180, 190, 200, 220));
        paint_material_icon(pixmap, node, icon_name, color);
        return;
    }

    paint_image_placeholder(pixmap, node);
}

fn render_raster_asset(
    pixmap: &mut Pixmap,
    node: &SceneNode,
    bytes: &[u8],
    style: ImageStyle<'_>,
) -> Result<(), PaintError> {
    let img = image::load_from_memory(bytes).map_err(|err| PaintError::Asset(err.to_string()))?;
    let rgba = img.to_rgba8();
    // `image` decodes straight alpha; tiny-skia stores premultiplied. Skipping
    // this leaves transparent pixels too bright and haloes cut-out edges.
    let premultiplied = premultiply_rgba(rgba.as_raw());
    let img_pixmap = PixmapRef::from_bytes(&premultiplied, rgba.width(), rgba.height())
        .ok_or_else(|| PaintError::Asset("failed to create PixmapRef".to_owned()))?;

    let rx = node.bounds.x;
    let ry = node.bounds.y;
    let rw = node.bounds.width;
    let rh = node.bounds.height;

    let iw = rgba.width() as f32;
    let ih = rgba.height() as f32;

    // `object-position` decides where the image sits in whatever room the fit
    // mode leaves over. Centring is its default, which is what the fit modes
    // did on their own before it was read.
    let (align_x, align_y) = style.alignment();

    let (transform, needs_clip) = match style.fit.unwrap_or("fill") {
        "contain" => {
            let scale = (rw / iw).min(rh / ih);
            let dx = rx + (rw - iw * scale) * align_x;
            let dy = ry + (rh - ih * scale) * align_y;
            (
                Transform::from_scale(scale, scale).post_translate(dx, dy),
                false,
            )
        }
        "cover" => {
            let scale = (rw / iw).max(rh / ih);
            let dx = rx + (rw - iw * scale) * align_x;
            let dy = ry + (rh - ih * scale) * align_y;
            (
                Transform::from_scale(scale, scale).post_translate(dx, dy),
                true,
            )
        }
        "crop" | "none" => {
            let dx = rx + (rw - iw) * align_x;
            let dy = ry + (rh - ih) * align_y;
            (Transform::from_translate(dx, dy), true)
        }
        _ => {
            // `fill` stretches to both edges, so there is no room left for
            // `object-position` to place it in.
            let scale_x = rw / iw;
            let scale_y = rh / ih;
            (
                Transform::from_scale(scale_x, scale_y).post_translate(rx, ry),
                false,
            )
        }
    };

    let mask = if needs_clip {
        create_clip_mask(pixmap.width(), pixmap.height(), node)
    } else {
        None
    };

    pixmap.draw_pixmap(
        0,
        0,
        img_pixmap,
        &PixmapPaint {
            quality: style.quality(),
            ..Default::default()
        },
        transform,
        mask.as_ref(),
    );

    Ok(())
}

fn premultiply_rgba(straight: &[u8]) -> Vec<u8> {
    let mut premultiplied = Vec::with_capacity(straight.len());
    for pixel in straight.chunks_exact(4) {
        let alpha = pixel[3] as u32;
        for channel in &pixel[..3] {
            // Rounded `channel * alpha / 255`, which stays <= alpha and so is
            // always a valid premultiplied component.
            premultiplied.push(((*channel as u32 * alpha + 127) / 255) as u8);
        }
        premultiplied.push(pixel[3]);
    }
    premultiplied
}

fn looks_like_svg(bytes: &[u8]) -> bool {
    std::str::from_utf8(bytes)
        .map(|text| text.trim_start().starts_with("<svg"))
        .unwrap_or(false)
}

fn render_svg_asset(pixmap: &mut Pixmap, node: &SceneNode, bytes: &[u8]) -> Result<(), PaintError> {
    let target_width = node.bounds.width.ceil().max(1.0) as u32;
    let target_height = node.bounds.height.ceil().max(1.0) as u32;
    let options = resvg::usvg::Options::default();
    let tree = resvg::usvg::Tree::from_data(bytes, &options)
        .map_err(|err| PaintError::Svg(err.to_string()))?;
    let svg_size = tree.size();
    let scale_x = target_width as f32 / svg_size.width();
    let scale_y = target_height as f32 / svg_size.height();
    let mut icon_pixmap =
        Pixmap::new(target_width, target_height).ok_or(PaintError::Allocation {
            width: target_width,
            height: target_height,
        })?;

    resvg::render(
        &tree,
        Transform::from_scale(scale_x, scale_y),
        &mut icon_pixmap.as_mut(),
    );

    pixmap.draw_pixmap(
        node.bounds.x.round() as i32,
        node.bounds.y.round() as i32,
        icon_pixmap.as_ref(),
        &PixmapPaint::default(),
        Transform::identity(),
        None,
    );

    Ok(())
}

fn paint_image_placeholder(pixmap: &mut Pixmap, node: &SceneNode) {
    let fill = Color::from_rgba8(255, 255, 255, 40);
    fill_rounded_rect(
        pixmap,
        node.bounds.x,
        node.bounds.y,
        node.bounds.width,
        node.bounds.height,
        node.radius.unwrap_or(4.0).min(8.0),
        fill,
    );

    let stroke = Border {
        width: 1.0,
        widths: BorderWidths::uniform(1.0),
        color: "#9aa4ad".to_owned(),
        style: "solid".to_owned(),
        align: "inside".to_owned(),
    };
    stroke_rounded_rect(pixmap, node, &stroke);

    let Some(color) = parse_color("#9aa4ad", 1.0) else {
        return;
    };
    let mut paint = Paint::default();
    paint.set_color(color);
    paint.anti_alias = true;

    let cx = node.bounds.x + node.bounds.width / 2.0;
    let top = node.bounds.y + node.bounds.height * 0.24;
    let radius = (node.bounds.width.min(node.bounds.height) * 0.22).max(3.0);
    let stroke = Stroke {
        width: (node.bounds.width.min(node.bounds.height) * 0.09).clamp(1.0, 2.5),
        ..Stroke::default()
    };

    let mut question = PathBuilder::new();
    question.move_to(cx - radius * 0.55, top + radius * 0.45);
    question.cubic_to(
        cx - radius * 0.55,
        top - radius * 0.15,
        cx + radius * 0.75,
        top - radius * 0.18,
        cx + radius * 0.65,
        top + radius * 0.55,
    );
    question.cubic_to(
        cx + radius * 0.58,
        top + radius,
        cx,
        top + radius,
        cx,
        top + radius * 1.45,
    );
    if let Some(path) = question.finish() {
        pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
    }

    if let Some(dot) = PathBuilder::from_circle(
        cx,
        node.bounds.y + node.bounds.height * 0.74,
        stroke.width * 0.7,
    ) {
        pixmap.fill_path(&dot, &paint, FillRule::Winding, Transform::identity(), None);
    }
}

/// Paints the node's border stack, bottom entry first.
fn paint_border(pixmap: &mut Pixmap, node: &SceneNode) {
    for border in &node.borders {
        if border.widths.is_uniform() {
            stroke_rounded_rect(pixmap, node, border);
        } else {
            stroke_sided_border(pixmap, node, border);
        }
    }
}

/// Paints the node's outline: a stroke that sits outside the box, centred on
/// the edge pushed out by `outline-offset`.
///
/// Drawn after the borders, so an outline and a border on the same node stack
/// outwards in the order a reader expects.
fn paint_outline(pixmap: &mut Pixmap, node: &SceneNode) {
    let Some(outline) = &node.outline else {
        return;
    };
    if outline.width <= 0.0 {
        return;
    }
    let Some(color) = parse_color(&outline.color, draw_opacity(node)) else {
        return;
    };

    // The outline hugs the outside of the box, so its own width pushes it out
    // by half again on top of the offset.
    let inflate = outline.offset + outline.width / 2.0;
    let x = node.bounds.x - inflate;
    let y = node.bounds.y - inflate;
    let width = node.bounds.width + inflate * 2.0;
    let height = paint_height(node) + inflate * 2.0;

    let path = if node.tag == "ellipse" {
        ellipse_path(x, y, width, height)
    } else {
        // The outline follows the node's corners, growing with the box. A
        // square box keeps a square outline, as in CSS.
        let radius = match node.radius.filter(|radius| *radius > 0.0) {
            Some(radius) => (radius + inflate).max(0.0),
            None => 0.0,
        };
        smoothed_rect_path(x, y, width, height, radius, node.corner_smoothing)
    };
    let Some(path) = path else {
        return;
    };

    let mut paint = Paint::default();
    paint.set_color(color);
    paint.anti_alias = true;
    pixmap.stroke_path(
        &path,
        &paint,
        &Stroke {
            width: outline.width,
            ..Default::default()
        },
        Transform::identity(),
        None,
    );
}

fn fill_rounded_rect(
    pixmap: &mut Pixmap,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    radius: f32,
    color: Color,
) {
    fill_smoothed_rect(pixmap, x, y, width, height, radius, 0.0, color);
}

#[allow(clippy::too_many_arguments)]
fn fill_smoothed_rect(
    pixmap: &mut Pixmap,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    radius: f32,
    smoothing: f32,
    color: Color,
) {
    if width <= 0.0 || height <= 0.0 {
        return;
    }

    let mut paint = Paint::default();
    paint.set_color(color);
    paint.anti_alias = true;

    if (width - height).abs() < 0.01 && radius >= width / 2.0 {
        if let Some(path) = ellipse_path(x, y, width, height) {
            pixmap.fill_path(
                &path,
                &paint,
                FillRule::Winding,
                Transform::identity(),
                None,
            );
        }
        return;
    }

    if radius <= 0.0 {
        if let Some(rect) = Rect::from_xywh(x, y, width, height) {
            pixmap.fill_rect(rect, &paint, Transform::identity(), None);
        }
        return;
    }

    if let Some(path) = smoothed_rect_path(x, y, width, height, radius, smoothing) {
        pixmap.fill_path(
            &path,
            &paint,
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
}

fn stroke_rounded_rect(pixmap: &mut Pixmap, node: &SceneNode, border: &Border) {
    if border.width <= 0.0 || node.bounds.width <= 0.0 || node.bounds.height <= 0.0 {
        return;
    }
    let Some(color) = parse_color(&border.color, draw_opacity(node)) else {
        return;
    };

    let mut paint = Paint::default();
    paint.set_color(color);
    paint.anti_alias = true;
    let stroke = Stroke {
        width: border.width,
        ..Default::default()
    };

    let offset = match border.align.as_str() {
        "center" => 0.0,
        "outside" => -border.width / 2.0,
        _ => border.width / 2.0,
    };
    let delta_size = match border.align.as_str() {
        "center" => 0.0,
        "outside" => border.width,
        _ => -border.width,
    };

    if node.tag == "ellipse" {
        if let Some(path) = ellipse_path(
            node.bounds.x + offset,
            node.bounds.y + offset,
            (node.bounds.width + delta_size).max(0.0),
            (node.bounds.height + delta_size).max(0.0),
        ) {
            pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
        }
        return;
    }

    if let Some(path) = smoothed_rect_path(
        node.bounds.x + offset,
        node.bounds.y + offset,
        (node.bounds.width + delta_size).max(0.0),
        (node.bounds.height + delta_size).max(0.0),
        node.radius.unwrap_or(0.0),
        node.corner_smoothing,
    ) {
        pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
    }
}

fn stroke_sided_border(pixmap: &mut Pixmap, node: &SceneNode, border: &Border) {
    let Some(color) = parse_color(&border.color, draw_opacity(node)) else {
        return;
    };
    let x = node.bounds.x;
    let y = node.bounds.y;
    let w = node.bounds.width;
    let h = node.bounds.height;

    let align = border.align.as_str();

    if border.widths.top > 0.0 {
        let sw = border.widths.top;
        let dy = match align {
            "center" => 0.0,
            "outside" => -sw / 2.0,
            _ => sw / 2.0,
        };
        stroke_line(pixmap, x, y + dy, x + w, y + dy, sw, color);
    }
    if border.widths.right > 0.0 {
        let sw = border.widths.right;
        let dx = match align {
            "center" => 0.0,
            "outside" => sw / 2.0,
            _ => -sw / 2.0,
        };
        stroke_line(pixmap, x + w + dx, y, x + w + dx, y + h, sw, color);
    }
    if border.widths.bottom > 0.0 {
        let sw = border.widths.bottom;
        let dy = match align {
            "center" => 0.0,
            "outside" => sw / 2.0,
            _ => -sw / 2.0,
        };
        stroke_line(pixmap, x, y + h + dy, x + w, y + h + dy, sw, color);
    }
    if border.widths.left > 0.0 {
        let sw = border.widths.left;
        let dx = match align {
            "center" => 0.0,
            "outside" => -sw / 2.0,
            _ => sw / 2.0,
        };
        stroke_line(pixmap, x + dx, y, x + dx, y + h, sw, color);
    }
}

fn material_icon_name(src: &str) -> Option<&str> {
    let marker = "material-symbols/";
    let start = src.find(marker)? + marker.len();
    let end = src[start..].find(".svg")? + start;
    Some(&src[start..end])
}

fn icon_color(src: &str) -> Option<Color> {
    let query = src.split_once('?')?.1;
    let value = query
        .split('&')
        .find_map(|part| part.strip_prefix("color="))?
        .replace("%23", "#");
    parse_color(&value, 1.0)
}

fn paint_material_icon(pixmap: &mut Pixmap, node: &SceneNode, name: &str, color: Color) {
    let x = node.bounds.x;
    let y = node.bounds.y;
    let w = node.bounds.width;
    let h = node.bounds.height;
    let stroke_width = (w.min(h) * 0.1).clamp(1.4, 2.8);

    match name {
        "arrow-back-rounded" => {
            stroke_line(
                pixmap,
                x + w * 0.18,
                y + h * 0.5,
                x + w * 0.82,
                y + h * 0.5,
                stroke_width,
                color,
            );
            stroke_line(
                pixmap,
                x + w * 0.18,
                y + h * 0.5,
                x + w * 0.45,
                y + h * 0.22,
                stroke_width,
                color,
            );
            stroke_line(
                pixmap,
                x + w * 0.18,
                y + h * 0.5,
                x + w * 0.45,
                y + h * 0.78,
                stroke_width,
                color,
            );
        }
        "arrow-forward-rounded" => {
            stroke_line(
                pixmap,
                x + w * 0.18,
                y + h * 0.5,
                x + w * 0.82,
                y + h * 0.5,
                stroke_width,
                color,
            );
            stroke_line(
                pixmap,
                x + w * 0.82,
                y + h * 0.5,
                x + w * 0.55,
                y + h * 0.22,
                stroke_width,
                color,
            );
            stroke_line(
                pixmap,
                x + w * 0.82,
                y + h * 0.5,
                x + w * 0.55,
                y + h * 0.78,
                stroke_width,
                color,
            );
        }
        "keyboard-arrow-down-rounded" => {
            stroke_line(
                pixmap,
                x + w * 0.25,
                y + h * 0.38,
                x + w * 0.5,
                y + h * 0.62,
                stroke_width,
                color,
            );
            stroke_line(
                pixmap,
                x + w * 0.75,
                y + h * 0.38,
                x + w * 0.5,
                y + h * 0.62,
                stroke_width,
                color,
            );
        }
        "check-rounded" => {
            stroke_line(
                pixmap,
                x + w * 0.22,
                y + h * 0.52,
                x + w * 0.42,
                y + h * 0.72,
                stroke_width,
                color,
            );
            stroke_line(
                pixmap,
                x + w * 0.42,
                y + h * 0.72,
                x + w * 0.8,
                y + h * 0.28,
                stroke_width,
                color,
            );
        }
        "info-outline-rounded" | "help-outline-rounded" => {
            stroke_ellipse(
                pixmap,
                x + w * 0.12,
                y + h * 0.12,
                w * 0.76,
                h * 0.76,
                stroke_width,
                color,
            );
            if name == "info-outline-rounded" {
                fill_circle(
                    pixmap,
                    x + w * 0.5,
                    y + h * 0.32,
                    stroke_width * 0.65,
                    color,
                );
                stroke_line(
                    pixmap,
                    x + w * 0.5,
                    y + h * 0.45,
                    x + w * 0.5,
                    y + h * 0.7,
                    stroke_width,
                    color,
                );
            } else {
                stroke_line(
                    pixmap,
                    x + w * 0.38,
                    y + h * 0.38,
                    x + w * 0.5,
                    y + h * 0.25,
                    stroke_width,
                    color,
                );
                stroke_line(
                    pixmap,
                    x + w * 0.5,
                    y + h * 0.25,
                    x + w * 0.62,
                    y + h * 0.38,
                    stroke_width,
                    color,
                );
                stroke_line(
                    pixmap,
                    x + w * 0.62,
                    y + h * 0.38,
                    x + w * 0.5,
                    y + h * 0.54,
                    stroke_width,
                    color,
                );
                fill_circle(pixmap, x + w * 0.5, y + h * 0.7, stroke_width * 0.65, color);
            }
        }
        "signal-cellular-alt-rounded" => {
            fill_rounded_rect(
                pixmap,
                x + w * 0.18,
                y + h * 0.56,
                w * 0.14,
                h * 0.26,
                1.0,
                color,
            );
            fill_rounded_rect(
                pixmap,
                x + w * 0.42,
                y + h * 0.38,
                w * 0.14,
                h * 0.44,
                1.0,
                color,
            );
            fill_rounded_rect(
                pixmap,
                x + w * 0.66,
                y + h * 0.2,
                w * 0.14,
                h * 0.62,
                1.0,
                color,
            );
        }
        "wifi-rounded" => {
            stroke_arc_like(pixmap, x, y, w, h, 0.18, color, stroke_width);
            stroke_arc_like(pixmap, x, y, w, h, 0.36, color, stroke_width);
            fill_circle(
                pixmap,
                x + w * 0.5,
                y + h * 0.72,
                stroke_width * 0.85,
                color,
            );
        }
        "battery-full-alt-rounded" => {
            stroke_rect(
                pixmap,
                x + w * 0.12,
                y + h * 0.28,
                w * 0.68,
                h * 0.44,
                stroke_width,
                color,
            );
            fill_rounded_rect(
                pixmap,
                x + w * 0.8,
                y + h * 0.4,
                w * 0.08,
                h * 0.2,
                1.0,
                color,
            );
            fill_rounded_rect(
                pixmap,
                x + w * 0.22,
                y + h * 0.38,
                w * 0.48,
                h * 0.24,
                1.0,
                color,
            );
        }
        _ => paint_generic_icon(pixmap, node, color, stroke_width),
    }
}

fn paint_generic_icon(pixmap: &mut Pixmap, node: &SceneNode, color: Color, stroke_width: f32) {
    let x = node.bounds.x;
    let y = node.bounds.y;
    let w = node.bounds.width;
    let h = node.bounds.height;
    stroke_rounded_icon_rect(
        pixmap,
        x + w * 0.22,
        y + h * 0.2,
        w * 0.56,
        h * 0.6,
        stroke_width,
        color,
    );
    stroke_line(
        pixmap,
        x + w * 0.34,
        y + h * 0.38,
        x + w * 0.66,
        y + h * 0.38,
        stroke_width,
        color,
    );
}

fn stroke_line(pixmap: &mut Pixmap, x1: f32, y1: f32, x2: f32, y2: f32, width: f32, color: Color) {
    let mut pb = PathBuilder::new();
    pb.move_to(x1, y1);
    pb.line_to(x2, y2);
    if let Some(path) = pb.finish() {
        stroke_path(pixmap, &path, width, color);
    }
}

fn stroke_rect(
    pixmap: &mut Pixmap,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    stroke_width: f32,
    color: Color,
) {
    if let Some(path) = rounded_rect_path(x, y, width, height, stroke_width) {
        stroke_path(pixmap, &path, stroke_width, color);
    }
}

fn stroke_rounded_icon_rect(
    pixmap: &mut Pixmap,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    stroke_width: f32,
    color: Color,
) {
    if let Some(path) = rounded_rect_path(x, y, width, height, width.min(height) * 0.08) {
        stroke_path(pixmap, &path, stroke_width, color);
    }
}

fn stroke_ellipse(
    pixmap: &mut Pixmap,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    stroke_width: f32,
    color: Color,
) {
    if let Some(path) = ellipse_path(x, y, width, height) {
        stroke_path(pixmap, &path, stroke_width, color);
    }
}

// Geometry parameters; grouping them would not make a call site clearer.
#[allow(clippy::too_many_arguments)]
fn stroke_arc_like(
    pixmap: &mut Pixmap,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    inset: f32,
    color: Color,
    stroke_width: f32,
) {
    let mut pb = PathBuilder::new();
    pb.move_to(x + width * inset, y + height * 0.48);
    pb.quad_to(
        x + width * 0.5,
        y + height * (0.2 + inset * 0.25),
        x + width * (1.0 - inset),
        y + height * 0.48,
    );
    if let Some(path) = pb.finish() {
        stroke_path(pixmap, &path, stroke_width, color);
    }
}

fn stroke_path(pixmap: &mut Pixmap, path: &tiny_skia::Path, width: f32, color: Color) {
    let mut paint = Paint::default();
    paint.set_color(color);
    paint.anti_alias = true;
    let stroke = Stroke {
        width,
        ..Default::default()
    };
    pixmap.stroke_path(path, &paint, &stroke, Transform::identity(), None);
}

fn fill_circle(pixmap: &mut Pixmap, cx: f32, cy: f32, radius: f32, color: Color) {
    fill_rounded_rect(
        pixmap,
        cx - radius,
        cy - radius,
        radius * 2.0,
        radius * 2.0,
        radius,
        color,
    );
}

pub(crate) fn rounded_rect_path(
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    radius: f32,
) -> Option<tiny_skia::Path> {
    smoothed_rect_path(x, y, width, height, radius, 0.0)
}

/// A rounded rectangle whose corners can be squircles.
///
/// `smoothing` is the `corner-smoothing` factor, 0 to 1. At 0 the corner is
/// the usual circular arc. Above it the curve leaves the edge earlier — it
/// reaches `radius * (1 + smoothing)` along each side — while still passing
/// the same distance from the corner. Spreading the same turn over a longer
/// run is what takes the curvature jump out of the join and reads as a
/// squircle; a corner that merely grew its radius would just look rounder.
///
/// The handle length that holds the corner distance fixed is
/// `4p/3 - kr`, where `p` is the reach and `k = 8(1 - 1/sqrt(2))/3`. At
/// `smoothing = 0` that is exactly the 0.5523 circular constant, so this stays
/// a plain rounded rectangle until smoothing is asked for.
///
/// One cubic per corner is an approximation of Figma's construction, which
/// flanks a shortened arc with two of them.
fn smoothed_rect_path(
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    radius: f32,
    smoothing: f32,
) -> Option<tiny_skia::Path> {
    if width <= 0.0 || height <= 0.0 {
        return None;
    }

    let smoothing = smoothing.clamp(0.0, 1.0);
    let reach_factor = 1.0 + smoothing;
    // The reach, not the radius, is what has to fit: stopping at the midpoint
    // of each side keeps opposite corners from overlapping. A radius too big
    // for its smoothing therefore gives up radius rather than smoothness, and
    // at smoothing 0 this is the plain `radius <= min(w, h) / 2` clamp.
    let radius = radius
        .max(0.0)
        .min(width / 2.0 / reach_factor)
        .min(height / 2.0 / reach_factor);
    let r = radius * reach_factor;

    let mut pb = PathBuilder::new();
    if r == 0.0 {
        pb.move_to(x, y);
        pb.line_to(x + width, y);
        pb.line_to(x + width, y + height);
        pb.line_to(x, y + height);
        pb.close();
        return pb.finish();
    }

    const CORNER_DISTANCE: f32 = 0.781_048_6;
    let c = 4.0 * r / 3.0 - CORNER_DISTANCE * radius;
    pb.move_to(x + r, y);
    pb.line_to(x + width - r, y);
    pb.cubic_to(x + width - r + c, y, x + width, y + r - c, x + width, y + r);
    pb.line_to(x + width, y + height - r);
    pb.cubic_to(
        x + width,
        y + height - r + c,
        x + width - r + c,
        y + height,
        x + width - r,
        y + height,
    );
    pb.line_to(x + r, y + height);
    pb.cubic_to(
        x + r - c,
        y + height,
        x,
        y + height - r + c,
        x,
        y + height - r,
    );
    pb.line_to(x, y + r);
    pb.cubic_to(x, y + r - c, x + r - c, y, x + r, y);
    pb.close();
    pb.finish()
}

pub(crate) fn ellipse_path(x: f32, y: f32, width: f32, height: f32) -> Option<tiny_skia::Path> {
    if width <= 0.0 || height <= 0.0 {
        return None;
    }

    let rx = width / 2.0;
    let ry = height / 2.0;
    let cx = x + rx;
    let cy = y + ry;
    let c = 0.552_284_8;

    let mut pb = PathBuilder::new();
    pb.move_to(cx + rx, cy);
    pb.cubic_to(cx + rx, cy + ry * c, cx + rx * c, cy + ry, cx, cy + ry);
    pb.cubic_to(cx - rx * c, cy + ry, cx - rx, cy + ry * c, cx - rx, cy);
    pb.cubic_to(cx - rx, cy - ry * c, cx - rx * c, cy - ry, cx, cy - ry);
    pb.cubic_to(cx + rx * c, cy - ry, cx + rx, cy - ry * c, cx + rx, cy);
    pb.close();
    pb.finish()
}

fn parse_color(value: &str, opacity: f32) -> Option<Color> {
    let value = value.trim();
    if value == "none" || value == "transparent" || value.starts_with("linear-gradient") {
        return None;
    }
    let hex = value.strip_prefix('#')?;
    let (r, g, b, a) = match hex.len() {
        6 => (
            u8::from_str_radix(&hex[0..2], 16).ok()?,
            u8::from_str_radix(&hex[2..4], 16).ok()?,
            u8::from_str_radix(&hex[4..6], 16).ok()?,
            255,
        ),
        8 => (
            u8::from_str_radix(&hex[0..2], 16).ok()?,
            u8::from_str_radix(&hex[2..4], 16).ok()?,
            u8::from_str_radix(&hex[4..6], 16).ok()?,
            u8::from_str_radix(&hex[6..8], 16).ok()?,
        ),
        _ => return None,
    };
    let alpha = ((a as f32) * opacity.clamp(0.0, 1.0)).round() as u8;
    Some(Color::from_rgba8(r, g, b, alpha))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{build_scene, compute_taffy_layout, parse_gui_xml};

    #[test]
    fn paints_scene_to_png_file() {
        let document = parse_gui_xml(
            r##"
            <gui version="0.2">
              <col w="120" fill="#ffffff" p="10" gap="8">
                <rect w="40" h="20" fill="#0d99ff" radius="4" />
                <text value="Hello" fill="#111111" />
              </col>
            </gui>
            "##,
        )
        .expect("valid gui");
        let layout = compute_taffy_layout(&document).expect("layout computes");
        let scene = build_scene(&document, &layout);

        let path = std::env::temp_dir().join("dotgui-renderer-paint-smoke.png");
        paint_scene_to_png(&scene, &path).expect("png paints");
        let metadata = std::fs::metadata(&path).expect("png exists");
        assert!(metadata.len() > 0);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn paints_zero_height_line_as_divider() {
        let document = parse_gui_xml(
            r##"
            <gui version="0.2">
              <col w="80" h="10" fill="#ffffff">
                <line w="80" fill="#000000" />
              </col>
            </gui>
            "##,
        )
        .expect("valid gui");
        let layout = compute_taffy_layout(&document).expect("layout computes");
        let scene = build_scene(&document, &layout);

        let path = std::env::temp_dir().join("dotgui-renderer-line-smoke.png");
        paint_scene_to_png(&scene, &path).expect("png paints");
        let metadata = std::fs::metadata(&path).expect("png exists");
        assert!(metadata.len() > 0);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn an_outline_paints_outside_the_box_across_the_offset() {
        // A 20x20 black box at (10, 10) in a 60x60 white canvas, outlined 2px
        // red 4px out: the ring lands at 4..6px from the edge, so x = 5 is on
        // it and x = 8 is in the gap the offset leaves.
        let painted = render(
            r##"
            <gui version="0.2">
              <frame w="60" h="60" fill="#ffffff">
                <rect abs x="10" y="10" w="20" h="20" fill="#000000"
                      outline="2 #ff0000" outline-offset="4" />
              </frame>
            </gui>
            "##,
        );

        assert_eq!(
            painted.get_pixel(5, 20).0[0..3],
            [255, 0, 0],
            "on the outline"
        );
        assert_eq!(
            painted.get_pixel(8, 20).0[0..3],
            [255, 255, 255],
            "in the gap"
        );
        assert_eq!(
            painted.get_pixel(20, 20).0[0..3],
            [0, 0, 0],
            "the box itself"
        );
    }

    #[test]
    fn an_outline_does_not_paint_without_a_width() {
        let painted = render(
            r##"
            <gui version="0.2">
              <frame w="40" h="40" fill="#ffffff">
                <rect abs x="10" y="10" w="20" h="20" fill="#000000" outline="#ff0000" />
              </frame>
            </gui>
            "##,
        );

        assert_eq!(painted.get_pixel(5, 20).0[0..3], [255, 255, 255]);
    }

    #[test]
    fn corner_smoothing_changes_the_corners_and_only_the_corners() {
        let box_of = |smoothing: &str| {
            render(&format!(
                r##"
                <gui version="0.2">
                  <frame w="120" h="120" fill="#ffffff">
                    <rect abs x="10" y="10" w="100" h="100" radius="20" fill="#000000"
                          corner-smoothing="{smoothing}" />
                  </frame>
                </gui>
                "##
            ))
        };

        let plain = box_of("0");
        let smoothed = box_of("1");

        // The corner curve now reaches radius * (1 + smoothing) = 40px along
        // each side, so every difference has to sit inside a 40px corner
        // square of the 100px box.
        let mut differences = 0;
        for y in 0..120 {
            for x in 0..120 {
                if plain.get_pixel(x, y) == smoothed.get_pixel(x, y) {
                    continue;
                }
                differences += 1;
                let (dx, dy) = (x as i32 - 10, y as i32 - 10);
                let in_corner = |d: i32| !(40..100 - 40).contains(&d);
                assert!(
                    in_corner(dx) && in_corner(dy),
                    "pixel ({x}, {y}) differs outside a corner"
                );
            }
        }

        assert!(differences > 0, "corner-smoothing changed nothing");
    }

    #[test]
    fn corner_smoothing_keeps_the_corner_the_same_distance_away() {
        // Smoothing spreads the turn over more of the edge; it must not round
        // the corner off harder. The diagonal is where that would show.
        let box_of = |smoothing: &str| {
            render(&format!(
                r##"
                <gui version="0.2">
                  <frame w="120" h="120" fill="#ffffff">
                    <rect abs x="10" y="10" w="100" h="100" radius="20" fill="#000000"
                          corner-smoothing="{smoothing}" />
                  </frame>
                </gui>
                "##
            ))
        };

        let corner_reach = |painted: &image::RgbaImage| {
            (0..40)
                .find(|step| painted.get_pixel(10 + step, 10 + step)[0] < 128)
                .expect("the diagonal enters the box")
        };

        assert_eq!(corner_reach(&box_of("0")), corner_reach(&box_of("1")));
    }

    #[test]
    fn overflow_hidden_on_one_axis_clips_only_that_axis() {
        // A 20x20 box at (10, 10) holding a child that runs off both its right
        // and its bottom edge. With `overflow-x="hidden"` the overhang to the
        // right is cut and the one below survives.
        let painted = render(
            r##"
            <gui version="0.2">
              <frame w="60" h="60" fill="#ffffff">
                <frame abs x="10" y="10" w="20" h="20" fill="#eeeeee" overflow-x="hidden">
                  <rect abs x="0" y="0" w="40" h="40" fill="#ff0000" />
                </frame>
              </frame>
            </gui>
            "##,
        );

        assert_eq!(
            painted.get_pixel(20, 20).0[0..3],
            [255, 0, 0],
            "inside the box"
        );
        assert_eq!(
            painted.get_pixel(35, 20).0[0..3],
            [255, 255, 255],
            "past the right edge, clipped by overflow-x"
        );
        assert_eq!(
            painted.get_pixel(20, 35).0[0..3],
            [255, 0, 0],
            "past the bottom edge, left alone"
        );
    }

    #[test]
    fn clip_without_overflow_still_clips_both_axes() {
        let painted = render(
            r##"
            <gui version="0.2">
              <frame w="60" h="60" fill="#ffffff">
                <frame abs x="10" y="10" w="20" h="20" fill="#eeeeee" clip>
                  <rect abs x="0" y="0" w="40" h="40" fill="#ff0000" />
                </frame>
              </frame>
            </gui>
            "##,
        );

        assert_eq!(painted.get_pixel(20, 20).0[0..3], [255, 0, 0]);
        assert_eq!(painted.get_pixel(35, 20).0[0..3], [255, 255, 255]);
        assert_eq!(painted.get_pixel(20, 35).0[0..3], [255, 255, 255]);
    }

    #[test]
    fn blend_multiply_darkens_against_what_is_behind_it() {
        // Mid grey over mid grey: multiply gives 0.5 * 0.5 = 0.25, so ~64.
        let painted = render(
            r##"
            <gui version="0.2">
              <frame w="20" h="20" fill="#808080">
                <rect abs x="0" y="0" w="20" h="20" fill="#808080" blend="multiply" />
              </frame>
            </gui>
            "##,
        );

        let red = painted.get_pixel(10, 10).0[0];
        assert!(
            (58..=70).contains(&red),
            "expected the product near 64, painted {red}"
        );
    }

    #[test]
    fn a_normal_blend_paints_the_same_as_none_at_all() {
        let with = render(
            r##"
            <gui version="0.2">
              <frame w="20" h="20" fill="#808080">
                <rect abs x="0" y="0" w="20" h="20" fill="#4080c0" blend="normal" />
              </frame>
            </gui>
            "##,
        );
        let without = render(
            r##"
            <gui version="0.2">
              <frame w="20" h="20" fill="#808080">
                <rect abs x="0" y="0" w="20" h="20" fill="#4080c0" />
              </frame>
            </gui>
            "##,
        );

        assert_eq!(with.get_pixel(10, 10), without.get_pixel(10, 10));
    }

    #[test]
    fn isolation_keeps_a_childs_blend_off_the_outer_backdrop() {
        // The blending child sits on a transparent group. Isolated, it has
        // nothing to multiply against and stays its own colour; without the
        // isolation it darkens against the red page behind.
        let page = |isolation: &str| {
            render(&format!(
                r##"
                <gui version="0.2">
                  <frame w="20" h="20" fill="#ff0000">
                    <frame abs x="0" y="0" w="20" h="20" {isolation}>
                      <rect abs x="0" y="0" w="20" h="20" fill="#808080" blend="multiply" />
                    </frame>
                  </frame>
                </gui>
                "##
            ))
        };

        let isolated = page("isolation").get_pixel(10, 10).0[0..3].to_vec();
        let open = page("").get_pixel(10, 10).0[0..3].to_vec();

        assert_eq!(isolated, vec![128, 128, 128], "nothing behind to multiply");
        assert_eq!(open, vec![128, 0, 0], "multiplied against the red page");
    }

    #[test]
    fn a_filter_runs_over_the_finished_subtree() {
        // The filter is on the parent, so it has to reach the child too.
        let painted = render(
            r##"
            <gui version="0.2">
              <frame w="20" h="20" fill="#ffffff">
                <frame abs x="0" y="0" w="20" h="20" filter="grayscale(1)">
                  <rect abs x="0" y="0" w="20" h="20" fill="#ff0000" />
                </frame>
              </frame>
            </gui>
            "##,
        );

        let [r, g, b, _] = painted.get_pixel(10, 10).0;
        assert_eq!((r, g, b), (54, 54, 54));
    }

    #[test]
    fn z_index_lifts_a_child_over_a_later_sibling() {
        let painted = render(
            r##"
            <gui version="0.2">
              <frame w="20" h="20" fill="#ffffff">
                <rect abs x="0" y="0" w="20" h="20" fill="#ff0000" z-index="1" />
                <rect abs x="0" y="0" w="20" h="20" fill="#0000ff" />
              </frame>
            </gui>
            "##,
        );

        assert_eq!(
            painted.get_pixel(10, 10).0[0..3],
            [255, 0, 0],
            "the red rect is listed first but lifted above the blue one"
        );
    }

    #[test]
    fn a_masking_child_shapes_its_parent_and_is_not_drawn() {
        // The mask is a 10x10 square in the corner of a 20x20 red group, so
        // only that corner survives — and the mask's own fill never appears.
        let painted = render(
            r##"
            <gui version="0.2">
              <frame w="20" h="20" fill="#ffffff">
                <group abs x="0" y="0" w="20" h="20">
                  <rect abs x="0" y="0" w="10" h="10" fill="#00ff00" mask="true" />
                  <rect abs x="0" y="0" w="20" h="20" fill="#ff0000" />
                </group>
              </frame>
            </gui>
            "##,
        );

        assert_eq!(
            painted.get_pixel(5, 5).0[0..3],
            [255, 0, 0],
            "inside the mask, the red rect shows"
        );
        assert_eq!(
            painted.get_pixel(15, 15).0[0..3],
            [255, 255, 255],
            "outside the mask, nothing does"
        );
    }

    #[test]
    fn a_clip_path_cuts_the_node_and_not_only_its_children() {
        // `clip` holds children in but leaves the node's own fill alone; a
        // clip-path shapes the fill too.
        let painted = render(
            r##"
            <gui version="0.2">
              <frame w="40" h="40" fill="#ffffff">
                <frame abs x="0" y="0" w="40" h="40" fill="#ff0000"
                       clip-path="inset(10px)" />
              </frame>
            </gui>
            "##,
        );

        assert_eq!(painted.get_pixel(20, 20).0[0..3], [255, 0, 0], "inside");
        assert_eq!(
            painted.get_pixel(5, 20).0[0..3],
            [255, 255, 255],
            "cut away"
        );
    }

    #[test]
    fn a_polygon_clip_path_keeps_its_own_side_of_the_edge() {
        // A triangle over the top-left half of the box.
        let painted = render(
            r##"
            <gui version="0.2">
              <frame w="40" h="40" fill="#ffffff">
                <frame abs x="0" y="0" w="40" h="40" fill="#ff0000"
                       clip-path="polygon(0% 0%, 100% 0%, 0% 100%)" />
              </frame>
            </gui>
            "##,
        );

        assert_eq!(
            painted.get_pixel(5, 5).0[0..3],
            [255, 0, 0],
            "above the edge"
        );
        assert_eq!(
            painted.get_pixel(35, 35).0[0..3],
            [255, 255, 255],
            "below the edge"
        );
    }

    #[test]
    fn an_svg_mask_source_shapes_the_group() {
        // A circle of radius 10 centred in a 40x40 mask, so the middle
        // survives and the corner does not.
        let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="40">
              <circle cx="20" cy="20" r="10" fill="#ffffff"/></svg>"##;

        let document = parse_gui_xml(
            r##"
            <gui version="0.2">
              <frame w="40" h="40" fill="#ffffff">
                <group abs x="0" y="0" w="40" h="40" mask-src="assets/mask.svg">
                  <rect abs x="0" y="0" w="40" h="40" fill="#ff0000" />
                </group>
              </frame>
            </gui>
            "##,
        )
        .expect("valid gui");
        let layout = compute_taffy_layout(&document).expect("layout computes");
        let scene = build_scene(&document, &layout);

        let mut package_assets = std::collections::BTreeMap::new();
        package_assets.insert("assets/mask.svg".to_owned(), svg.to_vec());
        let cache = AssetCache::new(std::env::temp_dir()).with_package_assets(package_assets);

        let png = paint_scene_to_png_bytes(&scene, Some(&cache), None).expect("scene paints");
        let painted = image::load_from_memory(&png)
            .expect("painted png decodes")
            .to_rgba8();

        assert_eq!(
            painted.get_pixel(20, 20).0[0..3],
            [255, 0, 0],
            "in the circle"
        );
        assert_eq!(
            painted.get_pixel(2, 2).0[0..3],
            [255, 255, 255],
            "outside it"
        );
    }

    #[test]
    fn mask_composite_subtract_cuts_the_shape_out_instead() {
        let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="40">
              <circle cx="20" cy="20" r="10" fill="#ffffff"/></svg>"##;

        let painted = |composite: &str| {
            let document = parse_gui_xml(&format!(
                r##"
                <gui version="0.2">
                  <frame w="40" h="40" fill="#ffffff">
                    <group abs x="0" y="0" w="40" h="40" mask-src="assets/mask.svg"
                           mask-composite="{composite}">
                      <rect abs x="0" y="0" w="40" h="40" fill="#ff0000" />
                    </group>
                  </frame>
                </gui>
                "##
            ))
            .expect("valid gui");
            let layout = compute_taffy_layout(&document).expect("layout computes");
            let scene = build_scene(&document, &layout);

            let mut package_assets = std::collections::BTreeMap::new();
            package_assets.insert("assets/mask.svg".to_owned(), svg.to_vec());
            let cache = AssetCache::new(std::env::temp_dir()).with_package_assets(package_assets);
            let png = paint_scene_to_png_bytes(&scene, Some(&cache), None).expect("scene paints");
            image::load_from_memory(&png)
                .expect("painted png decodes")
                .to_rgba8()
        };

        let cut = painted("subtract");
        assert_eq!(
            cut.get_pixel(20, 20).0[0..3],
            [255, 255, 255],
            "the circle is what is removed"
        );
        assert_eq!(cut.get_pixel(2, 2).0[0..3], [255, 0, 0], "the rest stays");
    }

    #[test]
    fn a_mask_that_cannot_be_resolved_leaves_the_node_alone() {
        // No asset cache, so `mask-src` resolves to nothing. Blanking the node
        // would lose content over a missing file.
        let painted = render(
            r##"
            <gui version="0.2">
              <frame w="20" h="20" fill="#ffffff">
                <group abs x="0" y="0" w="20" h="20" mask-src="assets/missing.svg">
                  <rect abs x="0" y="0" w="20" h="20" fill="#ff0000" />
                </group>
              </frame>
            </gui>
            "##,
        );

        assert_eq!(painted.get_pixel(10, 10).0[0..3], [255, 0, 0]);
    }

    #[test]
    fn rotation_turns_a_node_about_its_centre() {
        // A wide bar across the middle of a 40x40 box, turned a quarter turn:
        // it ends up vertical, so the pixels it covered and the ones it now
        // covers swap.
        let bar = |rotation: &str| {
            render(&format!(
                r##"
                <gui version="0.2">
                  <frame w="40" h="40" fill="#ffffff">
                    <rect abs x="0" y="15" w="40" h="10" fill="#ff0000"
                          rotation="{rotation}" />
                  </frame>
                </gui>
                "##
            ))
        };

        let flat = bar("0");
        assert_eq!(flat.get_pixel(4, 20).0[0..3], [255, 0, 0], "along the bar");
        assert_eq!(flat.get_pixel(20, 4).0[0..3], [255, 255, 255], "above it");

        let turned = bar("90");
        assert_eq!(
            turned.get_pixel(20, 4).0[0..3],
            [255, 0, 0],
            "the bar now runs the other way"
        );
        assert_eq!(turned.get_pixel(4, 20).0[0..3], [255, 255, 255]);
    }

    #[test]
    fn transform_origin_moves_the_pivot() {
        // Rotated about the top-left corner, a quarter turn sweeps the bar off
        // the canvas to the left rather than standing it up in place.
        let painted = render(
            r##"
            <gui version="0.2">
              <frame w="40" h="40" fill="#ffffff">
                <rect abs x="0" y="0" w="40" h="10" fill="#ff0000"
                      rotation="90" transform-origin="top-left" />
              </frame>
            </gui>
            "##,
        );

        assert_eq!(
            painted.get_pixel(4, 20).0[0..3],
            [255, 255, 255],
            "nothing is left in the box"
        );
    }

    #[test]
    fn flip_mirrors_the_node() {
        // A marker in the left quarter of the box moves to the right quarter.
        let painted = |flip: &str| {
            render(&format!(
                r##"
                <gui version="0.2">
                  <frame w="40" h="40" fill="#ffffff">
                    <frame abs x="0" y="0" w="40" h="40" {flip}>
                      <rect abs x="0" y="15" w="10" h="10" fill="#ff0000" />
                    </frame>
                  </frame>
                </gui>
                "##
            ))
        };

        let plain = painted("");
        assert_eq!(plain.get_pixel(5, 20).0[0..3], [255, 0, 0]);

        let mirrored = painted(r#"flip="h""#);
        assert_eq!(
            mirrored.get_pixel(34, 20).0[0..3],
            [255, 0, 0],
            "moved over"
        );
        assert_eq!(mirrored.get_pixel(5, 20).0[0..3], [255, 255, 255]);
    }

    #[test]
    fn scale_grows_the_node_about_its_centre() {
        let painted = render(
            r##"
            <gui version="0.2">
              <frame w="40" h="40" fill="#ffffff">
                <rect abs x="10" y="10" w="20" h="20" fill="#ff0000" scale-x="2" />
              </frame>
            </gui>
            "##,
        );

        // 20 wide about a centre at x=20 becomes 40 wide, so it reaches the edge.
        assert_eq!(painted.get_pixel(2, 20).0[0..3], [255, 0, 0]);
        assert_eq!(
            painted.get_pixel(20, 5).0[0..3],
            [255, 255, 255],
            "y untouched"
        );
    }

    #[test]
    fn a_mask_travels_with_its_nodes_transform() {
        // The mask is applied in the node's own space, so turning the node
        // turns the masked shape with it rather than leaving it behind.
        let painted = render(
            r##"
            <gui version="0.2">
              <frame w="40" h="40" fill="#ffffff">
                <frame abs x="0" y="0" w="40" h="40" fill="#ff0000"
                       clip-path="inset(0 30px 0 0)" rotation="90" />
              </frame>
            </gui>
            "##,
        );

        // The strip runs down the left edge; a quarter turn puts it along the top.
        assert_eq!(painted.get_pixel(20, 5).0[0..3], [255, 0, 0]);
        assert_eq!(painted.get_pixel(5, 20).0[0..3], [255, 255, 255]);
    }

    /// A 2x2 image of four flat colours, for testing how it is resampled.
    fn quad_image() -> Vec<u8> {
        let mut image = image::RgbaImage::new(2, 2);
        image.put_pixel(0, 0, image::Rgba([255, 0, 0, 255]));
        image.put_pixel(1, 0, image::Rgba([0, 0, 255, 255]));
        image.put_pixel(0, 1, image::Rgba([0, 0, 255, 255]));
        image.put_pixel(1, 1, image::Rgba([255, 0, 0, 255]));

        let mut bytes = Vec::new();
        image
            .write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Png,
            )
            .expect("test image encodes");
        bytes
    }

    fn render_with_asset(xml: &str, src: &str, bytes: Vec<u8>) -> image::RgbaImage {
        let document = parse_gui_xml(xml).expect("valid gui");
        let layout = compute_taffy_layout(&document).expect("layout computes");
        let scene = build_scene(&document, &layout);

        let mut package_assets = std::collections::BTreeMap::new();
        package_assets.insert(src.to_owned(), bytes);
        let cache = AssetCache::new(std::env::temp_dir()).with_package_assets(package_assets);

        let png = paint_scene_to_png_bytes(&scene, Some(&cache), None).expect("scene paints");
        image::load_from_memory(&png)
            .expect("painted png decodes")
            .to_rgba8()
    }

    #[test]
    fn image_rendering_pixelated_keeps_hard_edges() {
        // A 2x2 checker blown up to 40x40. Smoothed, the midpoint between two
        // cells is a blend; pixelated, it is one cell or the other.
        let page = |rendering: &str| {
            render_with_asset(
                &format!(
                    r##"
                    <gui version="0.2">
                      <frame w="40" h="40" fill="#ffffff">
                        <img abs x="0" y="0" w="40" h="40" src="assets/quad.png"
                             fit="fill" {rendering} />
                      </frame>
                    </gui>
                    "##
                ),
                "assets/quad.png",
                quad_image(),
            )
        };

        let pixelated = page(r#"image-rendering="pixelated""#);
        let edge = pixelated.get_pixel(19, 10).0;
        assert!(
            edge[1] == 0,
            "a hard edge keeps the source colours, painted {edge:?}"
        );

        let smooth = page("");
        let blended = smooth.get_pixel(19, 10).0;
        assert_ne!(blended, edge, "the default smooths the same pixel instead");
    }

    #[test]
    fn object_position_moves_the_image_in_its_box() {
        // A 2x2 image drawn at its own size inside a 40x40 box: `crop` leaves
        // 38px of slack, which object-position decides how to spend.
        let page = |position: &str| {
            render_with_asset(
                &format!(
                    r##"
                    <gui version="0.2">
                      <frame w="40" h="40" fill="#ffffff">
                        <img abs x="0" y="0" w="40" h="40" src="assets/quad.png"
                             fit="crop" {position} />
                      </frame>
                    </gui>
                    "##
                ),
                "assets/quad.png",
                quad_image(),
            )
        };

        let painted = |img: &image::RgbaImage| {
            (0..40)
                .flat_map(|y| (0..40).map(move |x| (x, y)))
                .find(|(x, y)| img.get_pixel(*x, *y).0[0..3] != [255, 255, 255])
        };

        assert_eq!(painted(&page("")), Some((19, 19)), "centred by default");
        assert_eq!(
            painted(&page(r#"object-position="0% 0%""#)),
            Some((0, 0)),
            "pinned to the top left"
        );
        assert_eq!(
            painted(&page(r#"object-position="right bottom""#)),
            Some((38, 38)),
            "pinned to the bottom right"
        );
    }

    #[test]
    fn font_smoothing_none_removes_partial_coverage() {
        let page = |smoothing: &str| {
            render(&format!(
                r##"
                <gui version="0.2">
                  <frame w="80" h="40" fill="#ffffff">
                    <text abs x="4" y="4" w="72" h="32" value="Ago" fill="#000000"
                          font-size="28" {smoothing} />
                  </frame>
                </gui>
                "##
            ))
        };

        let partial = |img: &image::RgbaImage| {
            (0..40)
                .flat_map(|y| (0..80).map(move |x| (x, y)))
                .filter(|(x, y)| {
                    let grey = img.get_pixel(*x, *y).0[0];
                    grey > 8 && grey < 247
                })
                .count()
        };

        let smoothed = partial(&page(""));
        let hard = partial(&page(r#"font-smoothing="none""#));

        assert!(smoothed > 0, "antialiasing leaves partial pixels");
        assert!(
            hard < smoothed / 4,
            "smoothing off should all but remove them: {hard} vs {smoothed}"
        );
    }

    #[test]
    fn line_thickness_sets_a_dividers_height() {
        let painted = |thickness: &str| {
            render(&format!(
                r##"
                <gui version="0.2">
                  <col w="20" h="12" fill="#ffffff">
                    <line w="20" fill="#000000" {thickness} />
                  </col>
                </gui>
                "##
            ))
        };

        let rows =
            |img: &image::RgbaImage| (0..12).filter(|y| img.get_pixel(10, *y).0[0] < 128).count();

        assert_eq!(rows(&painted("")), 1, "one pixel by default");
        assert_eq!(rows(&painted(r#"thickness="4""#)), 4);
    }

    #[test]
    fn paragraph_indent_moves_the_first_line_only() {
        let painted = render(
            r##"
            <gui version="0.2">
              <frame w="80" h="60" fill="#ffffff">
                <text abs x="0" y="0" w="80" h="60" value="wwww wwww" fill="#000000"
                      font-size="14" line-height="20" paragraph-indent="30" />
              </frame>
            </gui>
            "##,
        );

        let first_ink = |top: u32, bottom: u32| {
            (0..80).find(|x| (top..bottom).any(|y| painted.get_pixel(*x, y).0[0] < 200))
        };

        let first_line = first_ink(0, 20).expect("the first line is drawn");
        let second_line = first_ink(20, 40).expect("the second line is drawn");

        assert!(
            first_line >= 30,
            "the first line starts past the indent, at {first_line}"
        );
        assert!(
            second_line < 10,
            "the second line starts at the edge, at {second_line}"
        );
    }

    #[test]
    fn word_spacing_pushes_the_words_apart() {
        let end_of_ink = |spacing: &str| {
            let painted = render(&format!(
                r##"
                <gui version="0.2">
                  <frame w="200" h="30" fill="#ffffff">
                    <text abs x="0" y="0" w="200" h="30" value="a b c" fill="#000000"
                          font-size="14" white-space="nowrap" {spacing} />
                  </frame>
                </gui>
                "##
            ));
            (0..200)
                .rev()
                .find(|x| (0..30).any(|y| painted.get_pixel(*x, y).0[0] < 200))
                .expect("something is drawn")
        };

        let plain = end_of_ink("");
        let spaced = end_of_ink(r#"word-spacing="10""#);

        // Two spaces, ten pixels each, so the last glyph lands ~20px later.
        assert!(
            (18..=22).contains(&(spaced - plain)),
            "expected about 20px later, got {}",
            spaced - plain
        );
    }

    /// The first row of the box that has any ink in it.
    fn first_inked_row(img: &image::RgbaImage, width: u32, height: u32) -> Option<u32> {
        (0..height).find(|y| (0..width).any(|x| img.get_pixel(x, *y).0[0] < 200))
    }

    #[test]
    fn vertical_align_moves_a_short_block_down_its_box() {
        let page = |align: &str| {
            render(&format!(
                r##"
                <gui version="0.2">
                  <frame w="60" h="90" fill="#ffffff">
                    <text abs x="0" y="0" w="60" h="90" value="Hi" fill="#000000"
                          font-size="14" line-height="20" {align} />
                  </frame>
                </gui>
                "##
            ))
        };

        let top = first_inked_row(&page(""), 60, 90).expect("drawn");
        let middle = first_inked_row(&page(r#"vertical-align="center""#), 60, 90).expect("drawn");
        let bottom = first_inked_row(&page(r#"vertical-align="bottom""#), 60, 90).expect("drawn");

        assert!(top < middle, "centre sits below top: {top} vs {middle}");
        assert!(
            middle < bottom,
            "bottom sits below centre: {middle} vs {bottom}"
        );
        // 90 tall, a 20px line: the slack is 70, so bottom starts ~70 lower.
        assert!(
            (65..=75).contains(&(bottom - top)),
            "expected about 70px of travel, got {}",
            bottom - top
        );
    }

    #[test]
    fn leading_trim_pulls_the_first_line_up_to_its_cap_height() {
        let page = |trim: &str| {
            render(&format!(
                r##"
                <gui version="0.2">
                  <frame w="60" h="60" fill="#ffffff">
                    <text abs x="0" y="0" w="60" h="60" value="H" fill="#000000"
                          font-size="14" line-height="30" {trim} />
                  </frame>
                </gui>
                "##
            ))
        };

        let untrimmed = first_inked_row(&page(""), 60, 60).expect("drawn");
        let trimmed =
            first_inked_row(&page(r#"leading-trim="cap-height""#), 60, 60).expect("drawn");

        assert!(
            trimmed < untrimmed,
            "the trimmed cap starts higher: {trimmed} vs {untrimmed}"
        );
        assert!(
            trimmed <= 1,
            "and lands on the box's top edge, at {trimmed}"
        );
    }

    #[test]
    fn a_list_marker_is_drawn_and_the_text_follows_it() {
        let page = |list: &str| {
            render(&format!(
                r##"
                <gui version="0.2">
                  <frame w="90" h="30" fill="#ffffff">
                    <text abs x="0" y="0" w="90" h="30" value="Item" fill="#000000"
                          font-size="14" {list} />
                  </frame>
                </gui>
                "##
            ))
        };

        let first_ink = |img: &image::RgbaImage| {
            (0..90).find(|x| (0..30).any(|y| img.get_pixel(*x, y).0[0] < 200))
        };

        let plain = page("");
        let bulleted = page(r#"list="disc""#);

        assert!(
            first_ink(&plain).is_some_and(|x| x <= 2),
            "plain text starts at the edge"
        );
        assert!(
            first_ink(&bulleted).is_some_and(|x| x <= 2),
            "so does the marker that replaces it"
        );

        // The marker adds ink the plain version does not have, and pushes the
        // word right, so the two renders differ well past the start.
        let differing = (0..90)
            .filter(|x| (0..30).any(|y| plain.get_pixel(*x, y) != bulleted.get_pixel(*x, y)));
        assert!(
            differing.count() > 5,
            "the marker should change the line's layout"
        );
    }

    #[test]
    fn baseline_shift_lifts_one_run_off_the_shared_baseline() {
        // The same document twice, so the comparison is one glyph against
        // itself rather than two different letters against each other.
        let page = |shift: &str| {
            render(&format!(
                r##"
                <gui version="0.2">
                  <frame w="80" h="40" fill="#ffffff">
                    <text abs x="0" y="0" w="80" h="40" fill="#000000" font-size="14"
                          line-height="30" white-space="nowrap">
                      <segment value="H" />
                      <segment value="H" {shift} />
                    </text>
                  </frame>
                </gui>
                "##
            ))
        };

        // The topmost inked row across the whole box: the shifted run is the
        // only thing that can rise above the unshifted line.
        let top = |img: &image::RgbaImage| {
            (0..40)
                .find(|y| (0..80).any(|x| img.get_pixel(x, *y).0[0] < 200))
                .expect("something is drawn")
        };

        let flat = top(&page(""));
        let lifted = top(&page(r#"baseline-shift="8""#));

        assert_eq!(
            flat - lifted,
            8,
            "the shifted run rides exactly the 8px asked for"
        );
    }

    #[test]
    fn a_linear_gradient_runs_between_its_stops() {
        let painted = render(
            r##"
            <gui version="0.2">
              <frame w="40" h="40" fill="#ffffff">
                <rect abs x="0" y="0" w="40" h="40"
                      fill="linear-gradient(180deg, #000000 0%, #ffffff 100%)" />
              </frame>
            </gui>
            "##,
        );

        let top = painted.get_pixel(20, 1).0[0];
        let middle = painted.get_pixel(20, 20).0[0];
        let bottom = painted.get_pixel(20, 38).0[0];

        assert!(top < 20, "starts black, got {top}");
        assert!(bottom > 235, "ends white, got {bottom}");
        assert!(
            (100..=155).contains(&middle),
            "and passes mid grey, got {middle}"
        );
    }

    #[test]
    fn a_gradient_angle_turns_the_way_css_turns() {
        // 90deg runs to the right, so the dark end is on the left.
        let painted = render(
            r##"
            <gui version="0.2">
              <frame w="40" h="40" fill="#ffffff">
                <rect abs x="0" y="0" w="40" h="40"
                      fill="linear-gradient(90deg, #000000 0%, #ffffff 100%)" />
              </frame>
            </gui>
            "##,
        );

        assert!(painted.get_pixel(1, 20).0[0] < 20, "dark at the left");
        assert!(painted.get_pixel(38, 20).0[0] > 235, "light at the right");
        // And nothing changes down the box.
        assert_eq!(painted.get_pixel(20, 2).0, painted.get_pixel(20, 37).0);
    }

    #[test]
    fn a_gradient_stops_carry_their_own_alpha() {
        // Transparent black over white stays white at the top and darkens down.
        let painted = render(
            r##"
            <gui version="0.2">
              <frame w="40" h="40" fill="#ffffff">
                <rect abs x="0" y="0" w="40" h="40"
                      fill="linear-gradient(180deg, #00000000 0%, #000000ff 100%)" />
              </frame>
            </gui>
            "##,
        );

        assert!(painted.get_pixel(20, 1).0[0] > 240, "clear at the top");
        assert!(painted.get_pixel(20, 38).0[0] < 20, "opaque at the bottom");
    }

    #[test]
    fn a_gradient_is_held_inside_the_nodes_corners() {
        let painted = render(
            r##"
            <gui version="0.2">
              <frame w="40" h="40" fill="#ffffff">
                <ellipse abs x="0" y="0" w="40" h="40"
                         fill="linear-gradient(180deg, #000000 0%, #000000 100%)" />
              </frame>
            </gui>
            "##,
        );

        assert!(painted.get_pixel(20, 20).0[0] < 20, "inside the ellipse");
        assert!(
            painted.get_pixel(1, 1).0[0] > 240,
            "the corner outside it is untouched"
        );
    }

    #[test]
    fn radial_and_conic_gradients_paint_something() {
        for value in [
            "radial-gradient(circle at 50% 50%, #000000 0%, #ffffff 100%)",
            "conic-gradient(from 0deg at 50% 50%, #000000 0%, #ffffff 100%)",
        ] {
            let painted = render(&format!(
                r##"
                <gui version="0.2">
                  <frame w="40" h="40" fill="#7f7f7f">
                    <rect abs x="0" y="0" w="40" h="40" fill="{value}" />
                  </frame>
                </gui>
                "##
            ));

            let shades: std::collections::BTreeSet<u8> = (0..40)
                .flat_map(|y| (0..40).map(move |x| (x, y)))
                .map(|(x, y)| painted.get_pixel(x, y).0[0])
                .collect();

            assert!(
                shades.len() > 8,
                "{value} should paint a spread of shades, got {}",
                shades.len()
            );
        }
    }

    #[test]
    fn an_image_fill_is_clipped_to_the_nodes_shape() {
        let mut image = image::RgbaImage::new(1, 1);
        image.put_pixel(0, 0, image::Rgba([255, 0, 0, 255]));
        let mut bytes = Vec::new();
        image
            .write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Png,
            )
            .expect("test image encodes");

        let painted = render_with_asset(
            r##"
            <gui version="0.2">
              <frame w="40" h="40" fill="#ffffff">
                <ellipse abs x="0" y="0" w="40" h="40">
                  <appearance>
                    <fill type="image" src="assets/red.png" fit="fill" />
                  </appearance>
                </ellipse>
              </frame>
            </gui>
            "##,
            "assets/red.png",
            bytes,
        );

        assert_eq!(
            painted.get_pixel(20, 20).0[0..3],
            [255, 0, 0],
            "the image fills the ellipse"
        );
        assert_eq!(
            painted.get_pixel(1, 1).0[0..3],
            [255, 255, 255],
            "and is cut off at its edge"
        );
    }

    #[test]
    fn opacity_reaches_a_containers_children() {
        // The plain bug: a container's `opacity` used to fade its own paint
        // and leave everything inside it fully opaque.
        let painted = render(
            r##"
            <gui version="0.2">
              <frame w="40" h="40" fill="#ffffff">
                <frame abs x="0" y="0" w="40" h="40" opacity="0.5">
                  <rect abs x="0" y="0" w="40" h="40" fill="#000000" />
                </frame>
              </frame>
            </gui>
            "##,
        );

        let grey = painted.get_pixel(20, 20).0[0];
        assert!(
            (125..=131).contains(&grey),
            "expected half black over white, got {grey}"
        );
    }

    #[test]
    fn overlapping_children_do_not_compound_a_groups_opacity() {
        // The whole group fades once. Fading each child instead would darken
        // wherever two of them overlap.
        let painted = render(
            r##"
            <gui version="0.2">
              <frame w="60" h="60" fill="#ffffff">
                <frame abs x="0" y="0" w="60" h="60" opacity="0.5">
                  <rect abs x="5" y="5" w="30" h="30" fill="#000000" />
                  <rect abs x="20" y="20" w="30" h="30" fill="#000000" />
                </frame>
              </frame>
            </gui>
            "##,
        );

        let single = painted.get_pixel(10, 10).0[0];
        let overlap = painted.get_pixel(25, 25).0[0];
        assert_eq!(
            single, overlap,
            "the overlap must be no darker than either child alone"
        );
    }

    #[test]
    fn a_leaf_nodes_opacity_still_fades_it() {
        let painted = render(
            r##"
            <gui version="0.2">
              <frame w="40" h="40" fill="#ffffff">
                <rect abs x="0" y="0" w="40" h="40" fill="#000000" opacity="0.25" />
              </frame>
            </gui>
            "##,
        );

        let grey = painted.get_pixel(20, 20).0[0];
        assert!(
            (188..=194).contains(&grey),
            "expected a quarter of black over white, got {grey}"
        );
    }

    #[test]
    fn nested_opacity_multiplies() {
        let painted = render(
            r##"
            <gui version="0.2">
              <frame w="40" h="40" fill="#ffffff">
                <frame abs x="0" y="0" w="40" h="40" opacity="0.5">
                  <frame abs x="0" y="0" w="40" h="40" opacity="0.5">
                    <rect abs x="0" y="0" w="40" h="40" fill="#000000" />
                  </frame>
                </frame>
              </frame>
            </gui>
            "##,
        );

        let grey = painted.get_pixel(20, 20).0[0];
        assert!(
            (188..=194).contains(&grey),
            "0.5 inside 0.5 is a quarter, got {grey}"
        );
    }

    #[test]
    fn zero_opacity_hides_a_whole_subtree() {
        let painted = render(
            r##"
            <gui version="0.2">
              <frame w="40" h="40" fill="#ffffff">
                <frame abs x="0" y="0" w="40" h="40" opacity="0" fill="#ff0000">
                  <rect abs x="0" y="0" w="40" h="40" fill="#000000" />
                </frame>
              </frame>
            </gui>
            "##,
        );

        assert_eq!(painted.get_pixel(20, 20).0[0..3], [255, 255, 255]);
    }

    #[test]
    fn a_groups_opacity_fades_its_shadow_with_it() {
        // The shadow is part of the subtree, so it fades by the same amount.
        // Per-draw opacity never reached it at all.
        let shadow_at = |opacity: &str| {
            let painted = render(&format!(
                r##"
                <gui version="0.2">
                  <frame w="60" h="60" fill="#ffffff">
                    <rect abs x="15" y="10" w="30" h="20" fill="#ffffff" {opacity}>
                      <appearance>
                        <effect type="drop-shadow" x="0" y="8" radius="4"
                                color="#000000ff" />
                      </appearance>
                    </rect>
                  </frame>
                </gui>
                "##
            ));
            painted.get_pixel(30, 36).0[0]
        };

        let solid = shadow_at("");
        let faded = shadow_at(r#"opacity="0.5""#);
        assert!(
            faded > solid + 20,
            "a faded group casts a fainter shadow: {faded} vs {solid}"
        );
    }

    #[test]
    fn layer_blur_softens_the_nodes_own_edge() {
        // A hard black square on white: blurred, its edge stops being a step
        // and the pixels either side of it move toward each other.
        let page = |effect: &str| {
            render(&format!(
                r##"
                <gui version="0.2">
                  <frame w="60" h="60" fill="#ffffff">
                    <rect abs x="15" y="15" w="30" h="30" fill="#000000">
                      <appearance>{effect}</appearance>
                    </rect>
                  </frame>
                </gui>
                "##
            ))
        };

        let sharp = page("");
        let blurred = page(r#"<effect type="layer-blur" radius="8" />"#);

        // Just inside the edge: solid before, lifted after.
        assert_eq!(sharp.get_pixel(30, 16).0[0], 0);
        assert!(
            blurred.get_pixel(30, 16).0[0] > 30,
            "the inside of the edge lightens"
        );

        // Just outside it: untouched before, darkened after.
        assert_eq!(sharp.get_pixel(30, 12).0[0], 255);
        assert!(
            blurred.get_pixel(30, 12).0[0] < 225,
            "and ink spreads past the edge"
        );
    }

    #[test]
    fn layer_blur_blurs_the_subtree_not_the_backdrop() {
        // The distinction that separates it from `background-blur`: what sits
        // behind the node is left alone.
        let painted = render(
            r##"
            <gui version="0.2">
              <frame w="80" h="40" fill="#ffffff">
                <rect abs x="0" y="0" w="30" h="40" fill="#ff0000" />
                <frame abs x="40" y="10" w="30" h="20" fill="#000000">
                  <appearance>
                    <effect type="layer-blur" radius="6" />
                  </appearance>
                </frame>
              </frame>
            </gui>
            "##,
        );

        assert_eq!(
            painted.get_pixel(10, 20).0[0..3],
            [255, 0, 0],
            "the sibling behind is not blurred"
        );
        assert!(
            painted.get_pixel(55, 8).0[0] < 250,
            "but the blurred node spreads past its own box"
        );
    }

    #[test]
    fn layer_blur_reaches_a_nodes_children() {
        let painted = render(
            r##"
            <gui version="0.2">
              <frame w="60" h="60" fill="#ffffff">
                <frame abs x="0" y="0" w="60" h="60">
                  <appearance>
                    <effect type="layer-blur" radius="8" />
                  </appearance>
                  <rect abs x="15" y="15" w="30" h="30" fill="#000000" />
                </frame>
              </frame>
            </gui>
            "##,
        );

        assert!(
            painted.get_pixel(30, 12).0[0] < 225,
            "the child is blurred with its parent"
        );
    }

    #[test]
    fn an_invisible_layer_blur_does_nothing() {
        let painted = render(
            r##"
            <gui version="0.2">
              <frame w="60" h="60" fill="#ffffff">
                <rect abs x="15" y="15" w="30" h="30" fill="#000000">
                  <appearance>
                    <effect type="layer-blur" radius="8" visible="false" />
                  </appearance>
                </rect>
              </frame>
            </gui>
            "##,
        );

        assert_eq!(painted.get_pixel(30, 16).0[0], 0, "the edge stays hard");
        assert_eq!(painted.get_pixel(30, 12).0[0], 255);
    }

    #[test]
    fn paints_clipping_bounds() {
        let document = parse_gui_xml(
            r##"
            <gui version="0.2">
              <col w="100" h="100" fill="#ffffff" clip>
                <rect x="150" y="150" w="50" h="50" fill="#ff0000" />
              </col>
            </gui>
            "##,
        )
        .expect("valid gui");
        let layout = compute_taffy_layout(&document).expect("layout computes");
        let scene = build_scene(&document, &layout);

        let path = std::env::temp_dir().join("dotgui-renderer-clip-test.png");
        paint_scene_to_png(&scene, &path).expect("png paints");

        let bytes = std::fs::read(&path).expect("png readable");
        let _ = std::fs::remove_file(&path);
        assert!(!bytes.is_empty());
    }

    #[test]
    fn translucent_raster_pixels_composite_against_the_backdrop() {
        // A single white pixel at 50% alpha over black must land near mid grey.
        // Handing tiny-skia straight-alpha bytes would paint it pure white.
        let mut translucent = Vec::new();
        image::RgbaImage::from_pixel(1, 1, image::Rgba([255, 255, 255, 128]))
            .write_to(
                &mut std::io::Cursor::new(&mut translucent),
                image::ImageFormat::Png,
            )
            .expect("test image encodes");

        let document = parse_gui_xml(
            r##"
            <gui version="0.2">
              <col w="4" h="4" fill="#000000">
                <img src="assets/translucent.png" w="4" h="4" fit="fill" />
              </col>
            </gui>
            "##,
        )
        .expect("valid gui");
        let layout = compute_taffy_layout(&document).expect("layout computes");
        let scene = build_scene(&document, &layout);

        let mut package_assets = std::collections::BTreeMap::new();
        package_assets.insert("assets/translucent.png".to_owned(), translucent);
        let cache = AssetCache::new(std::env::temp_dir()).with_package_assets(package_assets);

        let png = paint_scene_to_png_bytes(&scene, Some(&cache), None).expect("scene paints");
        let painted = image::load_from_memory(&png)
            .expect("painted png decodes")
            .to_rgba8();

        let pixel = painted.get_pixel(2, 2);
        assert!(
            (100..=155).contains(&pixel[0]),
            "expected mid grey, painted {pixel:?}"
        );
    }

    #[test]
    fn each_segment_paints_in_its_own_colour() {
        let document = parse_gui_xml(
            r##"
            <gui version="0.2">
              <col w="220" h="40" fill="#ffffff" p="4">
                <text font-size="20" fill="#000000">
                  <segment value="RRRR" fill="#ff0000" />
                  <segment value="BBBB" fill="#0000ff" />
                </text>
              </col>
            </gui>
            "##,
        )
        .expect("valid gui");
        let layout = compute_taffy_layout(&document).expect("layout computes");
        let scene = build_scene(&document, &layout);

        let png = paint_scene_to_png_bytes(&scene, None, None).expect("scene paints");
        let painted = image::load_from_memory(&png)
            .expect("painted png decodes")
            .to_rgba8();

        let mut has_red = false;
        let mut has_blue = false;
        for pixel in painted.pixels() {
            let [r, g, b, _] = pixel.0;
            has_red |= r > 120 && g < 90 && b < 90;
            has_blue |= b > 120 && r < 90 && g < 90;
        }

        assert!(has_red, "the first segment should paint red glyphs");
        assert!(has_blue, "the second segment should paint blue glyphs");
    }

    /// Renders a document and hands back the pixels.
    fn render(xml: &str) -> image::RgbaImage {
        let document = parse_gui_xml(xml).expect("valid gui");
        let layout = compute_taffy_layout(&document).expect("layout computes");
        let scene = build_scene(&document, &layout);
        let png = paint_scene_to_png_bytes(&scene, None, None).expect("scene paints");
        image::load_from_memory(&png)
            .expect("painted png decodes")
            .to_rgba8()
    }

    const SHADOW_CARD: &str = r##"
        <gui version="0.2">
          <col w="80" h="80" fill="#ffffff" p="20">
            <rect w="40" h="20" fill="#ffffff">
              <appearance>
                <effect type="drop-shadow" x="0" y="6" radius="8" color="#000000ff" EXTRA />
              </appearance>
            </rect>
          </col>
        </gui>
    "##;

    #[test]
    fn a_drop_shadow_darkens_the_canvas_below_its_node() {
        let painted = render(&SHADOW_CARD.replace("EXTRA", ""));

        // The card occupies y=20..40; the shadow is thrown 6px down.
        let below = painted.get_pixel(40, 48);
        assert!(
            below[0] < 240,
            "expected a shadow under the card, found {below:?}"
        );

        // Well away from the card the canvas stays white.
        let corner = painted.get_pixel(2, 2);
        assert!(corner[0] > 250, "the shadow should not reach the corner");
    }

    #[test]
    fn an_invisible_effect_is_not_drawn() {
        let painted = render(&SHADOW_CARD.replace("EXTRA", r#"visible="false""#));

        assert_eq!(
            painted.get_pixel(40, 48)[0],
            255,
            "visible=\"false\" should leave the canvas untouched"
        );
    }

    #[test]
    fn spread_widens_the_shadow() {
        let tight = render(&SHADOW_CARD.replace("EXTRA", ""));
        let wide = render(&SHADOW_CARD.replace("EXTRA", r#"spread="6""#));

        let darkness = |image: &image::RgbaImage| {
            image
                .pixels()
                .map(|pixel| 255u32 - u32::from(pixel[0]))
                .sum::<u32>()
        };

        assert!(
            darkness(&wide) > darkness(&tight),
            "a positive spread should cast more shadow"
        );
    }

    #[test]
    fn a_background_blur_softens_what_is_behind_it() {
        // A hard black/white edge behind a frosted panel must come out grey
        // where the panel covers it.
        let painted = render(
            r##"
            <gui version="0.2">
              <grid unit="10" w="100" h="60" fill="#ffffff">
                <rect gc="1/5" gr="1/6" fill="#000000" />
                <rect gc="1/10" gr="3/4" fill="#ffffff00">
                  <appearance>
                    <effect type="background-blur" radius="20" />
                  </appearance>
                </rect>
              </grid>
            </gui>
            "##,
        );

        // Just right of the black block, inside the panel: blurred, so grey.
        let inside = painted.get_pixel(52, 25)[0];
        // The same column above the panel keeps its hard white.
        let outside = painted.get_pixel(52, 5)[0];

        assert!(
            (10..245).contains(&inside),
            "expected a blurred edge inside the panel, found {inside}"
        );
        assert_eq!(outside, 255, "outside the panel the edge stays hard");
    }

    #[test]
    fn paints_raster_image_fit_modes() {
        const RED_PNG: &[u8] = &[
            137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1,
            8, 2, 0, 0, 0, 144, 119, 83, 222, 0, 0, 0, 12, 73, 68, 65, 84, 120, 156, 99, 248, 207,
            192, 0, 0, 3, 1, 0, 2, 175, 172, 150, 14, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
        ];

        let document = parse_gui_xml(
            r##"
            <gui version="0.2">
              <col w="100" h="100">
                <img src="assets/red.png" w="100" h="100" fit="cover" />
              </col>
            </gui>
            "##,
        )
        .expect("valid gui");
        let layout = compute_taffy_layout(&document).expect("layout computes");
        let scene = build_scene(&document, &layout);

        let mut package_assets = std::collections::BTreeMap::new();
        package_assets.insert("assets/red.png".to_owned(), RED_PNG.to_vec());
        let cache = AssetCache::new(std::env::temp_dir()).with_package_assets(package_assets);

        let path = std::env::temp_dir().join("dotgui-renderer-raster-test.png");
        paint_scene_to_png_with_assets(&scene, &path, &cache).expect("png paints");

        let bytes = std::fs::read(&path).expect("png readable");
        let _ = std::fs::remove_file(&path);
        assert!(!bytes.is_empty());
    }

    #[test]
    fn paints_stroke_alignments() {
        let document = parse_gui_xml(
            r##"
            <gui version="0.2">
              <col w="100" h="100" fill="#ffffff" border="2 #000000 solid outside">
                <rect w="50" h="50" border="2 #ff0000 solid center" />
              </col>
            </gui>
            "##,
        )
        .expect("valid gui");
        let layout = compute_taffy_layout(&document).expect("layout computes");
        let scene = build_scene(&document, &layout);

        let path = std::env::temp_dir().join("dotgui-renderer-stroke-test.png");
        paint_scene_to_png(&scene, &path).expect("png paints");

        let bytes = std::fs::read(&path).expect("png readable");
        let _ = std::fs::remove_file(&path);
        assert!(!bytes.is_empty());
    }
}
