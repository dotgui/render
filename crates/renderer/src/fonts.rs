use crate::{AssetCache, AssetError, FontInfo, GuiDocument, TextMeasurer};
use fontdue::{Font, FontSettings};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, path::PathBuf, rc::Rc};
use thiserror::Error;
use ttf_parser::{Face, Tag};

#[derive(Debug, Error)]
pub enum FontError {
    #[error("asset error: {0}")]
    Asset(#[from] AssetError),

    #[error("failed to load font {family}: {message}")]
    Load { family: String, message: String },

    #[error("failed to read Google Fonts metadata for {family}: {message}")]
    GoogleMetadata { family: String, message: String },
}

#[derive(Default)]
pub struct FontStore {
    fonts: BTreeMap<FontFaceKey, Rc<FontFace>>,
    warnings: Vec<String>,
}

pub struct FontFace {
    fallback: Font,
    bytes: Rc<Vec<u8>>,
    weight: f32,
    collection_index: u32,
}

/// The variable-font axes a run of text asks for, beyond its weight.
///
/// A face that has no such axis ignores the setting, so these are safe to pass
/// for any font.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct FontAxes {
    /// `wdth`, a percentage where 100 is normal, from `font-stretch`.
    pub width: Option<f32>,
    /// `opsz`, which CSS drives from the font size unless
    /// `font-optical-sizing="none"` turns it off.
    pub optical_size: Option<f32>,
}

impl FontAxes {
    /// The axes a resolved text style asks for.
    ///
    /// `font-optical-sizing` is honoured when a document asks for it, and only
    /// then. CSS defaults it to `auto`, so a browser — and therefore kit —
    /// applies it to every face with an `opsz` axis; matching that here would
    /// change the metrics of existing documents and reflow their text, which
    /// is a deliberate fidelity change rather than a side effect of reading a
    /// new attribute. It is tracked separately.
    pub fn from_style(
        font_stretch: Option<&str>,
        font_optical_sizing: Option<&str>,
        font_size: f32,
    ) -> Self {
        Self {
            width: font_stretch.and_then(font_stretch_percentage),
            optical_size: (font_optical_sizing == Some("auto")).then_some(font_size),
        }
    }
}

/// `font-stretch` as a `wdth` percentage.
///
/// CSS defines the keywords as exact percentages, which is what the `wdth`
/// axis is measured in, so a keyword and its percentage are the same request.
fn font_stretch_percentage(value: &str) -> Option<f32> {
    let value = value.trim();
    match value {
        "ultra-condensed" => Some(50.0),
        "extra-condensed" => Some(62.5),
        "condensed" => Some(75.0),
        "semi-condensed" => Some(87.5),
        "normal" => Some(100.0),
        "semi-expanded" => Some(112.5),
        "expanded" => Some(125.0),
        "extra-expanded" => Some(150.0),
        "ultra-expanded" => Some(200.0),
        _ => value
            .strip_suffix('%')
            .unwrap_or(value)
            .trim()
            .parse::<f32>()
            .ok()
            .filter(|percentage| *percentage > 0.0),
    }
}

impl FontFace {
    fn new(bytes: Vec<u8>, weight: &str, collection_index: u32) -> Result<Self, String> {
        let fallback = Font::from_bytes(
            bytes.clone(),
            FontSettings {
                collection_index,
                ..FontSettings::default()
            },
        )
        .map_err(|err| err.to_string())?;
        Ok(Self {
            fallback,
            bytes: Rc::new(bytes),
            weight: normalize_weight(weight).parse().unwrap_or(400.0),
            collection_index,
        })
    }

    pub fn fallback(&self) -> &Font {
        &self.fallback
    }

    pub fn text_width(&self, value: &str, font_size: f32, axes: FontAxes) -> f32 {
        if let Some(width) = self.variable_text_width(value, font_size, axes) {
            return width;
        }

        value
            .chars()
            .map(|ch| self.fallback.metrics(ch, font_size).advance_width)
            .sum()
    }

    /// Height of a capital above the baseline, at this size.
    ///
    /// Falls back to a fraction of the font size when the face declares no
    /// `capHeight`, which is about where a Latin capital lands.
    pub fn cap_height(&self, font_size: f32) -> f32 {
        Face::parse(&self.bytes, self.collection_index)
            .ok()
            .and_then(|face| {
                let units = face.units_per_em() as f32;
                face.capital_height()
                    .map(|cap| cap as f32 / units * font_size)
            })
            .unwrap_or(font_size * crate::layout::CAP_HEIGHT_RATIO)
    }

    /// How far the line box's top edge sits above the cap height.
    ///
    /// `leading-trim` removes exactly this, which puts the top of a capital on
    /// the box's top edge. Both layout and painting call it.
    pub fn leading_trim(&self, font_size: f32, line_height: f32) -> f32 {
        (self.baseline_offset(font_size, line_height, FontAxes::default())
            - self.cap_height(font_size))
        .max(0.0)
    }

    pub fn baseline_offset(&self, font_size: f32, line_height: f32, axes: FontAxes) -> f32 {
        if let Some(face) = self.variable_face(axes) {
            let scale = font_size / face.units_per_em() as f32;
            let ascender = face.ascender() as f32 * scale;
            let descender = face.descender() as f32 * scale;
            let content_height = ascender - descender;
            return ((line_height - content_height) / 2.0) + ascender;
        }

        self.fallback
            .horizontal_line_metrics(font_size)
            .map(|metrics| {
                let content_height = metrics.ascent - metrics.descent;
                ((line_height - content_height) / 2.0) + metrics.ascent
            })
            .unwrap_or(font_size)
    }

    /// Short content hash of the loaded face.
    ///
    /// Two hosts that resolve the same declared family can still end up with
    /// different files — a newer macOS, a re-fetched Google face. Comparing
    /// fingerprints turns "the pixels changed" into "the font changed".
    pub fn fingerprint(&self) -> String {
        let mut hash = Sha256::new();
        hash.update(self.bytes.as_slice());
        hash.update(self.collection_index.to_le_bytes());
        hash.finalize()
            .iter()
            .take(8)
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }

    pub fn variable_face(&self, axes: FontAxes) -> Option<Face<'_>> {
        let mut face = Face::parse(&self.bytes, self.collection_index).ok()?;
        let _ = face.set_variation(Tag::from_bytes(b"wght"), self.weight);
        if let Some(width) = axes.width {
            let _ = face.set_variation(Tag::from_bytes(b"wdth"), width);
        }
        if let Some(optical_size) = axes.optical_size {
            let _ = face.set_variation(Tag::from_bytes(b"opsz"), optical_size);
        }
        Some(face)
    }

    fn variable_text_width(&self, value: &str, font_size: f32, axes: FontAxes) -> Option<f32> {
        let face = self.variable_face(axes)?;
        let scale = font_size / face.units_per_em() as f32;
        Some(
            value
                .chars()
                .map(|ch| {
                    face.glyph_index(ch)
                        .and_then(|glyph| face.glyph_hor_advance(glyph))
                        .map(|advance| advance as f32 * scale)
                        .unwrap_or_else(|| self.fallback.metrics(ch, font_size).advance_width)
                })
                .sum(),
        )
    }
}

impl FontStore {
    pub fn from_document(document: &GuiDocument, cache: &AssetCache) -> Result<Self, FontError> {
        let mut store = Self::default();
        let mut loaded_fonts = BTreeMap::<(String, String), Rc<FontFace>>::new();
        // Scanning the host's font directories is expensive, so it happens once
        // per family instead of once per declared weight/style combination.
        let mut system_faces = BTreeMap::<String, Vec<SystemFace>>::new();

        for (family, info) in &document.metadata.fonts {
            let weights = declared_weights(info);
            let styles = declared_styles(info);

            // One request covers the whole family, so this is outside the loop.
            let google_faces = if info.source == "google" {
                google_font_faces(family, &weights, &styles, cache)?
            } else {
                BTreeMap::new()
            };

            for weight in &weights {
                for style in &styles {
                    let weight_key = normalize_weight(weight);
                    let style_key = normalize_style(style);

                    let source = match info.source.as_str() {
                        "google" => nearest_face(&google_faces, &weight_key, &style_key)
                            .cloned()
                            .map(FontSource::Google),
                        "system" => {
                            let resolved = resolve_system_family(family, &mut system_faces);
                            if let Some((matched_family, face)) = resolved {
                                if matched_family != *family {
                                    store.warnings.push(format!(
                                        "'{family}' is not installed; rendered with '{matched_family}'"
                                    ));
                                }
                                choose_system_face(&face, weight, style)
                                    .cloned()
                                    .map(FontSource::System)
                            } else {
                                None
                            }
                        }
                        _ => None,
                    };

                    let Some(source) = source else {
                        store.warnings.push(format!(
                            "{} font '{family}' (weight {weight}, style {style}) could not be resolved",
                            info.source
                        ));
                        continue;
                    };

                    // Weight is part of the key because variable faces are
                    // instanced per weight from the same bytes.
                    let loaded_key = (source.cache_key(), weight_key.clone());
                    let font = if let Some(font) = loaded_fonts.get(&loaded_key) {
                        Rc::clone(font)
                    } else {
                        let (bytes, collection_index) = source.load(cache)?;
                        let font =
                            Rc::new(FontFace::new(bytes, weight, collection_index).map_err(
                                |message| FontError::Load {
                                    family: family.clone(),
                                    message,
                                },
                            )?);
                        loaded_fonts.insert(loaded_key, Rc::clone(&font));
                        font
                    };

                    store
                        .fonts
                        .insert(FontFaceKey::new(family, weight, style), font);
                }
            }
        }

        Ok(store)
    }

    /// What the store could not resolve, and what it used instead.
    ///
    /// A renderer running where a declared font is unavailable — a Linux server
    /// asked for `SF Pro Display`, say — still produces output, and the caller
    /// needs to be able to say so rather than silently shipping the wrong
    /// typeface.
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    pub fn get(
        &self,
        family: Option<&str>,
        weight: Option<&str>,
        style: Option<&str>,
    ) -> Option<&FontFace> {
        let family = family?;
        let weight = weight.unwrap_or("400");
        let style = style.unwrap_or("normal");
        self.fonts
            .get(&FontFaceKey::new(family, weight, style))
            .map(Rc::as_ref)
    }

    pub fn is_empty(&self) -> bool {
        self.fonts.is_empty()
    }

    /// Fingerprints of every loaded face, keyed by `family/weight/style`.
    ///
    /// Lets a caller record which font files a render was produced with.
    pub fn fingerprints(&self) -> BTreeMap<String, String> {
        self.fonts
            .iter()
            .map(|(key, face)| {
                (
                    format!("{}/{}/{}", key.family, key.weight, key.style),
                    face.fingerprint(),
                )
            })
            .collect()
    }
}

impl TextMeasurer for FontStore {
    fn text_width(
        &self,
        value: &str,
        font_family: Option<&str>,
        font_weight: Option<&str>,
        font_style: Option<&str>,
        font_size: f32,
        axes: FontAxes,
    ) -> f32 {
        if let Some(face) = self.get(font_family, font_weight, font_style) {
            return face.text_width(value, font_size, axes);
        }

        fallback_text_width(value, font_size)
    }

    fn leading_trim(
        &self,
        font_family: Option<&str>,
        font_weight: Option<&str>,
        font_style: Option<&str>,
        font_size: f32,
        line_height: f32,
    ) -> f32 {
        self.get(font_family, font_weight, font_style)
            .map_or(0.0, |face| face.leading_trim(font_size, line_height))
    }
}

/// Resolves every declared face of a Google family in one request.
///
/// This is the source the HTML renderer uses: the Google Fonts CSS API, which
/// answers with one `@font-face` per requested weight and style. Walking the
/// `google/fonts` GitHub repository instead costs several API calls per family
/// and is rate limited to 60 an hour for anonymous callers — enough to fail a
/// CI run partway through.
///
/// Callers without `woff2` support, which this renderer is, are served
/// `format('truetype')` URLs, so the referenced files load directly.
fn google_font_faces(
    family: &str,
    weights: &[String],
    styles: &[String],
    cache: &AssetCache,
) -> Result<BTreeMap<(String, String), String>, FontError> {
    let Some(url) = google_css_url(family, weights, styles) else {
        return Ok(BTreeMap::new());
    };

    let css = match cache.resolve(&url) {
        Ok(asset) => asset.bytes,
        // A family Google does not publish answers 400. That is a missing
        // font, not a broken renderer, so the caller reports and carries on.
        Err(AssetError::Fetch { .. }) => return Ok(BTreeMap::new()),
        Err(err) => return Err(FontError::Asset(err)),
    };

    let css = String::from_utf8(css).map_err(|err| FontError::GoogleMetadata {
        family: family.to_owned(),
        message: err.to_string(),
    })?;

    Ok(parse_font_face_css(&css))
}

/// Builds a CSS API request covering every declared weight and style at once.
///
/// `family=Roboto:ital,wght@0,400;0,700;1,400` — the axis list has to be
/// sorted or the API rejects it.
fn google_css_url(family: &str, weights: &[String], styles: &[String]) -> Option<String> {
    let family_param = family.trim().replace(' ', "+");
    if family_param.is_empty() {
        return None;
    }

    let mut axes = Vec::new();
    for style in styles {
        let italic = i32::from(normalize_style(style) == "italic");
        for weight in weights {
            axes.push((
                italic,
                normalize_weight(weight).parse::<u16>().unwrap_or(400),
            ));
        }
    }
    axes.sort_unstable();
    axes.dedup();

    let spec = axes
        .iter()
        .map(|(italic, weight)| format!("{italic},{weight}"))
        .collect::<Vec<_>>()
        .join(";");

    Some(format!(
        "https://fonts.googleapis.com/css2?family={family_param}:ital,wght@{spec}"
    ))
}

/// Pulls `(weight, style) -> url` out of the `@font-face` blocks of a
/// stylesheet.
fn parse_font_face_css(css: &str) -> BTreeMap<(String, String), String> {
    let mut faces = BTreeMap::new();

    for block in css.split("@font-face").skip(1) {
        let Some(block) = block.split('}').next() else {
            continue;
        };

        let mut style = "normal".to_owned();
        let mut weight = "400".to_owned();
        let mut url = None;

        for declaration in block.split(';') {
            let Some((property, value)) = declaration.split_once(':') else {
                continue;
            };
            match property.trim() {
                "font-style" => style = normalize_style(value.trim()),
                // A variable face answers with a range; its first value is the
                // lightest weight it can be instanced at.
                "font-weight" => {
                    weight = value.split_whitespace().next().unwrap_or("400").to_owned()
                }
                "src" => {
                    url = value
                        .split_once("url(")
                        .and_then(|(_, rest)| rest.split_once(')'))
                        .map(|(link, _)| link.trim().trim_matches('"').to_owned())
                }
                _ => {}
            }
        }

        if let Some(url) = url {
            faces.insert((normalize_weight(&weight), style), url);
        }
    }

    faces
}

/// Picks the closest published face when the exact weight is not available.
fn nearest_face<'a>(
    faces: &'a BTreeMap<(String, String), String>,
    weight: &str,
    style: &str,
) -> Option<&'a String> {
    if let Some(url) = faces.get(&(weight.to_owned(), style.to_owned())) {
        return Some(url);
    }

    let target = weight.parse::<i32>().unwrap_or(400);
    faces
        .iter()
        .filter(|((_, face_style), _)| face_style == style)
        .min_by_key(|((face_weight, _), _)| {
            (face_weight.parse::<i32>().unwrap_or(400) - target).abs()
        })
        .map(|(_, url)| url)
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct FontFaceKey {
    family: String,
    weight: String,
    style: String,
}

impl FontFaceKey {
    fn new(family: &str, weight: &str, style: &str) -> Self {
        Self {
            family: family.trim().to_ascii_lowercase(),
            weight: normalize_weight(weight),
            style: normalize_style(style),
        }
    }
}

fn declared_weights(info: &FontInfo) -> Vec<String> {
    info.weights
        .as_deref()
        .unwrap_or("400")
        .split_whitespace()
        .map(normalize_weight)
        .collect()
}

fn declared_styles(info: &FontInfo) -> Vec<String> {
    info.styles
        .as_deref()
        .unwrap_or("normal")
        .split_whitespace()
        .map(normalize_style)
        .collect()
}

fn normalize_weight(weight: &str) -> String {
    weight.trim().parse::<u16>().unwrap_or(400).to_string()
}

fn normalize_style(style: &str) -> String {
    match style.trim() {
        "italic" => "italic".to_owned(),
        _ => "normal".to_owned(),
    }
}

fn fallback_text_width(value: &str, font_size: f32) -> f32 {
    value.chars().count() as f32 * font_size * 0.55
}

/// Where a declared font face's bytes come from.
enum FontSource {
    Google(String),
    System(SystemFace),
}

impl FontSource {
    /// Identity used to avoid loading the same file twice.
    fn cache_key(&self) -> String {
        match self {
            FontSource::Google(url) => url.clone(),
            FontSource::System(face) => format!("system://{}#{}", face.path.display(), face.index),
        }
    }

    fn load(&self, cache: &AssetCache) -> Result<(Vec<u8>, u32), FontError> {
        match self {
            FontSource::Google(url) => Ok((cache.resolve(url)?.bytes, 0)),
            FontSource::System(face) => {
                let bytes = std::fs::read(&face.path).map_err(|err| FontError::Load {
                    family: face.path.display().to_string(),
                    message: err.to_string(),
                })?;
                Ok((bytes, face.index))
            }
        }
    }
}

/// A font face found on the host, identified without holding its bytes.
#[derive(Debug, Clone)]
struct SystemFace {
    path: PathBuf,
    /// Index within a `.ttc` collection; `0` for single-face files.
    index: u32,
    weight: u16,
    italic: bool,
    /// Variable faces carry a `wght` axis, so they can serve any weight.
    variable: bool,
}

/// Families to try when a declared `source="system"` family is not installed.
///
/// The HTML renderer emits the CSS stack
/// `system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif`, so
/// the browser lands on the host's UI font. Nothing resolves those generic
/// names off a filesystem, so the equivalent concrete families are tried here.
const SYSTEM_UI_FALLBACKS: &[&str] = &[
    "SF Pro",      // macOS
    "Segoe UI",    // Windows
    "DejaVu Sans", // most Linux distributions
    "Liberation Sans",
    "Arial",
];

/// Finds the declared family, or the nearest platform UI font.
///
/// Returns which family actually matched so the caller can report a
/// substitution.
fn resolve_system_family(
    family: &str,
    cache: &mut BTreeMap<String, Vec<SystemFace>>,
) -> Option<(String, Vec<SystemFace>)> {
    for candidate in std::iter::once(family).chain(SYSTEM_UI_FALLBACKS.iter().copied()) {
        let faces = cache
            .entry(candidate.to_owned())
            .or_insert_with(|| system_font_candidates(candidate));
        if !faces.is_empty() {
            return Some((candidate.to_owned(), faces.clone()));
        }
    }
    None
}

fn system_font_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = if cfg!(target_os = "macos") {
        [
            "/System/Library/Fonts",
            "/System/Library/Fonts/Supplemental",
            "/Library/Fonts",
        ]
        .iter()
        .map(PathBuf::from)
        .collect()
    } else if cfg!(target_os = "windows") {
        vec![PathBuf::from(r"C:\Windows\Fonts")]
    } else {
        [
            "/usr/share/fonts",
            "/usr/local/share/fonts",
            "/usr/share/fonts/truetype",
        ]
        .iter()
        .map(PathBuf::from)
        .collect()
    };

    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        dirs.push(home.join("Library/Fonts"));
        dirs.push(home.join(".local/share/fonts"));
        dirs.push(home.join(".fonts"));
    }

    dirs
}

/// Files holding a well-known UI family whose name matches neither the filename
/// nor the family recorded inside the font.
///
/// macOS ships "SF Pro" as `.SF NS` inside `SFNS.ttf`, so searching by name
/// never finds the most commonly declared family on the platform.
fn aliased_font_files(slug: &str) -> &'static [&'static str] {
    match slug {
        "sfpro" | "sfprodisplay" | "sfprotext" | "sfns" | "systemfont" | "appleystem" => &[
            "/System/Library/Fonts/SFNS.ttf",
            "/System/Library/Fonts/SFNSItalic.ttf",
        ],
        "sfprorounded" => &["/System/Library/Fonts/SFNSRounded.ttf"],
        "sfcompact" | "sfcompactdisplay" | "sfcompacttext" => &[
            "/System/Library/Fonts/SFCompact.ttf",
            "/System/Library/Fonts/SFCompactItalic.ttf",
        ],
        "sfmono" => &[
            "/System/Library/Fonts/SFNSMono.ttf",
            "/System/Library/Fonts/SFNSMonoItalic.ttf",
        ],
        "newyork" => &["/System/Library/Fonts/NewYork.ttf"],
        _ => &[],
    }
}

fn family_slug(family: &str) -> String {
    family
        .to_ascii_lowercase()
        .replace([' ', '-', '_', '.'], "")
}

/// Every face on the host that could serve `family`, in no particular order.
fn system_font_candidates(family: &str) -> Vec<SystemFace> {
    let slug = family_slug(family);
    let mut paths: Vec<PathBuf> = aliased_font_files(&slug)
        .iter()
        .map(PathBuf::from)
        .filter(|path| path.is_file())
        .collect();

    for dir in system_font_dirs() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            let lowercase = name.to_ascii_lowercase();
            if !(lowercase.ends_with(".ttf")
                || lowercase.ends_with(".otf")
                || lowercase.ends_with(".ttc"))
            {
                continue;
            }
            if family_slug(name).contains(&slug) && !paths.contains(&path) {
                paths.push(path);
            }
        }
    }

    let mut candidates = Vec::new();
    for path in paths {
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        let aliased = aliased_font_files(&slug)
            .iter()
            .any(|alias| path == PathBuf::from(alias));

        let face_count = ttf_parser::fonts_in_collection(&bytes).unwrap_or(1);
        for index in 0..face_count {
            let Ok(face) = Face::parse(&bytes, index) else {
                continue;
            };
            // An aliased file is trusted by path; everything else has to name
            // the family the document asked for.
            if !aliased && !face_family_matches(&face, &slug) {
                continue;
            }
            candidates.push(SystemFace {
                path: path.clone(),
                index,
                weight: face.weight().to_number(),
                italic: face.is_italic(),
                variable: face.is_variable(),
            });
        }
    }

    candidates
}

fn face_family_matches(face: &Face<'_>, slug: &str) -> bool {
    // Name ID 16 is the typographic family, 1 the legacy family name.
    let mut names = face
        .names()
        .into_iter()
        .filter(|name| name.name_id == 16 || name.name_id == 1)
        .filter_map(|name| name.to_string());

    names.any(|name| {
        let face_slug = family_slug(&name);
        face_slug == *slug || face_slug.contains(slug) || slug.contains(&face_slug)
    })
}

fn choose_system_face<'a>(
    candidates: &'a [SystemFace],
    weight: &str,
    style: &str,
) -> Option<&'a SystemFace> {
    let target_weight = normalize_weight(weight).parse::<u16>().unwrap_or(400);
    let target_italic = style == "italic";

    candidates.iter().max_by_key(|face| {
        // Matching the style matters more than matching the weight, and a
        // variable face can be instanced to whatever weight was asked for.
        let style_score = if face.italic == target_italic {
            10_000
        } else {
            0
        };
        let weight_score = if face.variable {
            1_000
        } else {
            1_000 - i32::from(face.weight).abs_diff(i32::from(target_weight)) as i32
        };
        style_score + weight_score
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn font_stretch_keywords_are_the_percentages_css_defines() {
        for (keyword, percentage) in [
            ("ultra-condensed", 50.0),
            ("condensed", 75.0),
            ("normal", 100.0),
            ("expanded", 125.0),
            ("ultra-expanded", 200.0),
        ] {
            let axes = FontAxes::from_style(Some(keyword), None, 16.0);
            assert_eq!(axes.width, Some(percentage), "{keyword}");
        }
    }

    #[test]
    fn font_stretch_takes_a_percentage_directly() {
        assert_eq!(
            FontAxes::from_style(Some("87.5%"), None, 16.0).width,
            Some(87.5)
        );
        assert_eq!(
            FontAxes::from_style(Some("120"), None, 16.0).width,
            Some(120.0)
        );
    }

    #[test]
    fn a_meaningless_font_stretch_sets_no_axis() {
        assert_eq!(
            FontAxes::from_style(Some("wide-ish"), None, 16.0).width,
            None
        );
        assert_eq!(FontAxes::from_style(Some("0%"), None, 16.0).width, None);
        assert_eq!(FontAxes::from_style(None, None, 16.0).width, None);
    }

    #[test]
    fn optical_sizing_is_driven_by_the_font_size_when_asked_for() {
        assert_eq!(
            FontAxes::from_style(None, Some("auto"), 28.0).optical_size,
            Some(28.0)
        );
        assert_eq!(
            FontAxes::from_style(None, Some("none"), 28.0).optical_size,
            None
        );
        // Absent, not `auto`: applying it by default would reflow existing
        // documents, which is tracked as its own change.
        assert_eq!(FontAxes::from_style(None, None, 28.0).optical_size, None);
    }

    #[test]
    fn empty_store_falls_back_to_approximate_width() {
        let store = FontStore::default();

        assert_eq!(
            store.text_width(
                "Hello",
                Some("Roboto"),
                Some("400"),
                Some("normal"),
                10.0,
                FontAxes::default()
            ),
            27.5
        );
    }

    #[test]
    fn face_lookup_requires_weight_and_style() {
        assert_ne!(
            FontFaceKey::new("Roboto", "500", "normal"),
            FontFaceKey::new("Roboto", "400", "normal")
        );
        assert_ne!(
            FontFaceKey::new("Roboto", "500", "normal"),
            FontFaceKey::new("Roboto", "500", "italic")
        );
    }

    #[test]
    fn font_declarations_default_to_regular_normal_face() {
        let info = FontInfo {
            source: "google".to_owned(),
            category: None,
            weights: None,
            styles: None,
            variants: None,
        };

        assert_eq!(declared_weights(&info), vec!["400"]);
        assert_eq!(declared_styles(&info), vec!["normal"]);
    }

    #[test]
    fn google_css_request_covers_every_declared_face_at_once() {
        let url = google_css_url(
            "Open Sans",
            &["700".to_owned(), "400".to_owned()],
            &["normal".to_owned(), "italic".to_owned()],
        )
        .expect("a family produces a request");

        // Spaces become `+`, and the axis list has to be sorted or the API
        // rejects it.
        assert_eq!(
            url,
            "https://fonts.googleapis.com/css2?family=Open+Sans:ital,wght@0,400;0,700;1,400;1,700"
        );
    }

    #[test]
    fn font_face_css_yields_a_url_per_weight_and_style() {
        let css = r#"
            @font-face {
              font-family: 'Roboto';
              font-style: normal;
              font-weight: 400;
              src: url(https://fonts.gstatic.com/s/roboto/v51/regular.ttf) format('truetype');
            }
            @font-face {
              font-family: 'Roboto';
              font-style: italic;
              font-weight: 700;
              src: url(https://fonts.gstatic.com/s/roboto/v51/bolditalic.ttf) format('truetype');
            }
        "#;

        let faces = parse_font_face_css(css);
        assert_eq!(faces.len(), 2);
        assert_eq!(
            faces.get(&("400".to_owned(), "normal".to_owned())).unwrap(),
            "https://fonts.gstatic.com/s/roboto/v51/regular.ttf"
        );
        assert_eq!(
            faces.get(&("700".to_owned(), "italic".to_owned())).unwrap(),
            "https://fonts.gstatic.com/s/roboto/v51/bolditalic.ttf"
        );
    }

    #[test]
    fn a_variable_face_is_keyed_by_the_start_of_its_weight_range() {
        let css = r#"
            @font-face {
              font-style: normal;
              font-weight: 100 900;
              src: url(https://fonts.gstatic.com/s/inter/variable.ttf) format('truetype');
            }
        "#;

        let faces = parse_font_face_css(css);
        assert!(faces.contains_key(&("100".to_owned(), "normal".to_owned())));
    }

    #[test]
    fn an_unpublished_weight_falls_back_to_the_nearest_one() {
        let mut faces = BTreeMap::new();
        faces.insert(
            ("400".to_owned(), "normal".to_owned()),
            "regular".to_owned(),
        );
        faces.insert(("700".to_owned(), "normal".to_owned()), "bold".to_owned());
        faces.insert(("400".to_owned(), "italic".to_owned()), "italic".to_owned());

        assert_eq!(nearest_face(&faces, "600", "normal").unwrap(), "bold");
        assert_eq!(nearest_face(&faces, "300", "normal").unwrap(), "regular");
        // Style is never traded away for a closer weight.
        assert_eq!(nearest_face(&faces, "700", "italic").unwrap(), "italic");
    }

    #[test]
    #[cfg_attr(not(target_os = "macos"), ignore = "searches macOS font directories")]
    fn resolves_a_system_font_by_name() {
        let candidates = system_font_candidates("Georgia");
        assert!(
            !candidates.is_empty(),
            "Georgia should be discoverable in the macOS font directories"
        );
        assert!(choose_system_face(&candidates, "400", "normal").is_some());
    }

    #[test]
    #[cfg_attr(not(target_os = "macos"), ignore = "searches macOS font directories")]
    fn resolves_the_apple_ui_family_through_its_alias() {
        // "SF Pro Display" is stored as `.SF NS` in SFNS.ttf, so it is only
        // reachable through the alias table.
        let candidates = system_font_candidates("SF Pro Display");
        assert!(
            !candidates.is_empty(),
            "SF Pro Display should resolve to the system UI font"
        );

        let chosen = choose_system_face(&candidates, "600", "normal")
            .expect("a face should be chosen for weight 600");
        assert!(
            chosen.variable,
            "the macOS UI font is variable, so one face serves every weight"
        );
    }

    #[test]
    fn face_selection_prefers_the_requested_style() {
        let candidates = vec![
            SystemFace {
                path: PathBuf::from("/fonts/regular.ttf"),
                index: 0,
                weight: 700,
                italic: false,
                variable: false,
            },
            SystemFace {
                path: PathBuf::from("/fonts/italic.ttf"),
                index: 0,
                weight: 400,
                italic: true,
                variable: false,
            },
        ];

        let chosen = choose_system_face(&candidates, "700", "italic").expect("a face is chosen");
        assert_eq!(chosen.path, PathBuf::from("/fonts/italic.ttf"));
    }
}
