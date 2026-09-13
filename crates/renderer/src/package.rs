//! Reading a `.gui` package: the documents at its root, its library, and its
//! assets.
//!
//! RFC-0042 lets a package carry one document or many. There is no fixed name
//! and no manifest: every `.guix` at the root is a document, whatever it is
//! called, except `library.guix`, which holds the package's shared
//! declarations. Order is the lexical sort of the filenames.

use crate::parser::{parse_gui_xml_with, parse_library, Library, ParseError, ParseOptions};
use crate::GuiDocument;
use std::{
    collections::BTreeMap,
    io::{Cursor, Read},
};
use thiserror::Error;
use zip::ZipArchive;

/// The one reserved filename: the package's shared declarations, never a
/// document.
pub const LIBRARY_FILENAME: &str = "library.guix";

#[derive(Debug, Error)]
pub enum PackageError {
    #[error("invalid .gui package: {0}")]
    Zip(#[from] zip::result::ZipError),

    #[error("failed to read package entry: {0}")]
    Read(#[from] std::io::Error),

    #[error(".gui package contains no .guix document")]
    NoDocuments,

    #[error("{0} is not valid UTF-8")]
    Utf8(String),

    #[error("expected one document, but the package holds {}: {}", .0.len(), .0.join(", "))]
    NotSingleDocument(Vec<String>),

    #[error("input is neither a .gui package nor .guix markup")]
    UnknownInput,
}

#[derive(Debug, Clone)]
pub struct GuiPackage {
    /// Every document at the package root, in presentation order. The
    /// library is not among them.
    pub documents: Vec<PackageDocument>,
    /// `library.guix`, when the package has one.
    pub library: Option<PackageDocument>,
    /// Everything that is not a root `.guix`: `assets/`, `preview.webp`.
    pub assets: BTreeMap<String, Vec<u8>>,
}

/// One `.guix` file in a package, as bytes.
///
/// The bytes are checked for UTF-8 only when read, so one unreadable document
/// fails on its own rather than taking the package with it.
#[derive(Debug, Clone)]
pub struct PackageDocument {
    /// The filename, which is the document's address.
    pub name: String,
    pub bytes: Vec<u8>,
}

impl PackageDocument {
    pub fn xml(&self) -> Result<&str, PackageError> {
        std::str::from_utf8(&self.bytes).map_err(|_| PackageError::Utf8(self.name.clone()))
    }
}

impl GuiPackage {
    /// Whether the package is 0.3 structure: more than one document, or a
    /// library. Every document in such a package must declare 0.3 or higher.
    pub fn is_multi_document(&self) -> bool {
        self.documents.len() > 1 || self.library.is_some()
    }

    /// The package's only document, for callers that handle one screen.
    pub fn single_document(&self) -> Result<&PackageDocument, PackageError> {
        match self.documents.as_slice() {
            [only] => Ok(only),
            _ => Err(PackageError::NotSingleDocument(
                self.documents.iter().map(|doc| doc.name.clone()).collect(),
            )),
        }
    }

    pub fn document(&self, name: &str) -> Option<&PackageDocument> {
        self.documents.iter().find(|doc| doc.name == name)
    }

    /// The package's `library.guix`, parsed.
    pub fn parse_library(&self) -> Option<Result<Library, ParseError>> {
        let library = self.library.as_ref()?;
        Some(match library.xml() {
            Ok(xml) => parse_library(xml),
            Err(_) => Err(ParseError::Encoding(library.name.clone())),
        })
    }

    /// Parses one document against the package's library.
    pub fn parse_document(
        &self,
        document: &PackageDocument,
        library: Option<&Result<Library, ParseError>>,
    ) -> Result<GuiDocument, ParseError> {
        let library = match library {
            Some(Ok(library)) => Some(library),
            // A document cannot be read against a library that did not parse:
            // any name it leaves undeclared may be the library's.
            Some(Err(err)) => return Err(ParseError::Library(library_reason(err))),
            None => None,
        };
        let xml = document
            .xml()
            .map_err(|_| ParseError::Encoding(document.name.clone()))?;
        parse_gui_xml_with(
            xml,
            ParseOptions {
                library,
                in_multi_document_package: self.is_multi_document(),
                ..ParseOptions::default()
            },
        )
    }

    /// Every page the package renders: its documents in order, then the
    /// library's own page when it has one.
    ///
    /// Each page parses on its own, so one broken document is reported by
    /// name while the rest still render.
    pub fn pages(&self) -> Vec<PackagePage> {
        let library = self.parse_library();

        let mut pages: Vec<PackagePage> = self
            .documents
            .iter()
            .map(|document| PackagePage {
                name: document.name.clone(),
                is_library: false,
                document: self.parse_document(document, library.as_ref()),
            })
            .collect();

        match (&self.library, library) {
            (Some(file), Some(Ok(library))) if library.has_page() => pages.push(PackagePage {
                name: file.name.clone(),
                is_library: true,
                document: library.page(),
            }),
            (Some(file), Some(Err(err))) => pages.push(PackagePage {
                name: file.name.clone(),
                is_library: true,
                document: Err(err),
            }),
            _ => {}
        }

        pages
    }
}

/// What went wrong with a library, without repeating that it was the library.
fn library_reason(err: &ParseError) -> String {
    match err {
        ParseError::Library(reason) => reason.clone(),
        other => other.to_string(),
    }
}

/// One renderable page of a package.
#[derive(Debug)]
pub struct PackagePage {
    /// The filename the page came from.
    pub name: String,
    /// Whether this is `library.guix`'s own page.
    pub is_library: bool,
    pub document: Result<GuiDocument, ParseError>,
}

/// What a caller handed over: a package, or bare markup (RFC-0043). The two
/// are told apart by their first bytes, `PK` against `<`.
#[derive(Debug, Clone)]
pub enum GuiInput {
    Package(GuiPackage),
    Markup(String),
}

pub fn read_gui_input(bytes: &[u8]) -> Result<GuiInput, PackageError> {
    if bytes.starts_with(b"PK") {
        return read_gui_package(bytes).map(GuiInput::Package);
    }

    let text = std::str::from_utf8(bytes).map_err(|_| PackageError::Utf8("markup".to_owned()))?;
    // A byte-order mark or leading whitespace is still markup.
    if text
        .trim_start_matches('\u{feff}')
        .trim_start()
        .starts_with('<')
    {
        Ok(GuiInput::Markup(text.to_owned()))
    } else {
        Err(PackageError::UnknownInput)
    }
}

/// The markup of a package's only document.
pub fn read_gui_package_xml(bytes: &[u8]) -> Result<String, PackageError> {
    let package = read_gui_package(bytes)?;
    package.single_document()?.xml().map(ToOwned::to_owned)
}

pub fn read_gui_package(bytes: &[u8]) -> Result<GuiPackage, PackageError> {
    let mut archive = ZipArchive::new(Cursor::new(bytes))?;
    let mut documents = Vec::new();
    let mut library = None;
    let mut assets = BTreeMap::new();

    for index in 0..archive.len() {
        let mut file = archive.by_index(index)?;
        if file.is_dir() {
            continue;
        }

        let name = file.name().to_owned();
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;

        // Documents sit at the root. A `.guix` inside a directory is not one.
        if name.ends_with(".guix") && !name.contains('/') {
            let document = PackageDocument { name, bytes };
            if document.name == LIBRARY_FILENAME {
                library = Some(document);
            } else {
                documents.push(document);
            }
        } else {
            assets.insert(name, bytes);
        }
    }

    if documents.is_empty() && library.is_none() {
        return Err(PackageError::NoDocuments);
    }
    documents.sort_by(|a, b| a.name.cmp(&b.name));

    Ok(GuiPackage {
        documents,
        library,
        assets,
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::io::Write;
    use zip::{write::SimpleFileOptions, ZipWriter};

    /// A package holding `entries`, in the order given.
    pub(crate) fn zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut bytes = Cursor::new(Vec::new());
        {
            let mut zip = ZipWriter::new(&mut bytes);
            for (name, contents) in entries {
                zip.start_file(*name, SimpleFileOptions::default())
                    .expect("entry starts");
                zip.write_all(contents).expect("entry writes");
            }
            zip.finish().expect("zip finishes");
        }
        bytes.into_inner()
    }

    #[test]
    fn reads_design_and_packaged_assets() {
        let bytes = zip(&[
            ("design.guix", b"<gui version=\"0.2\"><col /></gui>"),
            ("assets/icon.svg", b"<svg />"),
        ]);

        let package = read_gui_package(&bytes).expect("package reads");

        assert!(package
            .single_document()
            .unwrap()
            .xml()
            .unwrap()
            .contains("<gui"));
        assert_eq!(
            package.assets.get("assets/icon.svg").map(Vec::as_slice),
            Some(b"<svg />".as_slice())
        );
        assert!(!package.is_multi_document());
    }

    #[test]
    fn a_single_document_need_not_be_called_design() {
        let bytes = zip(&[("01-hello-world.guix", b"<gui version=\"0.3\"><col /></gui>")]);

        let xml = read_gui_package_xml(&bytes).expect("the only document is found by listing");
        assert!(xml.contains("0.3"));
    }

    #[test]
    fn documents_are_in_filename_order_and_the_library_is_not_one() {
        let bytes = zip(&[
            ("02-signup.guix", b"<gui />"),
            ("library.guix", b"<gui />"),
            ("preview.webp", b"webp"),
            ("01-welcome.guix", b"<gui />"),
            ("design.guix", b"<gui />"),
        ]);

        let package = read_gui_package(&bytes).expect("package reads");

        let names: Vec<_> = package.documents.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, ["01-welcome.guix", "02-signup.guix", "design.guix"]);
        assert_eq!(package.library.as_ref().unwrap().name, "library.guix");
        assert!(package.assets.contains_key("preview.webp"));
        assert!(package.is_multi_document());
        assert!(matches!(
            package.single_document(),
            Err(PackageError::NotSingleDocument(names)) if names.len() == 3
        ));
    }

    #[test]
    fn a_guix_inside_a_directory_is_not_a_document() {
        let bytes = zip(&[
            ("design.guix", b"<gui />"),
            ("assets/nested.guix", b"<gui />"),
        ]);

        let package = read_gui_package(&bytes).expect("package reads");
        assert_eq!(package.documents.len(), 1);
        assert!(package.assets.contains_key("assets/nested.guix"));
    }

    #[test]
    fn a_package_without_any_guix_is_refused() {
        let bytes = zip(&[("assets/icon.svg", b"<svg />")]);
        assert!(matches!(
            read_gui_package(&bytes),
            Err(PackageError::NoDocuments)
        ));
    }

    #[test]
    fn one_unreadable_document_does_not_take_the_package_with_it() {
        let bytes = zip(&[
            ("01-good.guix", b"<gui />"),
            ("02-bad.guix", &[0xff, 0xfe, 0x00]),
        ]);

        let package = read_gui_package(&bytes).expect("package still reads");
        assert!(package.documents[0].xml().is_ok());
        assert!(
            matches!(package.documents[1].xml(), Err(PackageError::Utf8(name)) if name == "02-bad.guix")
        );
    }

    #[test]
    fn input_is_told_apart_by_its_first_bytes() {
        let package = zip(&[("design.guix", b"<gui />")]);
        assert!(matches!(read_gui_input(&package), Ok(GuiInput::Package(_))));
        assert!(matches!(
            read_gui_input(b"\n  <gui version=\"0.3\" />"),
            Ok(GuiInput::Markup(_))
        ));
        assert!(matches!(
            read_gui_input(b"hello"),
            Err(PackageError::UnknownInput)
        ));
    }

    const LIBRARY: &[u8] = br##"<gui version="0.3" name="Library">
      <tokens>
        <color name="primary" value="#007AFF" />
        <color name="ink" value="#111111" />
      </tokens>
      <styles>
        <text-style name="Title" font-size="24" font-weight="700" color="$ink" />
      </styles>
      <components>
        <component id="comp-button">
          <props><prop name="label" type="string" target="label" /></props>
          <row fill="$primary" p="8 16" radius="8">
            <text id="label" text-style="Title" value="Button" />
          </row>
        </component>
        <component id="comp-screen">
          <col w="390" gap="16">
            <instance component="comp-button" label="Back" />
            <col id="body" slot="body" gap="8" />
          </col>
        </component>
      </components>
      <col w="900" p="48" gap="24">
        <text text-style="Title" value="Buttons" />
        <instance component="comp-button" />
      </col>
    </gui>"##;

    fn page<'a>(pages: &'a [PackagePage], name: &str) -> &'a PackagePage {
        pages
            .iter()
            .find(|page| page.name == name)
            .expect("page exists")
    }

    fn find<'a>(node: &'a crate::GuiNode, id: &str) -> Option<&'a crate::GuiNode> {
        if node.attributes.get("id").is_some_and(|it| it == id) {
            return Some(node);
        }
        node.children.iter().find_map(|child| find(child, id))
    }

    #[test]
    fn a_document_uses_what_the_library_declares_without_importing_it() {
        let bytes = zip(&[
            ("library.guix", LIBRARY),
            (
                "01-welcome.guix",
                br##"<gui version="0.3">
                  <tokens><color name="hero" value="#ff0000" /></tokens>
                  <components>
                    <component id="comp-hello"><text id="t" value="Hello" color="$hero" /></component>
                  </components>
                  <frame w="390" h="600" fill="$primary">
                    <instance component="comp-screen">
                      <slot name="body">
                        <instance component="comp-hello" />
                        <instance component="comp-button" label="Next" />
                      </slot>
                    </instance>
                  </frame>
                </gui>"##,
            ),
            (
                "02-done.guix",
                br##"<gui version="0.3"><col w="390"><text text-style="Title" value="Done" /></col></gui>"##,
            ),
        ]);

        let package = read_gui_package(&bytes).unwrap();
        let pages = package.pages();
        let names: Vec<_> = pages.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["01-welcome.guix", "02-done.guix", "library.guix"]);

        let welcome = page(&pages, "01-welcome.guix").document.as_ref().unwrap();
        assert_eq!(
            welcome.metadata.tokens["primary"], "#007AFF",
            "library token"
        );
        assert_eq!(welcome.metadata.tokens["hero"], "#ff0000", "local token");
        let body = find(&welcome.root, "body").expect("library scaffold expanded");
        assert_eq!(
            body.children.len(),
            2,
            "local and library components fill its slot"
        );
        assert_eq!(body.children[0].attributes["value"], "Hello");

        assert!(page(&pages, "02-done.guix").document.is_ok());

        let library = page(&pages, "library.guix");
        assert!(library.is_library, "the library's own page renders too");
        let style_guide = library.document.as_ref().unwrap();
        assert_eq!(style_guide.root.attributes["w"], "900");
    }

    #[test]
    fn a_library_without_a_layout_root_is_declarations_only() {
        let bytes = zip(&[
            ("library.guix", br##"<gui version="0.3"><tokens><color name="primary" value="#000" /></tokens></gui>"##),
            ("design.guix", br##"<gui version="0.3"><col w="10" fill="$primary" /></gui>"##),
        ]);

        let pages = read_gui_package(&bytes).unwrap().pages();
        assert_eq!(pages.len(), 1, "nothing to render for the library");
        assert!(pages[0].document.is_ok());
    }

    #[test]
    fn redeclaring_a_library_name_is_an_error_not_an_override() {
        let bytes = zip(&[
            ("library.guix", LIBRARY),
            (
                "01-a.guix",
                br##"<gui version="0.3">
                  <tokens><color name="primary" value="#ff0000" /></tokens>
                  <components><component id="comp-button"><text value="x" /></component></components>
                  <col w="10" />
                </gui>"##,
            ),
        ]);

        let pages = read_gui_package(&bytes).unwrap().pages();
        let err = page(&pages, "01-a.guix")
            .document
            .as_ref()
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("token '$primary' is already declared by library.guix"),
            "{err}"
        );
        assert!(
            err.contains("component 'comp-button' is already declared"),
            "{err}"
        );
    }

    #[test]
    fn a_name_that_resolves_nowhere_is_an_error() {
        let bytes = zip(&[
            ("library.guix", LIBRARY),
            (
                "01-a.guix",
                br##"<gui version="0.3">
                  <col w="10" fill="$missing">
                    <text text-style="Nope" value="$4.99" />
                    <instance component="comp-nope" />
                  </col>
                </gui>"##,
            ),
        ]);

        let pages = read_gui_package(&bytes).unwrap().pages();
        let err = page(&pages, "01-a.guix")
            .document
            .as_ref()
            .unwrap_err()
            .to_string();
        assert!(err.contains("token '$missing'"), "{err}");
        assert!(err.contains("text style 'Nope'"), "{err}");
        assert!(err.contains("unknown component 'comp-nope'"), "{err}");
        assert!(
            !err.contains("4.99"),
            "a price in text is not a token: {err}"
        );
    }

    #[test]
    fn the_library_depends_on_nothing() {
        let bytes = zip(&[
            (
                "library.guix",
                br##"<gui version="0.3">
                  <components>
                    <component id="comp-a"><row fill="$local-only"><instance component="comp-local" /></row></component>
                  </components>
                </gui>"##,
            ),
            (
                "01-a.guix",
                br##"<gui version="0.3">
                  <tokens><color name="local-only" value="#fff" /></tokens>
                  <components><component id="comp-local"><text value="x" /></component></components>
                  <col w="10"><instance component="comp-a" /></col>
                </gui>"##,
            ),
        ]);

        let pages = read_gui_package(&bytes).unwrap().pages();
        let err = page(&pages, "01-a.guix")
            .document
            .as_ref()
            .unwrap_err()
            .to_string();
        assert!(err.starts_with("library.guix is invalid"), "{err}");
        assert!(
            err.contains("'$local-only'") && err.contains("'comp-local'"),
            "{err}"
        );
    }

    #[test]
    fn one_broken_document_is_reported_by_name_and_the_rest_render() {
        let bytes = zip(&[
            (
                "01-good.guix",
                br##"<gui version="0.3"><col w="10" /></gui>"##,
            ),
            ("02-broken.guix", br##"<gui version="0.3"><col w="10">"##),
            (
                "03-good.guix",
                br##"<gui version="0.3"><col w="10" /></gui>"##,
            ),
        ]);

        let pages = read_gui_package(&bytes).unwrap().pages();
        assert!(pages[0].document.is_ok());
        assert!(matches!(pages[1].document, Err(ParseError::Xml(_))));
        assert!(pages[2].document.is_ok());
    }

    #[test]
    fn every_document_in_a_multi_document_package_declares_0_3() {
        let bytes = zip(&[
            ("01-a.guix", br##"<gui version="0.3"><col w="10" /></gui>"##),
            ("02-b.guix", br##"<gui version="0.2"><col w="10" /></gui>"##),
        ]);
        let pages = read_gui_package(&bytes).unwrap().pages();
        assert!(pages[0].document.is_ok());
        assert!(matches!(
            pages[1].document,
            Err(ParseError::VersionBelowPackage(_))
        ));

        // A lone 0.2 document keeps working, under any name.
        let bytes = zip(&[(
            "hello.guix",
            br##"<gui version="0.2"><col w="10" /></gui>"##,
        )]);
        assert!(read_gui_package(&bytes).unwrap().pages()[0]
            .document
            .is_ok());
    }
}
