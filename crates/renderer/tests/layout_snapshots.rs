//! Deterministic layout snapshots for every example in the workspace.
//!
//! The golden PNGs depend on which fonts the host happens to have, so they only
//! run on macOS. These snapshots exist to give every platform a regression
//! signal that cannot drift: they use [`compute_taffy_layout`], whose
//! `ApproxTextMeasurer` derives widths from the character count alone — no font
//! files, no network, identical everywhere.
//!
//! They cover the class of bug that pixel diffs are worst at explaining:
//! a box that collapses, a line that stops wrapping, a size that gets rounded
//! away. Review the diff and regenerate with:
//!
//! ```text
//! UPDATE_SNAPSHOTS=1 cargo test -p dotgui-renderer --test layout_snapshots
//! ```

use dotgui_renderer::{compute_taffy_layout, parse_gui_xml, read_gui_package_xml, LayoutBox};
use std::{
    fmt::Write as _,
    fs,
    path::{Path, PathBuf},
};

#[test]
fn layout_matches_committed_snapshots() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let examples_dir = manifest_dir.join("../../examples");
    let snapshots_dir = manifest_dir.join("tests").join("snapshots");
    let updating = std::env::var_os("UPDATE_SNAPSHOTS").is_some();

    // Hand-written fixtures cover features no example package uses yet.
    let fixtures_dir = manifest_dir.join("tests").join("fixtures");
    let mut example_paths = Vec::new();
    if let Ok(entries) = fs::read_dir(&fixtures_dir) {
        for entry in entries {
            let path = entry.expect("fixture entry is readable").path();
            if path.extension().is_some_and(|ext| ext == "guix") {
                example_paths.push(path);
            }
        }
    }
    let entries = fs::read_dir(&examples_dir).unwrap_or_else(|err| {
        panic!(
            "examples directory {} is unreadable ({err}); this test would \
             otherwise pass without checking anything",
            examples_dir.display()
        )
    });
    for entry in entries {
        let path = entry.expect("example entry is readable").path();
        if path.extension().is_some_and(|ext| ext == "gui") {
            example_paths.push(path);
        }
    }
    example_paths.sort();
    assert!(
        !example_paths.is_empty(),
        "expected at least one .gui example in {}",
        examples_dir.display()
    );

    if updating {
        fs::create_dir_all(&snapshots_dir).expect("snapshots dir is creatable");
    }

    let mut failures = Vec::new();
    for path in &example_paths {
        let name = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .expect("example has a usable file name");

        let rendered = snapshot_for(path);
        let snapshot_path = snapshots_dir.join(format!("{name}.txt"));

        if updating {
            fs::write(&snapshot_path, &rendered).expect("snapshot is writable");
            continue;
        }

        let expected = fs::read_to_string(&snapshot_path).unwrap_or_else(|err| {
            panic!(
                "snapshot {} is missing ({err}); regenerate with \
                 UPDATE_SNAPSHOTS=1 cargo test -p dotgui-renderer --test layout_snapshots",
                snapshot_path.display()
            )
        });

        if expected != rendered {
            failures.push(format!(
                "{name}\n{}",
                first_difference(&expected, &rendered)
            ));
        }
    }

    assert!(
        failures.is_empty(),
        "layout changed for {} example(s):\n\n{}\n\
         If the change is intended, review it and regenerate with \
         UPDATE_SNAPSHOTS=1 cargo test -p dotgui-renderer --test layout_snapshots",
        failures.len(),
        failures.join("\n\n")
    );
}

fn snapshot_for(path: &Path) -> String {
    let bytes = fs::read(path).expect("example is readable");
    // `.gui` is a package; `.guix` is the raw document.
    let xml = if path.extension().is_some_and(|ext| ext == "guix") {
        String::from_utf8(bytes)
            .unwrap_or_else(|err| panic!("{} is not UTF-8: {err}", path.display()))
    } else {
        read_gui_package_xml(&bytes).unwrap_or_else(|err| {
            panic!("{} did not open as a .gui package: {err}", path.display())
        })
    };
    let document =
        parse_gui_xml(&xml).unwrap_or_else(|err| panic!("{} did not parse: {err}", path.display()));
    let layout = compute_taffy_layout(&document)
        .unwrap_or_else(|err| panic!("{} did not lay out: {err}", path.display()));

    let mut out = String::new();
    write_node(&mut out, &layout, 0);
    out
}

/// One line per node: indented tag, position, and size.
///
/// Deliberately plain text rather than JSON so a regression shows up as a few
/// readable lines in a diff.
fn write_node(out: &mut String, node: &LayoutBox, depth: usize) {
    let _ = writeln!(
        out,
        "{:indent$}{} {}x{} @ {},{}",
        "",
        node.tag,
        round(node.rect.width),
        round(node.rect.height),
        round(node.rect.x),
        round(node.rect.y),
        indent = depth * 2,
    );
    for child in &node.children {
        // `<segment>` and `<appearance>` ride in the box tree to reach the
        // scene, but carry no geometry. A snapshot of positions and sizes has
        // nothing to say about them.
        if child.tag == "segment" || child.tag == "appearance" {
            continue;
        }
        write_node(out, child, depth + 1);
    }
}

/// Trims trailing zeros so whole pixels read as `56`, not `56.00`.
fn round(value: f32) -> String {
    let rounded = format!("{value:.2}");
    rounded
        .trim_end_matches('0')
        .trim_end_matches('.')
        .to_owned()
}

/// The first differing line with a little context, so the assertion message
/// points at the node that moved instead of dumping the whole tree.
fn first_difference(expected: &str, actual: &str) -> String {
    let expected_lines: Vec<&str> = expected.lines().collect();
    let actual_lines: Vec<&str> = actual.lines().collect();

    for (index, (want, got)) in expected_lines.iter().zip(actual_lines.iter()).enumerate() {
        if want != got {
            let start = index.saturating_sub(2);
            let mut context = String::new();
            for line in &expected_lines[start..index] {
                let _ = writeln!(context, "   {line}");
            }
            let _ = writeln!(context, "  -{want}");
            let _ = write!(context, "  +{got}");
            return context;
        }
    }

    format!(
        "  tree has {} lines, snapshot has {}",
        actual_lines.len(),
        expected_lines.len()
    )
}
