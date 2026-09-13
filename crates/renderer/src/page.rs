//! A page that is parsed and laid out once, then painted as often as needed.
//!
//! Layout is the expensive step that does not depend on how the page is
//! shown: zooming, panning, a thumbnail and a screenshot of one area all paint
//! the same laid-out page at a different scale or over a different rectangle.
//! A [`Page`] keeps the document, its layout and its scene, so each of those is
//! only a paint.
//!
//! It also takes screenshots: the whole page, an area of it given in document
//! pixels, or one element, found by an attribute — its `id`, its layer `name`,
//! or the `data-uid` an editor stamps on what it selects.

use crate::{
    build_scene, compute_taffy_layout_with_text, paint_scene_region_to_png_bytes,
    paint_scene_region_to_rgba, paint_scene_to_png_bytes, paint_scene_to_rgba, scene_pixel_size,
    AssetCache, FontStore, GuiDocument, LayoutBox, LayoutRect, PaintError, PixelRect, Scene,
    TaffyLayoutError,
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

/// The most pixels a screenshot may have: a 16384×16384 image, a little over a
/// gigabyte of RGBA. Past that a mistyped scale would exhaust memory rather
/// than produce anything useful.
const MAX_SCREENSHOT_PIXELS: u64 = 16384 * 16384;

impl Page {
    /// The box of the first element, in document order, whose `attribute` is
    /// `value`, in document pixels.
    ///
    /// An instance's parts keep the ids their component gave them, so an id
    /// used inside a component matches its first instance.
    pub fn element_bounds(&self, attribute: &str, value: &str) -> Option<LayoutRect> {
        find_box(&self.layout, attribute, value).map(|found| found.rect)
    }

    /// An area of the page, given in document pixels, at `scale`, as straight
    /// RGBA. The area's edges are snapped outward to whole pixels. What lies
    /// past the page is transparent.
    pub fn paint_area(
        &self,
        area: LayoutRect,
        scale: f32,
        assets: Option<&AssetCache>,
        fonts: Option<&FontStore>,
    ) -> Result<(u32, u32, Vec<u8>), PaintError> {
        let region = screenshot_region(area, scale)?;
        self.paint_region(scale, region, assets, fonts)
    }

    /// [`Page::paint_area`], as a PNG.
    pub fn paint_area_png(
        &self,
        area: LayoutRect,
        scale: f32,
        assets: Option<&AssetCache>,
        fonts: Option<&FontStore>,
    ) -> Result<Vec<u8>, PaintError> {
        let region = screenshot_region(area, scale)?;
        self.paint_region_png(scale, region, assets, fonts)
    }

    /// One element as it appears on the page, grown by `padding` document
    /// pixels on every side — room for a shadow or an outline — at `scale`, as
    /// straight RGBA.
    ///
    /// This is a picture of that part of the page, so whatever sits behind or
    /// over the element is in it too.
    pub fn paint_element(
        &self,
        attribute: &str,
        value: &str,
        scale: f32,
        padding: f32,
        assets: Option<&AssetCache>,
        fonts: Option<&FontStore>,
    ) -> Result<(u32, u32, Vec<u8>), PaintError> {
        let area = self.element_area(attribute, value, padding)?;
        self.paint_area(area, scale, assets, fonts)
    }

    /// [`Page::paint_element`], as a PNG.
    pub fn paint_element_png(
        &self,
        attribute: &str,
        value: &str,
        scale: f32,
        padding: f32,
        assets: Option<&AssetCache>,
        fonts: Option<&FontStore>,
    ) -> Result<Vec<u8>, PaintError> {
        let area = self.element_area(attribute, value, padding)?;
        self.paint_area_png(area, scale, assets, fonts)
    }

    /// The box of the element whose `attribute` is `value`, grown by
    /// `padding` document pixels on every side.
    pub fn element_area(
        &self,
        attribute: &str,
        value: &str,
        padding: f32,
    ) -> Result<LayoutRect, PaintError> {
        let rect =
            self.element_bounds(attribute, value)
                .ok_or_else(|| PaintError::NoSuchElement {
                    attribute: attribute.to_owned(),
                    value: value.to_owned(),
                })?;
        let padding = padding.max(0.0);
        Ok(LayoutRect {
            x: rect.x - padding,
            y: rect.y - padding,
            width: rect.width + padding * 2.0,
            height: rect.height + padding * 2.0,
        })
    }
}

/// The pixels covering `area` at `scale`, when that is an image worth painting.
fn screenshot_region(area: LayoutRect, scale: f32) -> Result<PixelRect, PaintError> {
    let region = PixelRect::covering(area, usable_scale(scale));
    let pixels = u64::from(region.width) * u64::from(region.height);
    if pixels == 0 {
        return Err(PaintError::InvalidSize {
            width: area.width * scale,
            height: area.height * scale,
        });
    }
    if pixels > MAX_SCREENSHOT_PIXELS {
        return Err(PaintError::TooLarge {
            width: region.width,
            height: region.height,
        });
    }
    Ok(region)
}

fn find_box<'a>(node: &'a LayoutBox, attribute: &str, value: &str) -> Option<&'a LayoutBox> {
    if node
        .attributes
        .get(attribute)
        .is_some_and(|found| found == value)
    {
        return Some(node);
    }
    node.children
        .iter()
        .find_map(|child| find_box(child, attribute, value))
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

    const CARD: &str = r##"<gui version="0.2">
      <col w="200" h="120" fill="#ffffff" p="20" gap="10">
        <rect id="badge" w="30" h="30" fill="#00ff00" />
        <row name="Card" w="120" h="40" fill="#0000ff" />
      </col>
    </gui>"##;

    fn card() -> Page {
        Page::new(parse_gui_xml(CARD).unwrap(), &FontStore::default()).unwrap()
    }

    #[test]
    fn an_element_is_found_by_any_attribute() {
        let page = card();
        let badge = page.element_bounds("id", "badge").unwrap();
        assert_eq!(
            (badge.x, badge.y, badge.width, badge.height),
            (20.0, 20.0, 30.0, 30.0)
        );
        let card = page.element_bounds("name", "Card").unwrap();
        assert_eq!((card.x, card.y), (20.0, 60.0));
        assert!(page.element_bounds("id", "missing").is_none());
    }

    #[test]
    fn an_element_screenshot_is_the_element_at_scale_with_padding() {
        let page = card();
        let (width, height, pixels) = page
            .paint_element("id", "badge", 2.0, 5.0, None, None)
            .unwrap();
        // 30 plus 5 each side, at 2x.
        assert_eq!((width, height), (80, 80));
        let at = |x: u32, y: u32| {
            let i = ((y * width + x) * 4) as usize;
            [pixels[i], pixels[i + 1], pixels[i + 2], pixels[i + 3]]
        };
        assert_eq!(at(2, 2), [255, 255, 255, 255], "padding shows the page");
        assert_eq!(at(40, 40), [0, 255, 0, 255], "the middle is the element");

        let png = page
            .paint_element_png("name", "Card", 1.0, 0.0, None, None)
            .unwrap();
        assert!(png.starts_with(b"\x89PNG"));
    }

    #[test]
    fn an_area_snaps_outward_to_whole_pixels() {
        let area = LayoutRect {
            x: 10.25,
            y: 5.5,
            width: 20.5,
            height: 10.0,
        };
        let region = PixelRect::covering(area, 2.0);
        assert_eq!(
            (region.x, region.y, region.width, region.height),
            (20, 11, 42, 20)
        );

        let (width, height, _) = card().paint_area(area, 2.0, None, None).unwrap();
        assert_eq!((width, height), (42, 20));
    }

    #[test]
    fn a_missing_element_or_an_absurd_size_is_an_error() {
        let page = card();
        assert!(matches!(
            page.paint_element("id", "nope", 1.0, 0.0, None, None),
            Err(PaintError::NoSuchElement { .. })
        ));
        let huge = LayoutRect {
            x: 0.0,
            y: 0.0,
            width: 200.0,
            height: 120.0,
        };
        assert!(matches!(
            page.paint_area(huge, 500.0, None, None),
            Err(PaintError::TooLarge { .. })
        ));
    }
}
