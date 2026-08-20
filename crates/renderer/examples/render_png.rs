use std::{env, fs, path::Path, process};

use dotgui_renderer::{
    build_scene, compute_taffy_layout_with_text, paint_scene_to_png_with_assets_and_fonts,
    parse_gui_xml, read_gui_package, AssetCache, FontStore,
};

fn main() {
    let input = env::args().nth(1).unwrap_or_else(|| {
        eprintln!("usage: cargo run -p dotgui-renderer --example render_png <file.gui> <out.png>");
        process::exit(2);
    });
    let output = env::args().nth(2).unwrap_or_else(|| {
        eprintln!("usage: cargo run -p dotgui-renderer --example render_png <file.gui> <out.png>");
        process::exit(2);
    });

    let bytes = fs::read(&input).unwrap_or_else(|err| {
        eprintln!("failed to read {input}: {err}");
        process::exit(1);
    });

    let (xml, package_assets) = if is_gui_package(&input, &bytes) {
        let package = read_gui_package(&bytes).unwrap_or_else(|err| {
            eprintln!("failed to open {input}: {err}");
            process::exit(1);
        });
        (package.xml, package.assets)
    } else {
        let xml = String::from_utf8(bytes).unwrap_or_else(|err| {
            eprintln!("failed to read {input} as UTF-8 XML: {err}");
            process::exit(1);
        });
        (xml, Default::default())
    };

    let document = parse_gui_xml(&xml).unwrap_or_else(|err| {
        eprintln!("failed to parse {input}: {err}");
        process::exit(1);
    });
    let cache = AssetCache::new(".gui-render/cache").with_package_assets(package_assets);
    let fonts = FontStore::from_document(&document, &cache).unwrap_or_else(|err| {
        eprintln!("warning: failed to resolve declared fonts: {err}");
        FontStore::default()
    });
    // A render that quietly used the wrong typeface is worse than one that
    // says so.
    for warning in fonts.warnings() {
        eprintln!("warning: {warning}");
    }

    let layout = compute_taffy_layout_with_text(&document, &fonts).unwrap_or_else(|err| {
        eprintln!("failed to lay out {input}: {err}");
        process::exit(1);
    });
    let scene = build_scene(&document, &layout);
    paint_scene_to_png_with_assets_and_fonts(&scene, &output, &cache, &fonts).unwrap_or_else(
        |err| {
            eprintln!("failed to paint {output}: {err}");
            process::exit(1);
        },
    );

    println!(
        "wrote {output} using asset cache {}",
        cache.root().display()
    );
}

fn is_gui_package(path: &str, bytes: &[u8]) -> bool {
    let has_gui_ext = Path::new(path).extension().is_some_and(|ext| ext == "gui");
    has_gui_ext || bytes.starts_with(b"PK\x03\x04")
}
