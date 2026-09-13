use crate::components::{self, Components, Scope};
use crate::issues::Issues;
use crate::validate;
use crate::{FontInfo, GuiDocument, GuiMetadata, GuiNode};
use roxmltree::{Document, Node};
use std::collections::BTreeMap;
use thiserror::Error;

const ROOT_LAYOUT_TAGS: &[&str] = &["frame", "stack", "row", "col", "grid"];
const PRESENCE_ATTRS: &[(&str, &str)] = &[
    ("abs", "true"),
    ("clip", "true"),
    ("gap", "auto"),
    // The spec writes `isolation` as presence-only. CSS spells the value
    // `isolate`, and that is what a document carrying one writes too.
    ("isolation", "isolate"),
    ("mask", "true"),
    ("reverse-z", "true"),
    ("truncate", "true"),
    ("wrap", "true"),
];

/// This renderer's own version. Its `major.minor` is the newest spec version
/// it implements, and a patch release fixes the renderer without changing the
/// spec it reads: renderer 0.3.2 reads the same documents as 0.3.0.
pub const RENDERER_VERSION: &str = env!("CARGO_PKG_VERSION");

/// The newest spec version this renderer implements. Support is cumulative:
/// every earlier version is implemented too, and a document declaring a newer
/// one is refused rather than drawn with features this renderer does not know.
pub const SUPPORTED_VERSION: &str = "0.3";
const SUPPORTED: (u32, u32) = (0, 3);
const MULTI_DOCUMENT: (u32, u32) = (0, 3);

#[derive(Debug, Error)]
pub enum ParseError {
    #[error("invalid XML: {0}")]
    Xml(#[from] roxmltree::Error),

    #[error("expected <gui> as the document root, found <{0}>")]
    WrongRoot(String),

    #[error("missing renderer root layout node")]
    MissingRootLayout,

    #[error("multiple root layout nodes found; expected exactly one")]
    MultipleRootLayouts,

    #[error(
        "the document declares version {0}, but this renderer ({RENDERER_VERSION}) reads documents up to version {SUPPORTED_VERSION}; a newer renderer is needed"
    )]
    UnsupportedVersion(String),

    #[error("the document declares version {0}, but a document in a package with a library or several documents must declare 0.3 or higher")]
    VersionBelowPackage(String),

    #[error("{0} is not valid UTF-8")]
    Encoding(String),

    #[error("library.guix is invalid: {0}")]
    Library(String),

    #[error("{}", .0.join("; "))]
    Invalid(Vec<String>),
}

/// Which rules a document is read under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocumentForm {
    /// Inside a package, or handed over by a host that holds its assets.
    Packaged,
    /// Bare markup served on its own (RFC-0043): in 0.3, its assets must be
    /// absolute URLs.
    Standalone,
}

#[derive(Debug, Clone, Copy)]
pub struct ParseOptions<'a> {
    pub form: DocumentForm,
    /// The package's `library.guix`, whose declarations the document may use.
    pub library: Option<&'a Library>,
    /// The document sits in a package with a library or several documents, so
    /// it must declare 0.3 or higher.
    pub in_multi_document_package: bool,
}

impl Default for ParseOptions<'_> {
    fn default() -> Self {
        Self {
            form: DocumentForm::Packaged,
            library: None,
            in_multi_document_package: false,
        }
    }
}

/// A package's `library.guix`: the declarations every document in the
/// package shares, and — when it has a layout root — a page of its own.
///
/// Everything above its layout root is shared; the layout root and what is
/// under it is local to the library. That page is the package's style guide
/// or documentation, and renders like any other document.
#[derive(Debug, Clone)]
pub struct Library {
    metadata: GuiMetadata,
    components: Components,
    has_page: bool,
    xml: String,
}

impl Library {
    /// The tokens, fonts and styles the library shares.
    pub fn metadata(&self) -> &GuiMetadata {
        &self.metadata
    }

    /// Whether the library has a layout root to render.
    pub fn has_page(&self) -> bool {
        self.has_page
    }

    /// The library's own page, resolved against its own declarations.
    pub fn page(&self) -> Result<GuiDocument, ParseError> {
        if !self.has_page {
            return Err(ParseError::MissingRootLayout);
        }
        parse_gui_xml_with(
            &self.xml,
            ParseOptions {
                in_multi_document_package: true,
                ..ParseOptions::default()
            },
        )
    }
}

/// Parses a document as a packaged one with no library: the form every 0.2
/// document has, and the form a host that holds a document's assets renders.
pub fn parse_gui_xml(xml: &str) -> Result<GuiDocument, ParseError> {
    parse_gui_xml_with(xml, ParseOptions::default())
}

/// Parses bare markup served on its own (RFC-0043).
pub fn parse_standalone_xml(xml: &str) -> Result<GuiDocument, ParseError> {
    parse_gui_xml_with(
        xml,
        ParseOptions {
            form: DocumentForm::Standalone,
            ..ParseOptions::default()
        },
    )
}

pub fn parse_gui_xml_with(xml: &str, options: ParseOptions) -> Result<GuiDocument, ParseError> {
    let parts = read_parts(
        xml,
        options.in_multi_document_package || options.library.is_some(),
    )?;
    let mut issues = Issues::new(parts.strict);
    let mut metadata = parts.metadata;

    if let Some(library) = options.library {
        metadata = merge_library(&library.metadata, metadata, &mut issues);
    }
    let mut layout_root = parts.layout_root.ok_or(ParseError::MissingRootLayout)?;

    // Instances are expanded here so nothing downstream ever sees one: layout,
    // the scene and painting work on the tree the document would have had if
    // it were written out longhand. This runs even when the document declares
    // no components, so an instance naming one that does not exist is dropped
    // rather than laid out as an unknown block.
    let local = components::read_components(&parts.component_blocks, &mut issues);
    if let Some(library) = options.library {
        for id in local.ids() {
            if library.components.contains(id) {
                issues.error(redeclared("component", id));
            }
        }
    }
    components::expand(
        &mut layout_root,
        Scope {
            local: &local,
            library: options.library.map(|library| &library.components),
        },
        &mut issues,
    );

    // A 0.2 document keeps its old leniency about what it refers to: it
    // renders as it always did, and nothing new is said about it.
    if parts.strict {
        validate::check_references(&layout_root, &metadata, &mut issues);
        validate::check_assets(
            &layout_root,
            &metadata,
            options.form == DocumentForm::Standalone,
            &mut issues,
        );
    }

    if !issues.errors.is_empty() {
        return Err(ParseError::Invalid(issues.errors));
    }

    Ok(GuiDocument {
        version: parts.version,
        name: parts.name,
        metadata,
        root: layout_root,
        warnings: issues.warnings,
    })
}

/// Parses a package's `library.guix`.
///
/// A library may have no layout root: then it is declarations only. What it
/// declares must resolve inside it, because the library depends on nothing.
pub fn parse_library(xml: &str) -> Result<Library, ParseError> {
    let parts = read_parts(xml, true)?;
    let mut issues = Issues::new(true);
    let components = components::read_components(&parts.component_blocks, &mut issues);
    validate::check_styles(&parts.metadata, &mut issues);

    for (id, body) in components.bodies() {
        let mut unexpanded = Issues::new(true);
        validate::check_tree(body, &parts.metadata, &mut unexpanded);
        check_instances_resolve(body, &components, &mut unexpanded);
        issues.errors.extend(
            unexpanded
                .errors
                .into_iter()
                .map(|e| format!("component '{id}': {e}")),
        );
    }

    if !issues.errors.is_empty() {
        return Err(ParseError::Library(issues.errors.join("; ")));
    }

    Ok(Library {
        metadata: parts.metadata,
        components,
        has_page: parts.layout_root.is_some(),
        xml: xml.to_owned(),
    })
}

/// Reports instances in a library component's body that name a component the
/// library does not declare.
fn check_instances_resolve(node: &GuiNode, components: &Components, issues: &mut Issues) {
    if node.tag == "instance" {
        if let Some(id) = node.attributes.get("component") {
            if !components.contains(id) {
                issues.violation(format!(
                    "<instance> names component '{id}', which the library does not declare"
                ));
            }
        }
    }
    for child in &node.children {
        check_instances_resolve(child, components, issues);
    }
}

/// A document's declarations with the library's added. A document may add
/// names of its own, but redeclaring one the library holds is an error, not
/// an override (RFC-0042).
fn merge_library(library: &GuiMetadata, local: GuiMetadata, issues: &mut Issues) -> GuiMetadata {
    let mut merged = library.clone();

    for (name, value) in local.tokens {
        if merged.tokens.insert(name.clone(), value).is_some() {
            issues.error(redeclared("token", &format!("${name}")));
        }
    }
    for (family, font) in local.fonts {
        if merged.fonts.insert(family.clone(), font).is_some() {
            issues.error(redeclared("font", &family));
        }
    }
    for (name, style) in local.styles {
        if merged.styles.insert(name.clone(), style).is_some() {
            issues.error(redeclared("text style", &name));
        }
    }
    for (name, effects) in local.effect_styles {
        if merged.effect_styles.insert(name.clone(), effects).is_some() {
            issues.error(redeclared("effect style", &name));
        }
    }

    merged
}

fn redeclared(kind: &str, name: &str) -> String {
    format!("{kind} '{name}' is already declared by library.guix; change it there instead of redeclaring it")
}

/// A document split at its layout root: declarations above, the screen below.
struct Parts {
    version: String,
    /// Whether the document declares 0.3 or higher.
    strict: bool,
    name: Option<String>,
    metadata: GuiMetadata,
    component_blocks: Vec<GuiNode>,
    layout_root: Option<GuiNode>,
}

fn read_parts(xml: &str, requires_multi_document: bool) -> Result<Parts, ParseError> {
    let normalized = normalize_presence_attrs(xml);
    let doc = Document::parse(&normalized)?;
    let root = doc.root_element();
    if root.tag_name().name() != "gui" {
        return Err(ParseError::WrongRoot(root.tag_name().name().to_owned()));
    }

    let version = attr(root, "version").unwrap_or_else(|| "0.2".to_owned());
    let parsed =
        parse_version(&version).ok_or_else(|| ParseError::UnsupportedVersion(version.clone()))?;
    if parsed > SUPPORTED {
        return Err(ParseError::UnsupportedVersion(version));
    }
    if requires_multi_document && parsed < MULTI_DOCUMENT {
        return Err(ParseError::VersionBelowPackage(version));
    }

    let mut metadata = GuiMetadata::default();
    let mut layout_root: Option<GuiNode> = None;
    let mut component_blocks = Vec::new();

    for child in root.children().filter(Node::is_element) {
        let tag = child.tag_name().name();
        match tag {
            "tokens" => read_tokens(child, &mut metadata),
            "fonts" => read_fonts(child, &mut metadata),
            "styles" => read_styles(child, &mut metadata),
            "components" => component_blocks.push(read_node(child)),
            t if ROOT_LAYOUT_TAGS.contains(&t) => {
                if layout_root.is_some() {
                    return Err(ParseError::MultipleRootLayouts);
                }
                layout_root = Some(read_node(child));
            }
            _ => {}
        }
    }

    Ok(Parts {
        strict: parsed >= MULTI_DOCUMENT,
        version,
        name: attr(root, "name"),
        metadata,
        component_blocks,
        layout_root,
    })
}

/// `major.minor`, ignoring any patch part.
fn parse_version(version: &str) -> Option<(u32, u32)> {
    let mut parts = version.trim().split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next().map_or(Some(0), |minor| minor.parse().ok())?;
    Some((major, minor))
}

/// Gives presence attributes a value — `<frame clip>` becomes
/// `<frame clip="true">` — so the markup is well-formed XML. `.gui` allows the
/// bare form; XML parsers, this crate's and a browser's, do not.
pub fn normalize_presence_attrs(xml: &str) -> String {
    let mut out = String::with_capacity(xml.len());
    let mut cursor = 0;

    while let Some(tag_start_offset) = xml[cursor..].find('<') {
        let tag_start = cursor + tag_start_offset;
        out.push_str(&xml[cursor..tag_start]);

        if xml[tag_start..].starts_with("<!--") {
            let end = xml[tag_start + 4..]
                .find("-->")
                .map(|offset| tag_start + 4 + offset + 3)
                .unwrap_or(xml.len());
            out.push_str(&xml[tag_start..end]);
            cursor = end;
            continue;
        }

        let Some(tag_end) = find_tag_end(xml, tag_start) else {
            out.push_str(&xml[tag_start..]);
            return out;
        };

        let tag = &xml[tag_start..tag_end];
        if tag.starts_with("</") || tag.starts_with("<?") || tag.starts_with("<!") {
            out.push_str(tag);
        } else {
            out.push_str(&normalize_tag_presence_attrs(tag));
        }
        cursor = tag_end;
    }

    out.push_str(&xml[cursor..]);
    out
}

fn find_tag_end(xml: &str, tag_start: usize) -> Option<usize> {
    let mut quote = None;
    for (offset, ch) in xml[tag_start..].char_indices() {
        match quote {
            Some(q) if ch == q => quote = None,
            Some(_) => {}
            None if ch == '"' || ch == '\'' => quote = Some(ch),
            None if ch == '>' => return Some(tag_start + offset + 1),
            None => {}
        }
    }
    None
}

fn normalize_tag_presence_attrs(tag: &str) -> String {
    let mut out = String::with_capacity(tag.len());
    let chars: Vec<char> = tag.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        let ch = chars[i];
        if ch == '"' || ch == '\'' {
            let quote = ch;
            out.push(ch);
            i += 1;
            while i < chars.len() {
                let inner = chars[i];
                out.push(inner);
                i += 1;
                if inner == quote {
                    break;
                }
            }
            continue;
        }

        if !ch.is_whitespace() {
            out.push(ch);
            i += 1;
            continue;
        }

        while i < chars.len() && chars[i].is_whitespace() {
            out.push(chars[i]);
            i += 1;
        }

        let name_start = i;
        if !chars.get(i).is_some_and(|c| c.is_ascii_alphabetic()) {
            continue;
        }

        while i < chars.len()
            && (chars[i].is_ascii_alphanumeric() || chars[i] == '-' || chars[i] == '_')
        {
            i += 1;
        }

        let name: String = chars[name_start..i].iter().collect();
        let value = presence_value(&name);
        let after = chars.get(i).copied();
        if let Some(value) = value {
            if after.is_none() || after.is_some_and(|c| c.is_whitespace() || c == '/' || c == '>') {
                out.push_str(&name);
                out.push_str("=\"");
                out.push_str(value);
                out.push('"');
                continue;
            }
        }

        out.push_str(&name);
    }

    out
}

fn presence_value(name: &str) -> Option<&'static str> {
    PRESENCE_ATTRS
        .iter()
        .find_map(|(attr, value)| (*attr == name).then_some(*value))
}

fn read_tokens(tokens_el: Node, metadata: &mut GuiMetadata) {
    for child in tokens_el.children().filter(Node::is_element) {
        let Some(name) = attr(child, "name").or_else(|| attr(child, "id")) else {
            continue;
        };
        let value = attr(child, "value").or_else(|| {
            child
                .text()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
        });
        if let Some(value) = value {
            metadata.tokens.insert(name, value);
        }
    }
}

/// Splits a `<styles>` block into text styles and effect styles.
///
/// A `<text-style>` is read as a bag of attributes. An `<effect-style>` holds
/// ordered `<effect>` children instead, so it is collected separately.
fn read_styles(styles_el: Node, metadata: &mut GuiMetadata) {
    for child in styles_el.children().filter(Node::is_element) {
        let Some(name) = attr(child, "name").or_else(|| attr(child, "id")) else {
            continue;
        };

        if child.tag_name().name() == "effect-style" {
            metadata.effect_styles.insert(
                name,
                child
                    .children()
                    .filter(Node::is_element)
                    .filter(|effect| effect.tag_name().name() == "effect")
                    .map(read_attributes)
                    .collect(),
            );
        } else {
            metadata.styles.insert(name, read_attributes(child));
        }
    }
}

fn read_fonts(fonts_el: Node, metadata: &mut GuiMetadata) {
    for child in fonts_el.children().filter(Node::is_element) {
        if child.tag_name().name() != "font" {
            continue;
        }

        let Some(family) = attr(child, "family") else {
            continue;
        };
        let Some(source) = attr(child, "source") else {
            continue;
        };

        metadata.fonts.insert(
            family,
            FontInfo {
                source,
                category: attr(child, "category"),
                weights: attr(child, "weights"),
                styles: attr(child, "styles"),
                variants: attr(child, "variants"),
            },
        );
    }
}

fn read_node(node: Node) -> GuiNode {
    let mut gui_node = GuiNode::new(node.tag_name().name());
    gui_node.attributes = read_attributes(node);

    let text = node
        .children()
        .filter(Node::is_text)
        .filter_map(|n| n.text())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("");
    if !text.is_empty() {
        gui_node.text = Some(text);
    }

    gui_node.children = node
        .children()
        .filter(Node::is_element)
        .map(read_node)
        .collect();

    gui_node
}

fn read_attributes(node: Node) -> BTreeMap<String, String> {
    node.attributes()
        .map(|attr| (attr.name().to_owned(), attr.value().to_owned()))
        .collect()
}

fn attr(node: Node, name: &str) -> Option<String> {
    node.attribute(name).map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal_gui_document() {
        let xml = r##"
          <gui version="0.2" name="Smoke">
            <tokens>
              <color name="surface" value="#ffffff" />
              <token name="space.4" value="16" />
            </tokens>
            <fonts>
              <font family="Inter" source="google" weights="400 700" styles="normal" />
            </fonts>
            <styles>
              <text-style name="title" font="Inter" size="24" />
            </styles>
            <col w="390" p="24" gap="12" fill="$surface">
              <text value="Hello GUI" style="title" />
              <rect w="120" h="44" radius="8" fill="#0d99ff" />
            </col>
          </gui>
        "##;

        let parsed = parse_gui_xml(xml).expect("valid gui");

        assert_eq!(parsed.version, "0.2");
        assert_eq!(parsed.name.as_deref(), Some("Smoke"));
        assert_eq!(parsed.metadata.tokens["surface"], "#ffffff");
        assert_eq!(parsed.metadata.tokens["space.4"], "16");
        assert_eq!(parsed.metadata.fonts["Inter"].source, "google");
        assert_eq!(
            parsed.metadata.fonts["Inter"].weights.as_deref(),
            Some("400 700")
        );
        assert_eq!(parsed.metadata.styles["title"]["size"], "24");
        assert_eq!(parsed.root.tag, "col");
        assert_eq!(parsed.root.children.len(), 2);
        assert_eq!(parsed.root.children[0].tag, "text");
        assert_eq!(parsed.root.children[0].attributes["value"], "Hello GUI");
    }

    #[test]
    fn rejects_documents_without_a_layout_root() {
        let err = parse_gui_xml(r#"<gui version="0.2"><tokens /></gui>"#).unwrap_err();
        assert!(matches!(err, ParseError::MissingRootLayout));
    }

    #[test]
    fn normalizes_presence_attributes_before_xml_parse() {
        let xml = r#"
          <gui version="0.2" name="Presence">
            <frame w="320" h="240" clip>
              <col abs x="0" y="0" wrap />
            </frame>
          </gui>
        "#;

        let parsed = parse_gui_xml(xml).expect("presence attrs should parse");

        assert_eq!(parsed.root.attributes["clip"], "true");
        assert_eq!(parsed.root.children[0].attributes["abs"], "true");
        assert_eq!(parsed.root.children[0].attributes["wrap"], "true");
    }

    #[test]
    fn parses_fonts_by_family_and_source() {
        let xml = r#"
          <gui version="0.2" name="Fonts">
            <fonts>
              <font family="Roboto" source="google" category="sans-serif" weights="400 500 700" styles="normal italic" />
              <font family="SF Pro" source="system" weights="400 600" styles="normal" />
            </fonts>
            <col w="390" />
          </gui>
        "#;

        let parsed = parse_gui_xml(xml).expect("valid gui");

        assert_eq!(parsed.metadata.fonts["Roboto"].source, "google");
        assert_eq!(
            parsed.metadata.fonts["Roboto"].category.as_deref(),
            Some("sans-serif")
        );
        assert_eq!(
            parsed.metadata.fonts["Roboto"].weights.as_deref(),
            Some("400 500 700")
        );
        assert_eq!(parsed.metadata.fonts["SF Pro"].source, "system");
    }

    #[test]
    fn a_version_newer_than_the_renderer_is_refused() {
        let err = parse_gui_xml(r#"<gui version="0.4"><col /></gui>"#).unwrap_err();
        assert!(matches!(err, ParseError::UnsupportedVersion(v) if v == "0.4"));
        assert!(matches!(
            parse_gui_xml(r#"<gui version="1.0"><col /></gui>"#),
            Err(ParseError::UnsupportedVersion(_))
        ));
        assert!(matches!(
            parse_gui_xml(r#"<gui version="soon"><col /></gui>"#),
            Err(ParseError::UnsupportedVersion(_))
        ));
        assert!(parse_gui_xml(r#"<gui version="0.3"><col /></gui>"#).is_ok());
        assert!(parse_gui_xml(r#"<gui version="0.1"><col /></gui>"#).is_ok());
        assert!(
            parse_gui_xml(r#"<gui><col /></gui>"#).is_ok(),
            "no version reads as 0.2"
        );
    }

    #[test]
    fn a_data_uri_is_an_error_in_0_3_at_any_size() {
        let err = parse_gui_xml(
            r#"<gui version="0.3"><col><img src="data:image/png;base64,AA==" w="1" h="1" /></col></gui>"#,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("data: URI"), "{err}");

        let err = parse_gui_xml(
            r#"<gui version="0.3"><tokens><token name="hero" value="data:image/png;base64,AA==" /></tokens><col /></gui>"#,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("token 'hero' holds a data: URI"), "{err}");

        // Unchanged for a document written before the rule.
        assert!(parse_gui_xml(
            r#"<gui version="0.2"><col><img src="data:image/png;base64,AA==" w="1" h="1" /></col></gui>"#,
        )
        .is_ok());
    }

    #[test]
    fn a_standalone_0_3_document_references_assets_only_by_absolute_url() {
        let local =
            r#"<gui version="0.3"><col><img src="assets/hero.webp" w="1" h="1" /></col></gui>"#;
        let err = parse_standalone_xml(local).unwrap_err().to_string();
        assert!(err.contains("local asset path"), "{err}");

        // The same markup inside a package is fine: the path is the package's.
        assert!(parse_gui_xml(local).is_ok());

        let remote = r#"<gui version="0.3">
          <tokens><token name="hero" value="https://example.com/hero.webp" /></tokens>
          <col><img src="$hero" w="1" h="1" /><frame mask-src="https://example.com/m.svg" /></col>
        </gui>"#;
        assert!(parse_standalone_xml(remote).is_ok());

        // Bare 0.2 markup predates the standalone form and keeps working.
        assert!(parse_standalone_xml(
            r#"<gui version="0.2"><col><img src="assets/hero.webp" w="1" h="1" /></col></gui>"#
        )
        .is_ok());
    }

    #[test]
    fn the_renderer_version_names_the_spec_it_implements() {
        // Bumping the crate to a new minor without implementing that spec, or
        // implementing a spec without bumping the crate, fails here.
        assert!(
            RENDERER_VERSION.starts_with(&format!("{SUPPORTED_VERSION}.")),
            "renderer {RENDERER_VERSION} should be a {SUPPORTED_VERSION}.x release"
        );
        assert_eq!(parse_version(SUPPORTED_VERSION), Some(SUPPORTED));
    }
}
