use wasm_bindgen::prelude::*;

#[wasm_bindgen]
pub fn parse_gui_summary(xml: &str) -> Result<String, JsValue> {
    let document =
        dotgui_renderer::parse_gui_xml(xml).map_err(|err| JsValue::from_str(&err.to_string()))?;

    serde_json::to_string_pretty(&document).map_err(|err| JsValue::from_str(&err.to_string()))
}
