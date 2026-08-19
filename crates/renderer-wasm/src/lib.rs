use wasm_bindgen::prelude::*;
use dotgui_renderer::{
    parse_gui_xml, compute_taffy_layout, build_scene, paint_scene_to_png_bytes,
    AssetCache, FontStore,
};

#[wasm_bindgen]
pub fn parse_gui_summary(xml: &str) -> Result<String, JsValue> {
    let document = parse_gui_xml(xml).map_err(|err| JsValue::from_str(&err.to_string()))?;
    serde_json::to_string_pretty(&document).map_err(|err| JsValue::from_str(&err.to_string()))
}

#[wasm_bindgen]
pub fn compute_layout_json(xml: &str) -> Result<String, JsValue> {
    let document = parse_gui_xml(xml).map_err(|err| JsValue::from_str(&err.to_string()))?;
    let layout = compute_taffy_layout(&document).map_err(|err| JsValue::from_str(&err.to_string()))?;
    serde_json::to_string_pretty(&layout).map_err(|err| JsValue::from_str(&err.to_string()))
}

#[wasm_bindgen]
pub fn build_scene_json(xml: &str) -> Result<String, JsValue> {
    let document = parse_gui_xml(xml).map_err(|err| JsValue::from_str(&err.to_string()))?;
    let layout = compute_taffy_layout(&document).map_err(|err| JsValue::from_str(&err.to_string()))?;
    let scene = build_scene(&document, &layout);
    serde_json::to_string_pretty(&scene).map_err(|err| JsValue::from_str(&err.to_string()))
}

#[wasm_bindgen]
pub fn render_png_from_xml(xml: &str) -> Result<Vec<u8>, JsValue> {
    let document = parse_gui_xml(xml).map_err(|err| JsValue::from_str(&err.to_string()))?;
    // Create an empty temp cache directory for WASM context
    let cache = AssetCache::new(std::env::temp_dir());
    let fonts = FontStore::from_document(&document, &cache)
        .unwrap_or_else(|_| FontStore::default());
    let layout = compute_taffy_layout(&document).map_err(|err| JsValue::from_str(&err.to_string()))?;
    let scene = build_scene(&document, &layout);
    paint_scene_to_png_bytes(&scene, Some(&cache), Some(&fonts))
        .map_err(|err| JsValue::from_str(&err.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wasm_apis() {
        let xml = r##"
        <gui version="0.2">
          <col w="100" h="100" fill="#ffffff">
            <rect w="50" h="50" fill="#ff0000" />
          </col>
        </gui>
        "##;

        let summary = parse_gui_summary(xml).unwrap();
        assert!(summary.contains("metadata"));

        let layout = compute_layout_json(xml).unwrap();
        assert!(layout.contains("width"));

        let scene = build_scene_json(xml).unwrap();
        assert!(scene.contains("root"));

        let png = render_png_from_xml(xml).unwrap();
        assert!(png.len() > 0);
    }
}
