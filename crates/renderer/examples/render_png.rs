use std::{
    env, fs,
    path::{Path, PathBuf},
    process,
};

use dotgui_renderer::{
    build_scene, compute_taffy_layout_with_text, paint_scene_to_png_with_assets_and_fonts,
    parse_standalone_xml, read_gui_input, AssetCache, FontStore, GuiDocument, GuiInput, ParseError,
    RENDERER_VERSION, SUPPORTED_VERSION,
};

const USAGE: &str =
    "usage: cargo run -p dotgui-renderer --example render_png <file.gui|file.guix> <out.png>";

/// Renders a `.gui` package or bare `.guix` markup to PNG.
///
/// A package with one page writes `<out.png>`. A package with several — many
/// documents, or a library with a page of its own — writes one PNG per page
/// beside it, named `<out>-<document>.png`.
fn main() {
    if env::args().nth(1).as_deref() == Some("--version") {
        println!(
            "dotgui renderer {RENDERER_VERSION} (reads .gui up to version {SUPPORTED_VERSION})"
        );
        return;
    }
    let input = env::args().nth(1).unwrap_or_else(|| {
        eprintln!("{USAGE}");
        process::exit(2);
    });
    let output = env::args().nth(2).unwrap_or_else(|| {
        eprintln!("{USAGE}");
        process::exit(2);
    });

    let bytes = fs::read(&input).unwrap_or_else(|err| {
        eprintln!("failed to read {input}: {err}");
        process::exit(1);
    });

    let (pages, package_assets): (Vec<(String, Result<GuiDocument, ParseError>)>, _) =
        match read_gui_input(&bytes) {
            Ok(GuiInput::Package(package)) => (
                package
                    .pages()
                    .into_iter()
                    .map(|page| (page.name, page.document))
                    .collect(),
                package.assets,
            ),
            Ok(GuiInput::Markup(xml)) => {
                let name = Path::new(&input)
                    .file_name()
                    .map_or_else(|| input.clone(), |name| name.to_string_lossy().into_owned());
                (vec![(name, parse_standalone_xml(&xml))], Default::default())
            }
            Err(err) => {
                eprintln!("failed to open {input}: {err}");
                process::exit(1);
            }
        };

    let cache = AssetCache::new(".gui-render/cache").with_package_assets(package_assets);
    let single = pages.len() == 1;
    let mut failed = false;

    for (name, document) in pages {
        let document = match document {
            Ok(document) => document,
            Err(err) => {
                // One broken document is reported by name; the rest still render.
                eprintln!("error: {name}: {err}");
                failed = true;
                continue;
            }
        };
        for warning in &document.warnings {
            eprintln!("warning: {name}: {warning}");
        }

        let target = if single {
            PathBuf::from(&output)
        } else {
            page_output(&output, &name)
        };
        render(&document, &cache, &name, &target);
    }

    if failed {
        process::exit(1);
    }
}

fn render(document: &GuiDocument, cache: &AssetCache, name: &str, output: &Path) {
    let fonts = FontStore::from_document(document, cache).unwrap_or_else(|err| {
        eprintln!("warning: {name}: failed to resolve declared fonts: {err}");
        FontStore::default()
    });
    // A render that quietly used the wrong typeface is worse than one that
    // says so.
    for warning in fonts.warnings() {
        eprintln!("warning: {name}: {warning}");
    }

    let layout = compute_taffy_layout_with_text(document, &fonts).unwrap_or_else(|err| {
        eprintln!("failed to lay out {name}: {err}");
        process::exit(1);
    });
    let scene = build_scene(document, &layout);
    paint_scene_to_png_with_assets_and_fonts(&scene, output, cache, &fonts).unwrap_or_else(|err| {
        eprintln!("failed to paint {}: {err}", output.display());
        process::exit(1);
    });

    println!(
        "wrote {} using asset cache {}",
        output.display(),
        cache.root().display()
    );
}

/// `out.png` and `02-signup.guix` make `out-02-signup.png`.
fn page_output(output: &str, document: &str) -> PathBuf {
    let output = Path::new(output);
    let stem = output
        .file_stem()
        .map_or_else(|| "page".into(), |stem| stem.to_string_lossy());
    let document = Path::new(document)
        .file_stem()
        .map_or_else(|| "page".into(), |stem| stem.to_string_lossy());
    output.with_file_name(format!("{stem}-{document}.png"))
}
