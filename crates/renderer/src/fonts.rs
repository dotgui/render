use crate::{AssetCache, AssetError, FontInfo, GuiDocument, TextMeasurer};
use fontdue::{Font, FontSettings};
use serde::Deserialize;
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
}

pub struct FontFace {
    fallback: Font,
    bytes: Rc<Vec<u8>>,
    weight: f32,
    collection_index: u32,
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

    pub fn text_width(&self, value: &str, font_size: f32) -> f32 {
        if let Some(width) = self.variable_text_width(value, font_size) {
            return width;
        }

        value
            .chars()
            .map(|ch| self.fallback.metrics(ch, font_size).advance_width)
            .sum()
    }

    pub fn baseline_offset(&self, font_size: f32, line_height: f32) -> f32 {
        if let Some(face) = self.variable_face() {
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

    pub fn variable_face(&self) -> Option<Face<'_>> {
        let mut face = Face::parse(&self.bytes, self.collection_index).ok()?;
        let _ = face.set_variation(Tag::from_bytes(b"wght"), self.weight);
        Some(face)
    }

    fn variable_text_width(&self, value: &str, font_size: f32) -> Option<f32> {
        let face = self.variable_face()?;
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
            for weight in declared_weights(info) {
                for style in declared_styles(info) {
                    let source = match info.source.as_str() {
                        "google" => google_font_ttf_url(family, &weight, &style, cache)?
                            .map(FontSource::Google),
                        "system" => {
                            let candidates = system_faces
                                .entry(family.clone())
                                .or_insert_with(|| system_font_candidates(family));
                            choose_system_face(candidates, &weight, &style)
                                .cloned()
                                .map(FontSource::System)
                        }
                        _ => None,
                    };

                    let Some(source) = source else {
                        eprintln!(
                            "warning: {} font family '{family}' (weight {weight}, style {style}) could not be resolved",
                            info.source
                        );
                        continue;
                    };

                    // Weight is part of the key because variable faces are
                    // instanced per weight from the same bytes.
                    let loaded_key = (source.cache_key(), weight.clone());
                    let font = if let Some(font) = loaded_fonts.get(&loaded_key) {
                        Rc::clone(font)
                    } else {
                        let (bytes, collection_index) = source.load(cache)?;
                        let font =
                            Rc::new(FontFace::new(bytes, &weight, collection_index).map_err(
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
                        .insert(FontFaceKey::new(family, &weight, &style), font);
                }
            }
        }

        Ok(store)
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
    ) -> f32 {
        if let Some(face) = self.get(font_family, font_weight, font_style) {
            return face.text_width(value, font_size);
        }

        fallback_text_width(value, font_size)
    }
}

fn google_font_ttf_url(
    family: &str,
    weight: &str,
    style: &str,
    cache: &AssetCache,
) -> Result<Option<String>, FontError> {
    let slug = google_family_slug(family);
    if slug.is_empty() {
        return Ok(None);
    }

    for license_dir in ["ofl", "apache", "ufl"] {
        let entries = match google_font_entries(license_dir, &slug, family, cache) {
            Ok(entries) => entries,
            Err(FontError::Asset(AssetError::Fetch { message, .. }))
                if is_missing_google_font_directory(&message) =>
            {
                continue;
            }
            Err(err) => return Err(err),
        };
        if let Some(url) = select_google_ttf_url(&entries, weight, style) {
            return Ok(Some(url));
        }
    }

    Ok(None)
}

fn google_font_entries(
    license_dir: &str,
    slug: &str,
    family: &str,
    cache: &AssetCache,
) -> Result<Vec<GoogleFontEntry>, FontError> {
    let base_url =
        format!("https://api.github.com/repos/google/fonts/contents/{license_dir}/{slug}?ref=main");
    let mut entries = fetch_google_font_entries(&base_url, family, cache)?;

    let static_urls = entries
        .iter()
        .filter(|entry| entry.entry_type == "dir" && entry.name == "static")
        .map(|entry| {
            format!(
                "https://api.github.com/repos/google/fonts/contents/{}?ref=main",
                entry.path
            )
        })
        .collect::<Vec<_>>();

    for url in static_urls {
        entries.extend(fetch_google_font_entries(&url, family, cache)?);
    }

    Ok(entries)
}

fn fetch_google_font_entries(
    url: &str,
    family: &str,
    cache: &AssetCache,
) -> Result<Vec<GoogleFontEntry>, FontError> {
    let asset = cache.resolve(url)?;
    serde_json::from_slice(&asset.bytes).map_err(|err| FontError::GoogleMetadata {
        family: family.to_owned(),
        message: err.to_string(),
    })
}

#[derive(Debug, Clone, Deserialize)]
struct GoogleFontEntry {
    name: String,
    path: String,
    #[serde(rename = "type")]
    entry_type: String,
    download_url: Option<String>,
}

fn select_google_ttf_url(entries: &[GoogleFontEntry], weight: &str, style: &str) -> Option<String> {
    let normalized_style = normalize_style(style);
    let normalized_weight = normalize_weight(weight);

    entries
        .iter()
        .filter_map(|entry| {
            let url = entry.download_url.as_ref()?;
            let name = entry.name.to_ascii_lowercase();
            if !name.ends_with(".ttf") {
                return None;
            }

            if normalized_style == "italic" && !name.contains("italic") {
                return None;
            }
            if normalized_style == "normal" && name.contains("italic") {
                return None;
            }

            Some((font_file_score(&name, &normalized_weight), url))
        })
        .min_by_key(|(score, _)| *score)
        .map(|(_, url)| url.clone())
}

fn font_file_score(name: &str, weight: &str) -> u8 {
    if !name.contains('[') && static_file_matches_weight(name, weight) {
        return 0;
    }
    if name.contains("[wght]") || name.contains(",wght]") || name.contains("wght,") {
        return 10;
    }
    if !name.contains('[') {
        return 20;
    }
    30
}

fn static_file_matches_weight(name: &str, weight: &str) -> bool {
    match weight {
        "100" => name.contains("thin"),
        "200" => name.contains("extralight") || name.contains("ultralight"),
        "300" => name.contains("light"),
        "400" => name.contains("regular") || !name.contains('-'),
        "500" => name.contains("medium"),
        "600" => name.contains("semibold"),
        "700" => name.contains("bold") && !name.contains("extrabold"),
        "800" => name.contains("extrabold") || name.contains("ultrabold"),
        "900" => name.contains("black") || name.contains("heavy"),
        _ => false,
    }
}

fn google_family_slug(family: &str) -> String {
    family
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .flat_map(|ch| ch.to_lowercase())
        .collect()
}

fn is_missing_google_font_directory(message: &str) -> bool {
    message.contains("404") || message.to_ascii_lowercase().contains("not found")
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
    fn empty_store_falls_back_to_approximate_width() {
        let store = FontStore::default();

        assert_eq!(
            store.text_width("Hello", Some("Roboto"), Some("400"), Some("normal"), 10.0),
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
    fn google_family_slug_matches_repository_directory() {
        assert_eq!(google_family_slug("Roboto"), "roboto");
        assert_eq!(google_family_slug("Noto Sans"), "notosans");
    }

    #[test]
    fn google_ttf_selection_prefers_exact_static_face() {
        let entries = vec![
            google_entry("Roboto[wght].ttf"),
            google_entry("Roboto-Regular.ttf"),
            google_entry("Roboto-Bold.ttf"),
        ];

        assert_eq!(
            select_google_ttf_url(&entries, "700", "normal").as_deref(),
            Some("https://example.com/Roboto-Bold.ttf")
        );
    }

    #[test]
    fn google_ttf_selection_keeps_italic_separate() {
        let entries = vec![
            google_entry("Inter[opsz,wght].ttf"),
            google_entry("Inter-Italic[opsz,wght].ttf"),
        ];

        assert_eq!(
            select_google_ttf_url(&entries, "500", "italic").as_deref(),
            Some("https://example.com/Inter-Italic[opsz,wght].ttf")
        );
        assert_eq!(
            select_google_ttf_url(&entries, "500", "normal").as_deref(),
            Some("https://example.com/Inter[opsz,wght].ttf")
        );
    }

    fn google_entry(name: &str) -> GoogleFontEntry {
        GoogleFontEntry {
            name: name.to_owned(),
            path: format!("ofl/test/{name}"),
            entry_type: "file".to_owned(),
            download_url: Some(format!("https://example.com/{name}")),
        }
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
