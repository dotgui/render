//! Reading a `.gui` package: the documents at its root, its library, and its
//! assets.
//!
//! RFC-0042 lets a package carry one document or many. There is no fixed name
//! and no manifest: every `.guix` at the root is a document, whatever it is
//! called, except `library.guix`, which holds the package's shared
//! declarations. Order is the lexical sort of the filenames.
//!
//! RFC-0054 lets a package be a directory named `*.gui` as well as a ZIP. Both
//! forms pass through [`read_gui_entries`], which applies the same rules:
//! dotfiles and anything outside the package are ignored and listed, and a
//! path that leaves the package, two paths that differ only in case or
//! Unicode normalization, or (in a directory) a symbolic link, are refused.

use crate::parser::{parse_gui_xml_with, parse_library, Library, ParseError, ParseOptions};
use crate::{GuiDocument, Modes};
use std::{
    collections::BTreeMap,
    io::{Cursor, Read},
};
use thiserror::Error;
use unicode_normalization::UnicodeNormalization;
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

    #[error("entry \"{0}\" resolves outside the package")]
    EscapesPackage(String),

    #[error("entries \"{0}\" and \"{1}\" differ only in case or Unicode normalization, which a file system cannot hold apart")]
    Collision(String, String),

    #[error("\"{0}\" is a symbolic link; a package holds files, never links")]
    SymbolicLink(String),

    #[error("{0} is not a package: a package directory's name ends in .gui")]
    NotAPackageDirectory(String),
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
    /// Entries read but not part of the package — dotfiles, stray files, a
    /// `.guix` below the root — in path order (RFC-0054).
    pub ignored: Vec<String>,
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
        self.parse_document_in_mode(document, library, None)
    }

    /// Parses one document in a chosen mode (RFC-0037).
    ///
    /// The mode names an axis's active value — `{"theme": "dark"}` — for the
    /// axes the document declares. A subtree that pins its own keeps it.
    pub fn parse_document_in_mode(
        &self,
        document: &PackageDocument,
        library: Option<&Result<Library, ParseError>>,
        mode: Option<&Modes>,
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
                mode,
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
        self.pages_in_mode(None)
    }

    /// Every page the package renders, in a chosen mode (RFC-0037).
    pub fn pages_in_mode(&self, mode: Option<&Modes>) -> Vec<PackagePage> {
        let library = self.parse_library();

        let mut pages: Vec<PackagePage> = self
            .documents
            .iter()
            .map(|document| PackagePage {
                name: document.name.clone(),
                is_library: false,
                document: self.parse_document_in_mode(document, library.as_ref(), mode),
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
    let mut entries = Vec::new();
    for index in 0..archive.len() {
        let mut file = archive.by_index(index)?;
        if file.is_dir() {
            continue;
        }
        let name = file.name().to_owned();
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        entries.push((name, bytes));
    }
    read_gui_entries(entries)
}

/// Reads a package directory: a directory whose name ends in `.gui`, holding
/// the same entries a ZIP would (RFC-0054). Symbolic links are refused, never
/// followed.
#[cfg(not(target_arch = "wasm32"))]
pub fn read_gui_directory(path: &std::path::Path) -> Result<GuiPackage, PackageError> {
    let is_package = path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.ends_with(".gui"));
    if !is_package {
        return Err(PackageError::NotAPackageDirectory(
            path.display().to_string(),
        ));
    }

    fn walk(
        root: &std::path::Path,
        dir: &std::path::Path,
        entries: &mut Vec<(String, Vec<u8>)>,
    ) -> Result<(), PackageError> {
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let relative = entry
                .path()
                .strip_prefix(root)
                .map(|it| it.to_string_lossy().replace('\\', "/"))
                .unwrap_or_default();
            let kind = entry.file_type()?;
            if kind.is_symlink() {
                return Err(PackageError::SymbolicLink(relative));
            }
            if kind.is_dir() {
                walk(root, &entry.path(), entries)?;
            } else {
                entries.push((relative, std::fs::read(entry.path())?));
            }
        }
        Ok(())
    }

    let mut entries = Vec::new();
    walk(path, path, &mut entries)?;
    read_gui_entries(entries)
}

/// What an entry is to the package, or `None` when it is not part of it.
fn entry_kind(path: &str) -> Option<EntryKind> {
    // A dotfile is a tool's state, at any depth — never part of the package.
    if path.split('/').any(|segment| segment.starts_with('.')) {
        return None;
    }
    if !path.contains('/') {
        return if path.ends_with(".guix") {
            Some(EntryKind::Document)
        } else if path == "preview.webp" {
            Some(EntryKind::Asset)
        } else {
            None
        };
    }
    path.starts_with("assets/").then_some(EntryKind::Asset)
}

enum EntryKind {
    Document,
    Asset,
}

/// Builds a package from its entries — a ZIP's, or a walked directory's —
/// applying the rules both forms share (RFC-0054).
pub fn read_gui_entries(entries: Vec<(String, Vec<u8>)>) -> Result<GuiPackage, PackageError> {
    let mut documents = Vec::new();
    let mut library = None;
    let mut assets = BTreeMap::new();
    let mut ignored = Vec::new();
    let mut seen: BTreeMap<String, String> = BTreeMap::new();

    for (raw, bytes) in entries {
        let name: String = raw.nfc().collect();
        let escapes = name.starts_with('/')
            || name.starts_with('\\')
            || name.as_bytes().get(1) == Some(&b':')
            || name.split(['/', '\\']).any(|segment| segment == "..");
        if escapes {
            return Err(PackageError::EscapesPackage(raw));
        }
        let key = name.to_lowercase();
        if let Some(other) = seen.get(&key) {
            return Err(PackageError::Collision(other.clone(), raw));
        }
        seen.insert(key, raw);

        match entry_kind(&name) {
            Some(EntryKind::Document) => {
                let document = PackageDocument { name, bytes };
                if document.name == LIBRARY_FILENAME {
                    library = Some(document);
                } else {
                    documents.push(document);
                }
            }
            Some(EntryKind::Asset) => {
                assets.insert(name, bytes);
            }
            None => ignored.push(name),
        }
    }

    if documents.is_empty() && library.is_none() {
        return Err(PackageError::NoDocuments);
    }
    documents.sort_by(|a, b| a.name.cmp(&b.name));
    ignored.sort();

    Ok(GuiPackage {
        documents,
        library,
        assets,
        ignored,
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

    const FILES: &[(&str, &[u8])] = &[
        (
            "01-welcome.guix",
            b"<gui version=\"0.4.1\"><col w=\"390\" /></gui>",
        ),
        ("library.guix", b"<gui version=\"0.4.1\"><tokens /></gui>"),
        ("preview.webp", b"webp"),
        ("assets/logo.svg", b"<svg />"),
    ];

    fn owned(files: &[(&str, &[u8])]) -> Vec<(String, Vec<u8>)> {
        files
            .iter()
            .map(|(name, bytes)| ((*name).to_owned(), bytes.to_vec()))
            .collect()
    }

    #[test]
    fn a_zip_and_its_directory_are_the_same_package() {
        let from_zip = read_gui_package(&zip(FILES)).expect("zip reads");
        let dir = std::env::temp_dir().join(format!("dotgui-folder-{}.gui", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for (name, bytes) in FILES {
            let path = dir.join(name);
            std::fs::create_dir_all(path.parent().expect("parent")).expect("dir");
            std::fs::write(path, bytes).expect("write");
        }
        std::fs::write(dir.join(".DS_Store"), b"x").expect("write");
        let from_dir = read_gui_directory(&dir).expect("directory reads");
        let _ = std::fs::remove_dir_all(&dir);

        let names = |p: &GuiPackage| {
            p.documents
                .iter()
                .map(|d| d.name.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(names(&from_dir), names(&from_zip));
        assert_eq!(from_dir.assets, from_zip.assets);
        assert_eq!(
            from_dir.library.map(|l| l.bytes),
            from_zip.library.map(|l| l.bytes)
        );
        assert_eq!(from_dir.ignored, vec![".DS_Store".to_owned()]);
    }

    #[test]
    fn a_directory_must_be_named_gui_and_hold_no_links() {
        let base = std::env::temp_dir().join(format!("dotgui-links-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let plain = base.join("plain");
        std::fs::create_dir_all(&plain).expect("dir");
        assert!(matches!(
            read_gui_directory(&plain),
            Err(PackageError::NotAPackageDirectory(_))
        ));

        let package = base.join("linked.gui");
        std::fs::create_dir_all(package.join("assets")).expect("dir");
        std::fs::write(
            package.join("01.guix"),
            b"<gui version=\"0.4.1\"><col /></gui>",
        )
        .expect("write");
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("/etc/hosts", package.join("assets/hosts.svg"))
                .expect("link");
            assert!(matches!(
                read_gui_directory(&package),
                Err(PackageError::SymbolicLink(path)) if path == "assets/hosts.svg"
            ));
        }
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn litter_is_ignored_and_listed() {
        let mut files = owned(FILES);
        for name in [
            ".git/config",
            "assets/.hidden.png",
            "notes.txt",
            "nested/02.guix",
        ] {
            files.push((name.to_owned(), b"x".to_vec()));
        }
        let package = read_gui_entries(files).expect("reads");
        assert_eq!(
            package.ignored,
            [
                ".git/config",
                "assets/.hidden.png",
                "nested/02.guix",
                "notes.txt"
            ]
        );
        assert!(!package.assets.contains_key("assets/.hidden.png"));
    }

    #[test]
    fn escaping_paths_and_collisions_are_refused() {
        for bad in [
            "../outside.svg",
            "/abs.svg",
            "assets/../../x.svg",
            "C:/x.svg",
        ] {
            let mut files = owned(FILES);
            files.push((bad.to_owned(), b"x".to_vec()));
            assert!(
                matches!(
                    read_gui_entries(files),
                    Err(PackageError::EscapesPackage(_))
                ),
                "{bad}"
            );
        }
        let mut files = owned(FILES);
        files.push(("assets/Logo.svg".to_owned(), b"x".to_vec()));
        assert!(matches!(
            read_gui_entries(files),
            Err(PackageError::Collision(..))
        ));

        let mut files = owned(FILES);
        files.push(("assets/caf\u{e9}.svg".to_owned(), b"x".to_vec()));
        files.push(("assets/cafe\u{301}.svg".to_owned(), b"y".to_vec()));
        assert!(matches!(
            read_gui_entries(files),
            Err(PackageError::Collision(..))
        ));
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
