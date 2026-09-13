//! WASM bindings for the `.gui` renderer.
//!
//! The browser has no filesystem and no HTTP client we can reach from Rust, so
//! this crate builds `dotgui-renderer` with its `net` feature off. Assets and
//! fonts have to travel with the document: prefer the `*_from_package` entry
//! points, which read a packaged `.gui` from memory.

use dotgui_renderer::{
    build_scene, compute_taffy_layout_with_text, font_urls_in_stylesheet, google_stylesheet_urls,
    missing_system_font_files, normalize_presence_attrs, paint_scene_to_png_bytes,
    paint_scene_to_rgba, parse_gui_xml, parse_gui_xml_with, parse_library, parse_standalone_xml,
    read_gui_package, AssetCache, FontStore, GuiDocument, GuiNode, Library, ParseError,
    ParseOptions, RENDERER_VERSION, SUPPORTED_VERSION,
};
use std::{cell::RefCell, collections::BTreeMap, rc::Rc};
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
        // Markup handed over on its own is a standalone document (RFC-0043).
        document: parse_standalone_xml(xml).map_err(to_js)?,
        // No packaged assets: `src` values that are not data URIs cannot be
        // resolved without a host filesystem.
        cache: in_memory_cache(BTreeMap::new()),
    })
}

/// A package's only document. These one-shot entry points draw one screen; a
/// package of several goes through [`Engine::load_package`].
fn load_package(bytes: &[u8]) -> Result<Loaded, JsValue> {
    let package = read_gui_package(bytes).map_err(to_js)?;
    let document = package.single_document().map_err(to_js)?;
    let library = package.parse_library();
    Ok(Loaded {
        document: package
            .parse_document(document, library.as_ref())
            .map_err(to_js)?,
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

/// A renderer that outlives one call, for a host that redraws as a document is
/// edited.
///
/// It holds the bytes a document needs and cannot fetch for itself: a package's
/// own assets, and whatever the host has fetched on its behalf — Google font
/// stylesheets and files, remote images, the host's system font files — each
/// kept under the exact URL or path the document resolves it by. The markup is
/// not held: the host owns it, edits it, and passes the current text to every
/// [`Engine::render`].
#[wasm_bindgen]
pub struct Engine {
    cache: AssetCache,
    /// The fonts the last render loaded, kept while the document declares the
    /// same fonts and no asset has arrived since. Parsing font files is the
    /// slowest step that does not depend on the design, and an edit almost
    /// never changes the declarations, so an edit should not pay for it again.
    fonts: RefCell<Option<LoadedFonts>>,
    /// The package the markup comes from, once one is loaded. Without one,
    /// markup is a standalone document.
    package: RefCell<Option<PackageContext>>,
}

/// How a loaded package's documents are read.
struct PackageContext {
    /// `library.guix` as last loaded or edited, or why it did not parse.
    library: Option<Result<Library, String>>,
    /// A library or several documents: every document must declare 0.3.
    multi_document: bool,
}

struct LoadedFonts {
    /// The `<fonts>` declarations these were loaded for.
    declarations: String,
    store: Rc<FontStore>,
    warnings: Vec<String>,
}

impl Default for Engine {
    fn default() -> Self {
        Self {
            cache: in_memory_cache(BTreeMap::new()),
            fonts: RefCell::new(None),
            package: RefCell::new(None),
        }
    }
}

/// One render: the pixels, and the geometry a host needs to answer "what is
/// under this point".
#[wasm_bindgen]
pub struct Frame {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
    layout: String,
    warnings: Vec<String>,
}

#[wasm_bindgen]
impl Frame {
    #[wasm_bindgen(getter)]
    pub fn width(&self) -> u32 {
        self.width
    }

    #[wasm_bindgen(getter)]
    pub fn height(&self) -> u32 {
        self.height
    }

    /// Straight RGBA, row by row: the bytes of an `ImageData` of
    /// `width` x `height`, ready for `putImageData` with no PNG in between.
    #[wasm_bindgen(getter)]
    pub fn pixels(&self) -> Vec<u8> {
        self.pixels.clone()
    }

    /// The layout tree as JSON, with absolute rects and every node's attributes.
    #[wasm_bindgen(getter)]
    pub fn layout(&self) -> String {
        self.layout.clone()
    }

    /// Fonts that did not resolve, and what was used instead.
    #[wasm_bindgen(getter)]
    pub fn warnings(&self) -> Vec<String> {
        self.warnings.clone()
    }
}

#[wasm_bindgen]
impl Engine {
    #[wasm_bindgen(constructor)]
    pub fn new() -> Engine {
        Engine::default()
    }

    /// Holds `bytes` under `key`: a package path, a remote URL, or the path of
    /// a system font file the host listed.
    pub fn set_asset(&mut self, key: String, bytes: Vec<u8>) {
        self.cache.insert_package_asset(key, bytes);
        self.fonts.replace(None);
    }

    /// Reads a packaged `.gui`, keeps its assets and library, and returns its
    /// pages as JSON:
    ///
    /// ```json
    /// { "documents": [{ "name": "01-welcome.guix", "xml": "<gui …" }],
    ///   "library": { "name": "library.guix", "xml": "<gui …", "hasPage": true } }
    /// ```
    ///
    /// Documents are in presentation order. `library` is `null` when the
    /// package has none. A document that is not valid UTF-8 comes back with
    /// `xml: null` and an `error`, so the rest still open.
    pub fn load_package(&mut self, package: &[u8]) -> Result<String, JsValue> {
        let package = read_gui_package(package).map_err(to_js)?;
        let library = package.parse_library();

        let page = |document: &dotgui_renderer::PackageDocument| match document.xml() {
            Ok(xml) => serde_json::json!({ "name": document.name, "xml": xml }),
            Err(err) => {
                serde_json::json!({ "name": document.name, "xml": null, "error": err.to_string() })
            }
        };
        let mut library_json = package.library.as_ref().map(page);
        if let (Some(json), Some(Ok(parsed))) = (library_json.as_mut(), library.as_ref()) {
            json["hasPage"] = parsed.has_page().into();
        }
        let pages = serde_json::json!({
            "documents": package.documents.iter().map(page).collect::<Vec<_>>(),
            "library": library_json,
        });

        self.package.replace(Some(PackageContext {
            library: library.map(|result| result.map_err(|err| library_reason(&err))),
            multi_document: package.is_multi_document(),
        }));
        for (key, bytes) in package.assets {
            self.cache.insert_package_asset(key, bytes);
        }
        self.fonts.replace(None);
        serde_json::to_string(&pages).map_err(to_js)
    }

    /// The font files on the host, by path — what a native renderer would find
    /// in the host's font directories. `source="system"` families resolve
    /// against this list exactly as they would against those directories; the
    /// files' bytes arrive through [`Engine::set_asset`], as
    /// [`Engine::missing_font_files`] asks for them.
    pub fn set_host_font_files(&mut self, paths: Vec<String>) {
        let cache = std::mem::replace(&mut self.cache, AssetCache::new("."));
        self.cache = cache.with_host_font_files(paths.into_iter().map(Into::into).collect());
        self.fonts.replace(None);
    }

    /// The system font files this document needs whose bytes the engine does
    /// not hold yet. Like [`Engine::missing_font_urls`], ask again after supplying
    /// them: a family that is not installed falls back to the next UI font.
    pub fn missing_font_files(
        &self,
        xml: &str,
        library: Option<bool>,
    ) -> Result<Vec<String>, JsValue> {
        let document = self.read(xml, library)?;
        Ok(missing_system_font_files(&document, &self.cache))
    }

    /// The web font URLs this document needs that the engine does not hold
    /// yet: Google stylesheets, then the font files they name.
    ///
    /// Font files are only known once their stylesheet is held, so a host
    /// fetches what this returns and asks again until it comes back empty.
    /// Fonts decide where text breaks, so a host should wait for these before
    /// a first render, or the layout will jump when they arrive.
    pub fn missing_font_urls(
        &self,
        xml: &str,
        library: Option<bool>,
    ) -> Result<Vec<String>, JsValue> {
        let document = self.read(xml, library)?;
        let mut wanted = Vec::new();

        for stylesheet in google_stylesheet_urls(&document) {
            // Only a held stylesheet is read: resolving one that is not held
            // would try to fetch it, which this build cannot.
            if !self.cache.has_package_asset(&stylesheet) {
                wanted.push(stylesheet);
            } else if let Ok(css) = self.cache.resolve(&stylesheet) {
                wanted.extend(font_urls_in_stylesheet(&String::from_utf8_lossy(
                    &css.bytes,
                )));
            }
        }

        Ok(self.not_held(wanted))
    }

    /// The remote images this document needs that the engine does not hold
    /// yet: `src` values, `url(...)` fills, and tokens that name either.
    ///
    /// Images never move the layout — their boxes are sized by the markup — so
    /// a host can render first and fill these in when they arrive.
    pub fn missing_image_urls(
        &self,
        xml: &str,
        library: Option<bool>,
    ) -> Result<Vec<String>, JsValue> {
        let document = self.read(xml, library)?;
        let mut wanted = Vec::new();
        collect_remote_urls(&document.root, &mut wanted);
        wanted.extend(
            document
                .metadata
                .tokens
                .values()
                .flat_map(|value| remote_urls_in(value)),
        );
        Ok(self.not_held(wanted))
    }

    /// Renders at `density` device pixels per document pixel — pass the
    /// screen's `devicePixelRatio` for a sharp canvas. The layout stays in
    /// document pixels, so hit testing does not change with density.
    ///
    /// Pass `library: true` when `xml` is the package's `library.guix`: its
    /// page is drawn, and every later document resolves against this markup,
    /// so an edit to the library shows up in the documents that use it.
    pub fn render(&self, xml: &str, density: f32, library: Option<bool>) -> Result<Frame, JsValue> {
        let document = self.read(xml, library)?;
        let cache = &self.cache;
        let (fonts, mut warnings) = self.fonts_for(&document)?;
        warnings.splice(0..0, document.warnings.iter().cloned());
        let fonts: &FontStore = &fonts;
        let layout = compute_taffy_layout_with_text(&document, fonts).map_err(to_js)?;
        let mut scene = build_scene(&document, &layout);
        if density.is_finite() && density > 0.0 && density != 1.0 {
            scene = scene.scaled(density);
        }
        let (width, height, pixels) =
            paint_scene_to_rgba(&scene, Some(cache), Some(fonts)).map_err(to_js)?;
        warnings.dedup();
        Ok(Frame {
            width,
            height,
            pixels,
            layout: serde_json::to_string(&layout).map_err(to_js)?,
            warnings,
        })
    }
}

impl Engine {
    /// Parses markup as what it is: a package document resolved against the
    /// library, the library's own page, or a standalone document.
    ///
    /// Reading the library's page also takes its declarations as the
    /// package's library from now on.
    fn read(&self, xml: &str, library: Option<bool>) -> Result<GuiDocument, JsValue> {
        let mut package = self.package.borrow_mut();

        if library == Some(true) {
            let parsed = parse_library(xml).map_err(|err| library_reason(&err));
            let failure = parsed.as_ref().err().cloned();
            match package.as_mut() {
                Some(context) => context.library = Some(parsed),
                None => {
                    *package = Some(PackageContext {
                        library: Some(parsed),
                        multi_document: true,
                    })
                }
            }
            if let Some(reason) = failure {
                return Err(to_js(ParseError::Library(reason)));
            }
            return parse_gui_xml_with(
                xml,
                ParseOptions {
                    in_multi_document_package: true,
                    ..ParseOptions::default()
                },
            )
            .map_err(to_js);
        }

        let Some(context) = package.as_ref() else {
            return parse_standalone_xml(xml).map_err(to_js);
        };
        let library = match &context.library {
            Some(Ok(library)) => Some(library),
            Some(Err(reason)) => return Err(to_js(ParseError::Library(reason.clone()))),
            None => None,
        };
        parse_gui_xml_with(
            xml,
            ParseOptions {
                library,
                in_multi_document_package: context.multi_document,
                ..ParseOptions::default()
            },
        )
        .map_err(to_js)
    }

    fn not_held(&self, mut urls: Vec<String>) -> Vec<String> {
        urls.retain(|url| !self.cache.has_package_asset(url));
        urls.sort();
        urls.dedup();
        urls
    }

    /// The document's fonts: the last render's, when its declarations match.
    fn fonts_for(&self, document: &GuiDocument) -> Result<(Rc<FontStore>, Vec<String>), JsValue> {
        let declarations = serde_json::to_string(&document.metadata.fonts).map_err(to_js)?;
        if let Some(loaded) = self.fonts.borrow().as_ref() {
            if loaded.declarations == declarations {
                return Ok((Rc::clone(&loaded.store), loaded.warnings.clone()));
            }
        }

        // A font the host could not fetch is a missing font, not a failed
        // render: draw without it and say so.
        let (store, warnings) = match FontStore::from_document(document, &self.cache) {
            Ok(store) => {
                let warnings = store.warnings().to_vec();
                (store, warnings)
            }
            Err(err) => (FontStore::default(), vec![err.to_string()]),
        };
        let store = Rc::new(store);
        self.fonts.replace(Some(LoadedFonts {
            declarations,
            store: Rc::clone(&store),
            warnings: warnings.clone(),
        }));
        Ok((store, warnings))
    }
}

/// What went wrong with a library, without repeating that it was the library.
fn library_reason(err: &ParseError) -> String {
    match err {
        ParseError::Library(reason) => reason.clone(),
        other => other.to_string(),
    }
}

fn collect_remote_urls(node: &GuiNode, into: &mut Vec<String>) {
    for value in node.attributes.values() {
        into.extend(remote_urls_in(value));
    }
    for child in &node.children {
        collect_remote_urls(child, into);
    }
}

/// Every `http(s)://` URL in an attribute value: a bare `src`, or one inside a
/// `url(...)` fill.
fn remote_urls_in(value: &str) -> Vec<String> {
    let mut urls = Vec::new();
    let mut rest = value;
    while let Some(start) = [rest.find("http://"), rest.find("https://")]
        .into_iter()
        .flatten()
        .min()
    {
        let tail = &rest[start..];
        let end = tail
            .find(|c: char| c.is_whitespace() || matches!(c, ')' | '"' | '\''))
            .unwrap_or(tail.len());
        urls.push(tail[..end].to_owned());
        rest = &tail[end..];
    }
    urls
}

/// This renderer's version, e.g. `0.3.0`. Its `major.minor` is the newest spec
/// version it reads.
#[wasm_bindgen]
pub fn renderer_version() -> String {
    RENDERER_VERSION.to_owned()
}

/// The newest spec version this renderer reads, e.g. `0.3`. A document
/// declaring a newer one is refused.
#[wasm_bindgen]
pub fn supported_spec_version() -> String {
    SUPPORTED_VERSION.to_owned()
}

/// The markup with presence attributes (`<frame clip>`) given values, so a
/// strict XML parser — the browser's `DOMParser` — accepts what `.gui` allows.
#[wasm_bindgen]
pub fn normalize_markup(xml: &str) -> String {
    normalize_presence_attrs(xml)
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

    #[test]
    fn engine_asks_for_a_stylesheet_then_the_fonts_it_names() {
        let xml = r##"
        <gui version="0.2">
          <fonts><font family="Inter" source="google" weights="400" styles="normal" /></fonts>
          <col w="100" h="100" fill="url(https://example.com/bg.png)">
            <img src="http://example.com/a.png" w="10" h="10" />
          </col>
        </gui>
        "##;
        let mut engine = Engine::new();

        let first = engine.missing_font_urls(xml, None).unwrap();
        let stylesheet = first
            .iter()
            .find(|url| url.starts_with("https://fonts.googleapis.com/"))
            .expect("the Google stylesheet is asked for")
            .clone();
        assert_eq!(first.len(), 1, "images are not fonts: {first:?}");
        assert_eq!(
            engine.missing_image_urls(xml, None).unwrap(),
            vec![
                "http://example.com/a.png".to_owned(),
                "https://example.com/bg.png".to_owned()
            ]
        );

        let css = "@font-face { font-style: normal; font-weight: 400; src: url(https://fonts.gstatic.com/inter.ttf) format('truetype'); }";
        engine.set_asset(stylesheet, css.as_bytes().to_vec());
        let second = engine.missing_font_urls(xml, None).unwrap();
        assert!(second.contains(&"https://fonts.gstatic.com/inter.ttf".to_owned()));
        assert!(!second
            .iter()
            .any(|url| url.starts_with("https://fonts.googleapis.com/")));
    }

    #[test]
    fn engine_frame_carries_attributes_a_host_can_select_by() {
        let xml = r##"<gui version="0.2"><col w="100" h="100"><rect data-uid="7" w="50" h="50" fill="#ff0000" /></col></gui>"##;
        let frame = Engine::new().render(xml, 1.0, None).unwrap();
        assert_eq!((frame.width(), frame.height()), (100, 100));
        assert_eq!(frame.pixels().len(), 100 * 100 * 4);
        // The red square's first pixel, straight RGBA.
        assert_eq!(&frame.pixels()[..4], &[255, 0, 0, 255]);
        assert!(frame.layout().contains(r#""data-uid":"7""#));

        // Twice the pixels, the same layout.
        let dense = Engine::new().render(xml, 2.0, None).unwrap();
        assert_eq!((dense.width(), dense.height()), (200, 200));
        assert_eq!(dense.layout(), frame.layout());
    }

    #[test]
    fn engine_reloads_fonts_only_when_the_declarations_change() {
        let engine = Engine::new();
        // `unresolved` fonts are never fetched, so this stays off the network.
        let with = |family: &str| {
            format!(
                r#"<gui version="0.2"><fonts><font family="{family}" source="unresolved" weights="400" styles="normal" /></fonts><col w="10" h="10" /></gui>"#
            )
        };
        let first = parse_gui_xml(&with("Inter")).unwrap();
        let (a, _) = engine.fonts_for(&first).unwrap();
        let (b, _) = engine.fonts_for(&first).unwrap();
        assert!(
            Rc::ptr_eq(&a, &b),
            "same declarations reuse the loaded fonts"
        );

        let second = parse_gui_xml(&with("Roboto")).unwrap();
        let (c, _) = engine.fonts_for(&second).unwrap();
        assert!(!Rc::ptr_eq(&a, &c), "new declarations load again");
    }

    fn zip(entries: &[(&str, &str)]) -> Vec<u8> {
        use std::io::Write;
        let mut bytes = std::io::Cursor::new(Vec::new());
        {
            let mut zip = zip::ZipWriter::new(&mut bytes);
            for (name, contents) in entries {
                zip.start_file(*name, zip::write::SimpleFileOptions::default())
                    .unwrap();
                zip.write_all(contents.as_bytes()).unwrap();
            }
            zip.finish().unwrap();
        }
        bytes.into_inner()
    }

    const LIBRARY: &str = r##"<gui version="0.3">
      <tokens><color name="brand" value="#ff0000" /></tokens>
      <col w="20" h="20" fill="$brand" />
    </gui>"##;
    const SCREEN: &str = r##"<gui version="0.3"><col w="10" h="10" fill="$brand" /></gui>"##;

    #[test]
    fn engine_renders_every_page_of_a_multi_document_package() {
        let mut engine = Engine::new();
        let pages = engine
            .load_package(&zip(&[
                ("02-b.guix", SCREEN),
                ("library.guix", LIBRARY),
                ("01-a.guix", SCREEN),
            ]))
            .unwrap();
        let pages: serde_json::Value = serde_json::from_str(&pages).unwrap();
        assert_eq!(pages["documents"][0]["name"], "01-a.guix");
        assert_eq!(pages["documents"][1]["name"], "02-b.guix");
        assert_eq!(pages["library"]["hasPage"], true);

        // A document resolves the library's token without declaring it.
        let frame = engine.render(SCREEN, 1.0, None).unwrap();
        assert_eq!(&frame.pixels()[..4], &[255, 0, 0, 255]);

        // The library's own page draws too.
        let library = pages["library"]["xml"].as_str().unwrap();
        let page = engine.render(library, 1.0, Some(true)).unwrap();
        assert_eq!((page.width(), page.height()), (20, 20));

        // Editing the library reaches the documents that use it.
        let edited = LIBRARY.replace("#ff0000", "#0000ff");
        engine.render(&edited, 1.0, Some(true)).unwrap();
        let frame = engine.render(SCREEN, 1.0, None).unwrap();
        assert_eq!(&frame.pixels()[..4], &[0, 0, 255, 255]);
    }
}
