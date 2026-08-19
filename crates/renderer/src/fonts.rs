use crate::{AssetCache, AssetError, FontInfo, GuiDocument, TextMeasurer};
use fontdue::{Font, FontSettings};
use serde::Deserialize;
use std::{collections::BTreeMap, rc::Rc};
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

        for (family, info) in &document.metadata.fonts {
            if info.source != "google" && info.source != "system" {
                continue;
            }

            for weight in declared_weights(info) {
                for style in declared_styles(info) {
                    if info.source == "google" {
                        if let Some(url) = google_font_ttf_url(family, &weight, &style, cache)? {
                            let loaded_key = (url.clone(), weight.clone());
                            let font = if let Some(font) = loaded_fonts.get(&loaded_key) {
                                Rc::clone(font)
                            } else {
                                let asset = cache.resolve(&url)?;
                                let font = Rc::new(FontFace::new(asset.bytes, &weight, 0).map_err(
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
                    } else if info.source == "system" {
                        if let Some((bytes, collection_index)) = find_system_font(family, &weight, &style) {
                            let loaded_key = (format!("system://{family}/{collection_index}"), weight.clone());
                            let font = if let Some(font) = loaded_fonts.get(&loaded_key) {
                                Rc::clone(font)
                            } else {
                                let font = Rc::new(FontFace::new(bytes, &weight, collection_index).map_err(
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

fn find_system_font(family: &str, weight: &str, style: &str) -> Option<(Vec<u8>, u32)> {
    let slug = family.to_ascii_lowercase().replace(' ', "").replace('-', "");
    let target_weight = normalize_weight(weight).parse::<u16>().unwrap_or(400);
    let target_italic = style == "italic";

    let dirs = [
        "/System/Library/Fonts",
        "/System/Library/Fonts/Supplemental",
        "/Library/Fonts",
    ];

    let mut candidates = Vec::new();

    for dir in dirs {
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let Some(name) = path.file_name().and_then(|n| n.to_str()).map(|n| n.to_ascii_lowercase()) else {
                continue;
            };
            if name.ends_with(".ttf") || name.ends_with(".otf") || name.ends_with(".ttc") {
                let name_slug = name.replace(' ', "").replace('-', "");
                if name_slug.contains(&slug) {
                    candidates.push(path);
                }
            }
        }
    }

    let mut best_bytes = None;
    let mut best_index = 0;
    let mut best_score = -1;

    for path in candidates {
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(_) => continue,
        };
        let num_fonts = ttf_parser::fonts_in_collection(&bytes).unwrap_or(1);
        for index in 0..num_fonts {
            if let Ok(face) = Face::parse(&bytes, index) {
                let mut face_family = None;
                for name in face.names() {
                    let name_id = name.name_id;
                    // Name ID 16: Typographic Family, Name ID 1: Font Family
                    if name_id == 16 || (name_id == 1 && face_family.is_none()) {
                        if let Some(s) = name.to_string() {
                            face_family = Some(s);
                        }
                    }
                }
                if let Some(face_family) = face_family {
                    let face_slug = face_family.to_ascii_lowercase().replace(' ', "").replace('-', "");
                    if face_slug == slug || face_slug.contains(&slug) || slug.contains(&face_slug) {
                        let face_weight = face.weight().to_number();
                        let face_italic = face.is_italic();
                        
                        let style_score = if face_italic == target_italic { 10 } else { 0 };
                        let weight_diff = (face_weight as i32 - target_weight as i32).abs();
                        let weight_score = 1000 - weight_diff;
                        let score = style_score + weight_score;

                        if score > best_score {
                            best_score = score;
                            best_bytes = Some(bytes.clone());
                            best_index = index;
                        }
                    }
                }
            }
        }
    }

    best_bytes.map(|bytes| (bytes, best_index))
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
    fn resolves_system_font_georgia() {
        let res = find_system_font("Georgia", "400", "normal");
        if cfg!(target_os = "macos") {
            assert!(res.is_some(), "Should find Georgia on macOS system fonts");
            let (bytes, index) = res.unwrap();
            assert!(bytes.len() > 0);
            assert_eq!(index, 0);
        }
    }
}
