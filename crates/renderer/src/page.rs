//! A page that is parsed and laid out once, then painted as often as needed.
//!
//! Layout is the expensive step that does not depend on how the page is
//! shown: zooming, panning, a thumbnail and a screenshot of one area all paint
//! the same laid-out page at a different scale or over a different rectangle.
//! A [`Page`] keeps the document, its layout and its scene, so each of those is
//! only a paint.

use crate::{
    build_scene, compute_taffy_layout_with_text, paint_scene_region_to_png_bytes,
    paint_scene_region_to_rgba, paint_scene_to_png_bytes, paint_scene_to_rgba, scene_pixel_size,
    AssetCache, FontStore, GuiDocument, LayoutBox, PaintError, PixelRect, Scene, TaffyLayoutError,
};
use std::sync::{Arc, Mutex};

/// A laid-out page, ready to paint whole or by region at any scale.
#[derive(Debug)]
pub struct Page {
    document: GuiDocument,
    layout: LayoutBox,
    scene: Arc<Scene>,
    /// The scene at the last scale asked for. A view that zooms paints many
    /// regions at one scale before it changes, and scaling a scene walks all of
    /// it.
    scaled: Mutex<Option<(f32, Arc<Scene>)>>,
}

impl Page {
    /// Lays `document` out with `fonts`, which decide where text breaks.
    pub fn new(document: GuiDocument, fonts: &FontStore) -> Result<Self, TaffyLayoutError> {
        let layout = compute_taffy_layout_with_text(&document, fonts)?;
        let scene = Arc::new(build_scene(&document, &layout));
        Ok(Self {
            document,
            layout,
            scene,
            scaled: Mutex::new(None),
        })
    }

    pub fn document(&self) -> &GuiDocument {
        &self.document
    }

    /// Every element's box, in document pixels.
    pub fn layout(&self) -> &LayoutBox {
        &self.layout
    }

    /// The scene at 1x.
    pub fn scene(&self) -> &Scene {
        &self.scene
    }

    /// The page's size in document pixels.
    pub fn size(&self) -> (f32, f32) {
        (self.layout.rect.width, self.layout.rect.height)
    }

    /// The page's size in pixels when painted at `scale`.
    pub fn pixel_size(&self, scale: f32) -> (u32, u32) {
        scene_pixel_size(&self.scene_at(scale))
    }

    /// The scene at `scale`: 1 is document pixels, 2 is twice as many each way.
    pub fn scene_at(&self, scale: f32) -> Arc<Scene> {
        let scale = usable_scale(scale);
        if scale == 1.0 {
            return Arc::clone(&self.scene);
        }
        let mut scaled = self
            .scaled
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some((cached, scene)) = scaled.as_ref() {
            if *cached == scale {
                return Arc::clone(scene);
            }
        }
        let scene = Arc::new(self.scene.scaled(scale));
        *scaled = Some((scale, Arc::clone(&scene)));
        scene
    }

    /// The whole page at `scale`, as straight RGBA.
    pub fn paint(
        &self,
        scale: f32,
        assets: Option<&AssetCache>,
        fonts: Option<&FontStore>,
    ) -> Result<(u32, u32, Vec<u8>), PaintError> {
        paint_scene_to_rgba(&self.scene_at(scale), assets, fonts)
    }

    /// One rectangle of the page at `scale`, as straight RGBA. `region` is in
    /// the pixels of the page painted at that scale, so the rectangles of a
    /// grid tile the whole page exactly.
    pub fn paint_region(
        &self,
        scale: f32,
        region: PixelRect,
        assets: Option<&AssetCache>,
        fonts: Option<&FontStore>,
    ) -> Result<(u32, u32, Vec<u8>), PaintError> {
        paint_scene_region_to_rgba(&self.scene_at(scale), region, assets, fonts)
    }

    /// The whole page at `scale`, as a PNG.
    pub fn paint_png(
        &self,
        scale: f32,
        assets: Option<&AssetCache>,
        fonts: Option<&FontStore>,
    ) -> Result<Vec<u8>, PaintError> {
        paint_scene_to_png_bytes(&self.scene_at(scale), assets, fonts)
    }

    /// One rectangle of the page at `scale`, as a PNG.
    pub fn paint_region_png(
        &self,
        scale: f32,
        region: PixelRect,
        assets: Option<&AssetCache>,
        fonts: Option<&FontStore>,
    ) -> Result<Vec<u8>, PaintError> {
        paint_scene_region_to_png_bytes(&self.scene_at(scale), region, assets, fonts)
    }
}

/// A scale that paints something: anything else is 1.
fn usable_scale(scale: f32) -> f32 {
    if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse_gui_xml;

    const XML: &str = r##"<gui version="0.2">
      <col w="100" h="60" fill="#ffffff">
        <rect w="40" h="20" fill="#ff0000" />
      </col>
    </gui>"##;

    fn page() -> Page {
        Page::new(parse_gui_xml(XML).unwrap(), &FontStore::default()).unwrap()
    }

    #[test]
    fn a_page_paints_whole_and_by_region_at_any_scale() {
        let page = page();
        assert_eq!(page.size(), (100.0, 60.0));
        assert_eq!(page.pixel_size(2.0), (200, 120));

        let (width, height, whole) = page.paint(2.0, None, None).unwrap();
        assert_eq!((width, height), (200, 120));
        assert_eq!(&whole[..4], &[255, 0, 0, 255]);

        let region = PixelRect {
            x: 60,
            y: 20,
            width: 40,
            height: 30,
        };
        let (_, _, part) = page.paint_region(2.0, region, None, None).unwrap();
        for y in 0..30 {
            for x in 0..40 {
                let i = ((y * 40 + x) * 4) as usize;
                let j = (((y + 20) * 200 + x + 60) * 4) as usize;
                assert_eq!(part[i..i + 4], whole[j..j + 4], "({x}, {y})");
            }
        }
    }

    #[test]
    fn the_scaled_scene_is_kept_for_the_next_paint_at_that_scale() {
        let page = page();
        let first = page.scene_at(1.5);
        assert!(Arc::ptr_eq(&first, &page.scene_at(1.5)));
        assert!(!Arc::ptr_eq(&first, &page.scene_at(3.0)));
        assert!(Arc::ptr_eq(&page.scene_at(1.0), &page.scene_at(f32::NAN)));
    }
}
