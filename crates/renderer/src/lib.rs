//! Native renderer core for `.gui`.
//!
//! This crate intentionally starts with parsing and a Rust-owned document model.
//! Layout, scene construction, and painting should build on this model rather
//! than depending on DOM, CSS, SVG, or browser primitives.

mod assets;
mod fonts;
mod layout;
mod model;
mod package;
mod paint;
mod parser;
mod scene;
mod taffy_layout;

pub use assets::{AssetCache, AssetError, ResolvedAsset};
pub use fonts::{FontError, FontFace, FontStore};
pub use layout::{
    compute_layout, compute_layout_with_text, ApproxTextMeasurer, LayoutBox, LayoutRect,
    TextMeasurer,
};
pub use model::{FontInfo, GuiDocument, GuiMetadata, GuiNode};
pub use package::{read_gui_package, read_gui_package_xml, GuiPackage, PackageError};
pub use paint::{
    paint_scene_to_png, paint_scene_to_png_with_assets, paint_scene_to_png_with_assets_and_fonts,
    paint_scene_to_png_bytes, PaintError,
};
pub use parser::{parse_gui_xml, ParseError};
pub use scene::{build_scene, Border, BorderWidths, PaintContent, Scene, SceneNode};
pub use taffy_layout::{compute_taffy_layout, TaffyLayoutError};

#[cfg(test)]
mod examples_tests {
    use super::{build_scene, compute_taffy_layout, parse_gui_xml, read_gui_package_xml};
    use std::{fs, path::PathBuf};

    #[test]
    fn parses_all_workspace_gui_examples() {
        let examples_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("examples");

        if !examples_dir.exists() {
            return;
        }

        let mut count = 0;
        for entry in fs::read_dir(&examples_dir).expect("examples dir is readable") {
            let path = entry.expect("example entry is readable").path();
            if path.extension().is_none_or(|ext| ext != "gui") {
                continue;
            }

            let bytes = fs::read(&path).expect("example can be read");
            let xml = read_gui_package_xml(&bytes).unwrap_or_else(|err| {
                panic!("{} did not open as .gui package: {err}", path.display())
            });
            parse_gui_xml(&xml)
                .unwrap_or_else(|err| panic!("{} did not parse: {err}", path.display()));
            count += 1;
        }

        assert!(count > 0, "expected at least one .gui example");
    }

    #[test]
    fn computes_taffy_layout_for_all_workspace_gui_examples() {
        let examples_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("examples");

        if !examples_dir.exists() {
            return;
        }

        let mut count = 0;
        for entry in fs::read_dir(&examples_dir).expect("examples dir is readable") {
            let path = entry.expect("example entry is readable").path();
            if path.extension().is_none_or(|ext| ext != "gui") {
                continue;
            }

            let bytes = fs::read(&path).expect("example can be read");
            let xml = read_gui_package_xml(&bytes).unwrap_or_else(|err| {
                panic!("{} did not open as .gui package: {err}", path.display())
            });
            let document = parse_gui_xml(&xml)
                .unwrap_or_else(|err| panic!("{} did not parse: {err}", path.display()));
            compute_taffy_layout(&document).unwrap_or_else(|err| {
                panic!("{} did not layout with Taffy: {err}", path.display())
            });
            count += 1;
        }

        assert!(count > 0, "expected at least one .gui example");
    }

    #[test]
    fn builds_scene_for_all_workspace_gui_examples() {
        let examples_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("examples");

        if !examples_dir.exists() {
            return;
        }

        let mut count = 0;
        for entry in fs::read_dir(&examples_dir).expect("examples dir is readable") {
            let path = entry.expect("example entry is readable").path();
            if path.extension().is_none_or(|ext| ext != "gui") {
                continue;
            }

            let bytes = fs::read(&path).expect("example can be read");
            let xml = read_gui_package_xml(&bytes).unwrap_or_else(|err| {
                panic!("{} did not open as .gui package: {err}", path.display())
            });
            let document = parse_gui_xml(&xml)
                .unwrap_or_else(|err| panic!("{} did not parse: {err}", path.display()));
            let layout = compute_taffy_layout(&document).unwrap_or_else(|err| {
                panic!("{} did not layout with Taffy: {err}", path.display())
            });
            let scene = build_scene(&document, &layout);
            assert!(
                scene.root.bounds.width >= 0.0,
                "{} scene root has invalid width",
                path.display()
            );
            count += 1;
        }

        assert!(count > 0, "expected at least one .gui example");
    }
}
