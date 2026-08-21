use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuiDocument {
    pub version: String,
    pub name: Option<String>,
    pub metadata: GuiMetadata,
    pub root: GuiNode,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuiMetadata {
    pub tokens: BTreeMap<String, String>,
    pub fonts: BTreeMap<String, FontInfo>,
    pub styles: BTreeMap<String, BTreeMap<String, String>>,
    /// Named effect stacks from `<styles>`, keyed by name.
    ///
    /// A `<text-style>` is a bag of attributes, so it fits `styles`. An
    /// `<effect-style>` is an ordered list of `<effect>` children instead, so
    /// it needs its own shape.
    pub effect_styles: BTreeMap<String, Vec<BTreeMap<String, String>>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FontInfo {
    pub source: String,
    pub category: Option<String>,
    pub weights: Option<String>,
    pub styles: Option<String>,
    pub variants: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuiNode {
    pub tag: String,
    pub attributes: BTreeMap<String, String>,
    pub text: Option<String>,
    pub children: Vec<GuiNode>,
}

impl GuiNode {
    pub fn new(tag: impl Into<String>) -> Self {
        Self {
            tag: tag.into(),
            attributes: BTreeMap::new(),
            text: None,
            children: Vec::new(),
        }
    }
}
