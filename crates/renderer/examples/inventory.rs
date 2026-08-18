use std::{
    collections::BTreeMap,
    env, fs,
    path::{Path, PathBuf},
    process,
};

use dotgui_renderer::{parse_gui_xml, read_gui_package_xml, GuiNode};

fn main() {
    let path = env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("examples"));

    let files = collect_gui_files(&path);
    if files.is_empty() {
        eprintln!("no .gui files found at {}", path.display());
        process::exit(1);
    }

    let mut tags = BTreeMap::new();
    let mut attrs = BTreeMap::new();
    let mut attrs_by_tag: BTreeMap<String, BTreeMap<String, usize>> = BTreeMap::new();

    for file in &files {
        let bytes = fs::read(file).unwrap_or_else(|err| {
            eprintln!("failed to read {}: {err}", file.display());
            process::exit(1);
        });
        let xml = read_gui_package_xml(&bytes).unwrap_or_else(|err| {
            eprintln!("failed to open {}: {err}", file.display());
            process::exit(1);
        });
        let document = parse_gui_xml(&xml).unwrap_or_else(|err| {
            eprintln!("failed to parse {}: {err}", file.display());
            process::exit(1);
        });
        walk(&document.root, &mut tags, &mut attrs, &mut attrs_by_tag);
    }

    println!("files: {}", files.len());
    print_counts("tags", &tags, 30);
    print_counts("attributes", &attrs, 40);

    println!("\nlayout-ish attributes by common container tag:");
    for tag in ["col", "row", "frame", "grid", "text", "rect", "img"] {
        if let Some(counts) = attrs_by_tag.get(tag) {
            print_counts(tag, counts, 24);
        }
    }
}

fn collect_gui_files(path: &Path) -> Vec<PathBuf> {
    if path.is_file() {
        return path
            .extension()
            .is_some_and(|ext| ext == "gui")
            .then(|| path.to_owned())
            .into_iter()
            .collect();
    }

    let mut files = fs::read_dir(path)
        .unwrap_or_else(|err| {
            eprintln!("failed to read {}: {err}", path.display());
            process::exit(1);
        })
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "gui"))
        .collect::<Vec<_>>();
    files.sort();
    files
}

fn walk(
    node: &GuiNode,
    tags: &mut BTreeMap<String, usize>,
    attrs: &mut BTreeMap<String, usize>,
    attrs_by_tag: &mut BTreeMap<String, BTreeMap<String, usize>>,
) {
    *tags.entry(node.tag.clone()).or_default() += 1;

    for attr in node.attributes.keys() {
        *attrs.entry(attr.clone()).or_default() += 1;
        *attrs_by_tag
            .entry(node.tag.clone())
            .or_default()
            .entry(attr.clone())
            .or_default() += 1;
    }

    for child in &node.children {
        walk(child, tags, attrs, attrs_by_tag);
    }
}

fn print_counts(title: &str, counts: &BTreeMap<String, usize>, limit: usize) {
    let mut sorted = counts.iter().collect::<Vec<_>>();
    sorted.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));

    println!("\n{title}:");
    for (name, count) in sorted.into_iter().take(limit) {
        println!("  {name:<24} {count}");
    }
}
