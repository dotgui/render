use std::{env, fs, path::Path, process};

use dotgui_renderer::{build_scene, compute_taffy_layout, parse_gui_xml, read_gui_package_xml};

fn main() {
    let path = env::args().nth(1).unwrap_or_else(|| {
        eprintln!("usage: cargo run -p dotgui-renderer --example scene <file.gui>");
        process::exit(2);
    });

    let bytes = fs::read(&path).unwrap_or_else(|err| {
        eprintln!("failed to read {path}: {err}");
        process::exit(1);
    });

    let xml = if is_gui_package(&path, &bytes) {
        read_gui_package_xml(&bytes).unwrap_or_else(|err| {
            eprintln!("failed to open {path}: {err}");
            process::exit(1);
        })
    } else {
        String::from_utf8(bytes).unwrap_or_else(|err| {
            eprintln!("failed to read {path} as UTF-8 XML: {err}");
            process::exit(1);
        })
    };

    let document = parse_gui_xml(&xml).unwrap_or_else(|err| {
        eprintln!("failed to parse {path}: {err}");
        process::exit(1);
    });
    let layout = compute_taffy_layout(&document).unwrap_or_else(|err| {
        eprintln!("failed to layout {path}: {err}");
        process::exit(1);
    });
    let scene = build_scene(&document, &layout);

    println!(
        "{}",
        serde_json::to_string_pretty(&scene).expect("scene serializes to json")
    );
}

fn is_gui_package(path: &str, bytes: &[u8]) -> bool {
    let has_gui_ext = Path::new(path).extension().is_some_and(|ext| ext == "gui");
    has_gui_ext || bytes.starts_with(b"PK\x03\x04")
}
