use crate::{
    AssetCache, Border, BorderWidths, FontFace, FontStore, PaintContent, Scene, SceneNode,
};
use fontdue::{Font, FontSettings};
use std::{fs, path::Path};
use thiserror::Error;
use tiny_skia::{
    Color, FillRule, Paint, PathBuilder, Pixmap, PixmapPaint, Rect, Stroke, Transform,
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

fn paint_scene(
    scene: &Scene,
    path: impl AsRef<Path>,
    asset_cache: Option<&AssetCache>,
    fonts: Option<&FontStore>,
) -> Result<(), PaintError> {
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
        .save_png(path)
        .map_err(|err| PaintError::Png(err.to_string()))?;
    Ok(())
}

fn paint_node(
    pixmap: &mut Pixmap,
    node: &SceneNode,
    font: Option<&Font>,
    asset_cache: Option<&AssetCache>,
    fonts: Option<&FontStore>,
) {
    paint_fill(pixmap, node);
    paint_content(pixmap, node, font, asset_cache, fonts);

    for child in &node.children {
        paint_node(pixmap, child, font, asset_cache, fonts);
    }

    paint_border(pixmap, node);
}

fn paint_fill(pixmap: &mut Pixmap, node: &SceneNode) {
    if matches!(node.content, PaintContent::Text { .. }) {
        return;
    }

    let Some(fill) = node.fill.as_deref() else {
        return;
    };
    let Some(color) = parse_color(fill, node.opacity) else {
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

    fill_rounded_rect(
        pixmap,
        node.bounds.x,
        node.bounds.y,
        node.bounds.width,
        paint_height(node),
        node.radius.unwrap_or(0.0),
        color,
    );
}

fn paint_height(node: &SceneNode) -> f32 {
    if node.tag == "line" && node.bounds.height <= 0.0 {
        1.0
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
            value,
            font_family,
            font_weight,
            font_style,
            font_size,
            line_height,
            can_wrap,
            max_lines,
            truncate,
            text_align,
            ..
        } => {
            if let Some(face) = fonts.and_then(|fonts| {
                fonts.get(
                    font_family.as_deref(),
                    font_weight.as_deref(),
                    font_style.as_deref(),
                )
            }) {
                paint_text_face(
                    pixmap,
                    node,
                    face,
                    value,
                    *font_size,
                    *line_height,
                    *can_wrap,
                    *max_lines,
                    *truncate,
                    text_align.as_deref(),
                );
            } else if let Some(font) = font {
                paint_text_fontdue(
                    pixmap,
                    node,
                    font,
                    value,
                    *font_size,
                    *line_height,
                    *can_wrap,
                    *max_lines,
                    *truncate,
                    text_align.as_deref(),
                );
            } else {
                paint_text_placeholder(pixmap, node);
            }
        }
        PaintContent::Image { src } => paint_image(pixmap, node, src, asset_cache),
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

fn paint_text_face(
    pixmap: &mut Pixmap,
    node: &SceneNode,
    face: &FontFace,
    value: &str,
    font_size: f32,
    line_height: f32,
    can_wrap: bool,
    max_lines: Option<usize>,
    truncate: bool,
    text_align: Option<&str>,
) {
    if paint_text_variable(
        pixmap,
        node,
        face,
        value,
        font_size,
        line_height,
        can_wrap,
        max_lines,
        truncate,
        text_align,
    )
    .is_none()
    {
        paint_text_fontdue(
            pixmap,
            node,
            face.fallback(),
            value,
            font_size,
            line_height,
            can_wrap,
            max_lines,
            truncate,
            text_align,
        );
    }
}

fn paint_text_variable(
    pixmap: &mut Pixmap,
    node: &SceneNode,
    face: &FontFace,
    value: &str,
    font_size: f32,
    line_height: f32,
    can_wrap: bool,
    max_lines: Option<usize>,
    truncate: bool,
    text_align: Option<&str>,
) -> Option<()> {
    let color = node
        .fill
        .as_deref()
        .and_then(|fill| parse_color(fill, node.opacity))?;
    let ttf_face = face.variable_face()?;
    let scale = font_size / ttf_face.units_per_em() as f32;
    let mut paint = Paint::default();
    paint.set_color(color);
    paint.anti_alias = true;

    let baseline_offset = face.baseline_offset(font_size, line_height);
    let mut baseline = node.bounds.y + baseline_offset;
    let max_y = node.bounds.y + node.bounds.height;
    let mut lines = if can_wrap && !truncate {
        wrap_text_face(face, value, font_size, node.bounds.width)
    } else {
        vec![value.to_owned()]
    };
    apply_line_limit_and_ellipsis(&mut lines, max_lines, truncate, node.bounds.width, |text| {
        face.text_width(text, font_size)
    });

    for line in lines {
        if baseline - font_size > max_y {
            break;
        }

        let mut cursor_x = aligned_text_x(
            node.bounds.x,
            node.bounds.width,
            face.text_width(&line, font_size),
            text_align,
        );
        for ch in line.chars() {
            let Some(glyph) = ttf_face.glyph_index(ch) else {
                cursor_x += face.fallback().metrics(ch, font_size).advance_width;
                continue;
            };

            let mut builder = GlyphPathBuilder::new(cursor_x, baseline, scale);
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

            cursor_x += ttf_face
                .glyph_hor_advance(glyph)
                .map(|advance| advance as f32 * scale)
                .unwrap_or_else(|| face.fallback().metrics(ch, font_size).advance_width);
        }
        baseline += line_height;
    }

    Some(())
}

fn paint_text_fontdue(
    pixmap: &mut Pixmap,
    node: &SceneNode,
    font: &Font,
    value: &str,
    font_size: f32,
    line_height: f32,
    can_wrap: bool,
    max_lines: Option<usize>,
    truncate: bool,
    text_align: Option<&str>,
) {
    let Some(color) = node
        .fill
        .as_deref()
        .and_then(|fill| parse_color(fill, node.opacity))
    else {
        return;
    };

    let mut baseline = node.bounds.y + fontdue_baseline_offset(font, font_size, line_height);
    let max_y = node.bounds.y + node.bounds.height;

    let mut lines = if can_wrap && !truncate {
        wrap_text(font, value, font_size, node.bounds.width)
    } else {
        vec![value.to_owned()]
    };
    apply_line_limit_and_ellipsis(&mut lines, max_lines, truncate, node.bounds.width, |text| {
        text_width(font, text, font_size)
    });

    for line in lines {
        if baseline - font_size > max_y {
            break;
        }
        let mut cursor_x = aligned_text_x(
            node.bounds.x,
            node.bounds.width,
            text_width(font, &line, font_size),
            text_align,
        );
        for ch in line.chars() {
            let (metrics, bitmap) = font.rasterize(ch, font_size);
            let glyph_x = cursor_x + metrics.xmin as f32;
            let glyph_y = baseline - metrics.ymin as f32 - metrics.height as f32;

            draw_glyph_bitmap(
                pixmap,
                glyph_x,
                glyph_y,
                metrics.width,
                metrics.height,
                &bitmap,
                color,
            );
            cursor_x += metrics.advance_width;
        }
        baseline += line_height;
    }
}

fn wrap_text_face(face: &FontFace, value: &str, font_size: f32, max_width: f32) -> Vec<String> {
    let max_width = max_width.max(1.0);
    let mut lines = Vec::new();

    for source_line in value.lines() {
        let mut current = String::new();
        let mut current_width = 0.0_f32;

        for word in source_line.split_whitespace() {
            let word_width = face.text_width(word, font_size);
            let space_width = if current.is_empty() {
                0.0
            } else {
                face.text_width(" ", font_size)
            };

            if !current.is_empty() && current_width + space_width + word_width > max_width {
                lines.push(current);
                current = word.to_owned();
                current_width = word_width;
            } else {
                if !current.is_empty() {
                    current.push(' ');
                    current_width += space_width;
                }
                current.push_str(word);
                current_width += word_width;
            }
        }

        if current.is_empty() {
            lines.push(String::new());
        } else {
            lines.push(current);
        }
    }

    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

fn fontdue_baseline_offset(font: &Font, font_size: f32, line_height: f32) -> f32 {
    font.horizontal_line_metrics(font_size)
        .map(|metrics| ((line_height - font_size) / 2.0) + metrics.ascent)
        .unwrap_or(font_size)
}

fn apply_line_limit_and_ellipsis(
    lines: &mut Vec<String>,
    max_lines: Option<usize>,
    truncate: bool,
    max_width: f32,
    measure: impl Fn(&str) -> f32,
) {
    let max_lines = max_lines.unwrap_or(usize::MAX);
    if lines.len() > max_lines {
        lines.truncate(max_lines);
    }

    if truncate && max_lines == 1 {
        let source = lines.first().cloned().unwrap_or_default();
        *lines = vec![ellipsize(&source, max_width, measure)];
    }
}

fn ellipsize(value: &str, max_width: f32, measure: impl Fn(&str) -> f32) -> String {
    if measure(value) <= max_width {
        return value.to_owned();
    }

    let ellipsis = "...";
    if measure(ellipsis) > max_width {
        return String::new();
    }

    let mut fitted = String::new();
    for ch in value.chars() {
        fitted.push(ch);
        let candidate = format!("{fitted}{ellipsis}");
        if measure(&candidate) > max_width {
            fitted.pop();
            break;
        }
    }

    format!("{fitted}{ellipsis}")
}

fn aligned_text_x(x: f32, width: f32, text_width: f32, align: Option<&str>) -> f32 {
    match align {
        Some("right") => x + (width - text_width).max(0.0),
        Some("center") | Some("middle") => x + ((width - text_width).max(0.0) / 2.0),
        _ => x,
    }
}

fn wrap_text(font: &Font, value: &str, font_size: f32, max_width: f32) -> Vec<String> {
    let max_width = max_width.max(1.0);
    let mut lines = Vec::new();

    for source_line in value.lines() {
        let mut current = String::new();
        let mut current_width = 0.0_f32;

        for word in source_line.split_whitespace() {
            let word_width = text_width(font, word, font_size);
            let space_width = if current.is_empty() {
                0.0
            } else {
                text_width(font, " ", font_size)
            };

            if !current.is_empty() && current_width + space_width + word_width > max_width {
                lines.push(current);
                current = word.to_owned();
                current_width = word_width;
            } else {
                if !current.is_empty() {
                    current.push(' ');
                    current_width += space_width;
                }
                current.push_str(word);
                current_width += word_width;
            }
        }

        if current.is_empty() {
            lines.push(String::new());
        } else {
            lines.push(current);
        }
    }

    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
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

fn draw_glyph_bitmap(
    pixmap: &mut Pixmap,
    x: f32,
    y: f32,
    width: usize,
    height: usize,
    bitmap: &[u8],
    color: Color,
) {
    let Some((r, g, b, base_a)) = color_to_rgba8(color) else {
        return;
    };

    for row in 0..height {
        for col in 0..width {
            let coverage = bitmap[row * width + col];
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
        .fill
        .as_deref()
        .and_then(|fill| parse_color(fill, node.opacity))
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

fn paint_image(pixmap: &mut Pixmap, node: &SceneNode, src: &str, asset_cache: Option<&AssetCache>) {
    if let Some(cache) = asset_cache {
        if let Ok(asset) = cache.resolve(src) {
            if asset.media_type.as_deref() == Some("image/svg+xml") || looks_like_svg(&asset.bytes)
            {
                if render_svg_asset(pixmap, node, &asset.bytes).is_ok() {
                    return;
                }
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

fn paint_border(pixmap: &mut Pixmap, node: &SceneNode) {
    let Some(border) = &node.border else {
        return;
    };
    if border.widths.is_uniform() {
        stroke_rounded_rect(pixmap, node, border);
    } else {
        stroke_sided_border(pixmap, node, border);
    }
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

    if let Some(path) = rounded_rect_path(x, y, width, height, radius) {
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
    let Some(color) = parse_color(&border.color, node.opacity) else {
        return;
    };

    let mut paint = Paint::default();
    paint.set_color(color);
    paint.anti_alias = true;
    let stroke = Stroke {
        width: border.width,
        ..Default::default()
    };

    if node.tag == "ellipse" {
        if let Some(path) = ellipse_path(
            node.bounds.x + border.width / 2.0,
            node.bounds.y + border.width / 2.0,
            (node.bounds.width - border.width).max(0.0),
            (node.bounds.height - border.width).max(0.0),
        ) {
            pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
        }
        return;
    }

    if let Some(path) = rounded_rect_path(
        node.bounds.x + border.width / 2.0,
        node.bounds.y + border.width / 2.0,
        (node.bounds.width - border.width).max(0.0),
        (node.bounds.height - border.width).max(0.0),
        node.radius.unwrap_or(0.0),
    ) {
        pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
    }
}

fn stroke_sided_border(pixmap: &mut Pixmap, node: &SceneNode, border: &Border) {
    let Some(color) = parse_color(&border.color, node.opacity) else {
        return;
    };
    let x = node.bounds.x;
    let y = node.bounds.y;
    let w = node.bounds.width;
    let h = node.bounds.height;

    if border.widths.top > 0.0 {
        stroke_line(
            pixmap,
            x,
            y + border.widths.top / 2.0,
            x + w,
            y + border.widths.top / 2.0,
            border.widths.top,
            color,
        );
    }
    if border.widths.right > 0.0 {
        stroke_line(
            pixmap,
            x + w - border.widths.right / 2.0,
            y,
            x + w - border.widths.right / 2.0,
            y + h,
            border.widths.right,
            color,
        );
    }
    if border.widths.bottom > 0.0 {
        stroke_line(
            pixmap,
            x,
            y + h - border.widths.bottom / 2.0,
            x + w,
            y + h - border.widths.bottom / 2.0,
            border.widths.bottom,
            color,
        );
    }
    if border.widths.left > 0.0 {
        stroke_line(
            pixmap,
            x + border.widths.left / 2.0,
            y,
            x + border.widths.left / 2.0,
            y + h,
            border.widths.left,
            color,
        );
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

fn rounded_rect_path(
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    radius: f32,
) -> Option<tiny_skia::Path> {
    if width <= 0.0 || height <= 0.0 {
        return None;
    }

    let r = radius.max(0.0).min(width / 2.0).min(height / 2.0);
    let mut pb = PathBuilder::new();
    if r == 0.0 {
        pb.move_to(x, y);
        pb.line_to(x + width, y);
        pb.line_to(x + width, y + height);
        pb.line_to(x, y + height);
        pb.close();
        return pb.finish();
    }

    let c = 0.552_284_8 * r;
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

fn ellipse_path(x: f32, y: f32, width: f32, height: f32) -> Option<tiny_skia::Path> {
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
}
