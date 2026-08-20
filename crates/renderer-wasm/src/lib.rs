//! WASM bindings for the `.gui` renderer.
//!
//! The browser has no filesystem and no HTTP client we can reach from Rust, so
//! this crate builds `dotgui-renderer` with its `net` feature off. Assets and
//! fonts have to travel with the document: prefer the `*_from_package` entry
//! points, which read a packaged `.gui` from memory.

use dotgui_renderer::{
    build_scene, compute_taffy_layout_with_text, paint_scene_to_png_bytes, parse_gui_xml,
    read_gui_package, AssetCache, FontStore, GuiDocument,
};
use std::collections::BTreeMap;
use wasm_bindgen::prelude::*;

/// A parsed document plus whatever assets came packaged with it.
struct Loaded {
    document: GuiDocument,
    cache: AssetCache,
}

fn to_js(err: impl std::fmt::Display) -> JsValue {
    JsValue::from_str(&err.to_string())
}

fn load_xml(xml: &str) -> Result<Loaded, JsValue> {
    Ok(Loaded {
        document: parse_gui_xml(xml).map_err(to_js)?,
        // No packaged assets: `src` values that are not data URIs cannot be
        // resolved without a host filesystem.
        cache: in_memory_cache(BTreeMap::new()),
    })
}

fn load_package(bytes: &[u8]) -> Result<Loaded, JsValue> {
    let package = read_gui_package(bytes).map_err(to_js)?;
    Ok(Loaded {
        document: parse_gui_xml(&package.xml).map_err(to_js)?,
        cache: in_memory_cache(package.assets),
    })
}

/// An asset cache that never touches disk.
///
/// `AssetCache` only reads or writes its root when it has to fetch something
/// remote, which the `net` feature gates off in this build.
fn in_memory_cache(assets: BTreeMap<String, Vec<u8>>) -> AssetCache {
    AssetCache::new(".").with_package_assets(assets)
}

impl Loaded {
    fn render_png(&self) -> Result<Vec<u8>, JsValue> {
        let fonts = FontStore::from_document(&self.document, &self.cache).unwrap_or_default();
        let layout = compute_taffy_layout_with_text(&self.document, &fonts).map_err(to_js)?;
        let scene = build_scene(&self.document, &layout);
        paint_scene_to_png_bytes(&scene, Some(&self.cache), Some(&fonts)).map_err(to_js)
    }

    fn scene_json(&self) -> Result<String, JsValue> {
        let fonts = FontStore::from_document(&self.document, &self.cache).unwrap_or_default();
        let layout = compute_taffy_layout_with_text(&self.document, &fonts).map_err(to_js)?;
        let scene = build_scene(&self.document, &layout);
        serde_json::to_string_pretty(&scene).map_err(to_js)
    }

    fn layout_json(&self) -> Result<String, JsValue> {
        let fonts = FontStore::from_document(&self.document, &self.cache).unwrap_or_default();
        let layout = compute_taffy_layout_with_text(&self.document, &fonts).map_err(to_js)?;
        serde_json::to_string_pretty(&layout).map_err(to_js)
    }
}

#[wasm_bindgen]
pub fn parse_gui_summary(xml: &str) -> Result<String, JsValue> {
    let document = parse_gui_xml(xml).map_err(to_js)?;
    serde_json::to_string_pretty(&document).map_err(to_js)
}

#[wasm_bindgen]
pub fn compute_layout_json(xml: &str) -> Result<String, JsValue> {
    load_xml(xml)?.layout_json()
}

#[wasm_bindgen]
pub fn build_scene_json(xml: &str) -> Result<String, JsValue> {
    load_xml(xml)?.scene_json()
}

#[wasm_bindgen]
pub fn render_png_from_xml(xml: &str) -> Result<Vec<u8>, JsValue> {
    load_xml(xml)?.render_png()
}

/// Reads a packaged `.gui` (a ZIP) from memory.
///
/// This is the entry point to prefer in a browser: images and fonts inside the
/// package resolve without any host I/O.
#[wasm_bindgen]
pub fn parse_package_summary(package: &[u8]) -> Result<String, JsValue> {
    let loaded = load_package(package)?;
    serde_json::to_string_pretty(&loaded.document).map_err(to_js)
}

#[wasm_bindgen]
pub fn compute_layout_json_from_package(package: &[u8]) -> Result<String, JsValue> {
    load_package(package)?.layout_json()
}

#[wasm_bindgen]
pub fn build_scene_json_from_package(package: &[u8]) -> Result<String, JsValue> {
    load_package(package)?.scene_json()
}

#[wasm_bindgen]
pub fn render_png_from_package(package: &[u8]) -> Result<Vec<u8>, JsValue> {
    load_package(package)?.render_png()
}

#[cfg(test)]
mod tests {
    use super::*;

    const XML: &str = r##"
    <gui version="0.2">
      <col w="100" h="100" fill="#ffffff">
        <rect w="50" h="50" fill="#ff0000" />
        <text value="hello" font-size="10" />
      </col>
    </gui>
    "##;

    #[test]
    fn exposes_document_layout_scene_and_png() {
        assert!(parse_gui_summary(XML).unwrap().contains("metadata"));
        assert!(compute_layout_json(XML).unwrap().contains("width"));
        assert!(build_scene_json(XML).unwrap().contains("root"));

        let png = render_png_from_xml(XML).unwrap();
        assert!(png.starts_with(b"\x89PNG"), "expected a PNG signature");
    }

    #[test]
    fn renders_without_touching_the_filesystem() {
        // The browser build has no cwd to fall back on, so rendering must not
        // depend on the cache root existing.
        let cache = in_memory_cache(BTreeMap::new());
        assert!(!cache.root().join("definitely-missing").exists());

        let png = render_png_from_xml(XML).unwrap();
        assert!(!png.is_empty());
    }
}
