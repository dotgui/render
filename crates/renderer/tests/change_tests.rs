//! Repainting what an edit changed gives the page the edit made.
//!
//! Each fixture is edited several ways — recoloured, faded, resized, rotated,
//! removed, inserted before, retexted — at nodes spread through its tree. For
//! every edit the areas [`scene_changes`] reports are painted from the edited
//! scene onto a paint of the original, and the result has to match a paint of
//! the edited page whole. A missed area shows up as stale pixels in a flat part
//! of the page; as in the region tests, only anti-aliased edges may differ.

use dotgui_renderer::{
    build_scene, compute_taffy_layout, paint_scene_region_to_rgba, paint_scene_to_rgba,
    parse_gui_xml, scene_changes, GuiDocument, GuiNode, PixelRect, Scene, SceneChanges,
};
use std::{fs, path::PathBuf};

const SCALE: f32 = 1.25;
const ROUNDING: i32 = 2;
const EDGE_CONTRAST: i32 = 8;
const EDITS_PER_FIXTURE: usize = 5;

fn scene_of(document: &GuiDocument) -> Scene {
    let layout = compute_taffy_layout(document).expect("document lays out");
    build_scene(document, &layout).scaled(SCALE)
}

/// Every node below the root, as a path of child indices, in document order.
fn paths(node: &GuiNode, prefix: &mut Vec<usize>, into: &mut Vec<Vec<usize>>) {
    for (index, child) in node.children.iter().enumerate() {
        prefix.push(index);
        into.push(prefix.clone());
        paths(child, prefix, into);
        prefix.pop();
    }
}

fn node_at<'a>(root: &'a mut GuiNode, path: &[usize]) -> &'a mut GuiNode {
    path.iter()
        .fold(root, |node, index| &mut node.children[*index])
}

/// Applies edit number `kind` at `path`, and says what it did.
fn edit(document: &mut GuiDocument, path: &[usize], kind: usize) -> String {
    let (parent_path, index) = path.split_at(path.len() - 1);
    let index = index[0];
    match kind % 7 {
        0 => {
            let node = node_at(&mut document.root, path);
            let attribute = if node.tag == "text" { "color" } else { "fill" };
            node.attributes
                .insert(attribute.to_owned(), "#e0115f".to_owned());
            format!("recolour <{}>", node.tag)
        }
        1 => {
            let node = node_at(&mut document.root, path);
            node.attributes
                .insert("opacity".to_owned(), "0.4".to_owned());
            format!("fade <{}>", node.tag)
        }
        2 => {
            let node = node_at(&mut document.root, path);
            let width = node.attributes.get("w").and_then(|w| w.parse::<f32>().ok());
            match width {
                Some(width) => {
                    node.attributes
                        .insert("w".to_owned(), format!("{}", width + 12.0));
                    format!("widen <{}>", node.tag)
                }
                None => {
                    node.attributes
                        .insert("rotation".to_owned(), "7".to_owned());
                    format!("rotate <{}>", node.tag)
                }
            }
        }
        3 => {
            let parent = node_at(&mut document.root, parent_path);
            let removed = parent.children.remove(index);
            format!("remove <{}>", removed.tag)
        }
        4 => {
            let parent = node_at(&mut document.root, parent_path);
            let mut rect = GuiNode::new("rect");
            for (name, value) in [("w", "18"), ("h", "18"), ("fill", "#1f8a52")] {
                rect.attributes.insert(name.to_owned(), value.to_owned());
            }
            parent.children.insert(index, rect);
            format!("insert before <{}>", parent.children[index + 1].tag)
        }
        5 => {
            let node = node_at(&mut document.root, path);
            if node.tag == "text" {
                node.attributes
                    .insert("value".to_owned(), "Edited".to_owned());
                "retext <text>".to_owned()
            } else {
                node.attributes.insert("radius".to_owned(), "12".to_owned());
                format!("round <{}>", node.tag)
            }
        }
        _ => {
            let node = node_at(&mut document.root, path);
            node.attributes
                .insert("visible".to_owned(), "false".to_owned());
            format!("hide <{}>", node.tag)
        }
    }
}

fn premultiplied(pixels: &[u8], width: u32, x: u32, y: u32) -> [i32; 4] {
    let i = ((y * width + x) * 4) as usize;
    let alpha = i32::from(pixels[i + 3]);
    let channel = |c: usize| i32::from(pixels[i + c]) * alpha / 255;
    [channel(0), channel(1), channel(2), alpha]
}

fn distance(a: [i32; 4], b: [i32; 4]) -> i32 {
    (0..4).map(|c| (a[c] - b[c]).abs()).max().unwrap_or(0)
}

#[test]
fn repainting_the_changed_areas_gives_the_edited_page() {
    let fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let mut files: Vec<_> = fs::read_dir(&fixtures)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "guix"))
        .collect();
    files.sort();

    let (mut failures, mut partial, mut whole, mut repainted, mut total) =
        (Vec::new(), 0, 0, 0.0_f64, 0.0_f64);
    for file in &files {
        let original = parse_gui_xml(&fs::read_to_string(file).unwrap()).unwrap();
        let before = scene_of(&original);
        let (width, height, before_pixels) = paint_scene_to_rgba(&before, None, None).unwrap();

        let mut targets = Vec::new();
        paths(&original.root, &mut Vec::new(), &mut targets);
        if targets.is_empty() {
            continue;
        }
        let step = (targets.len() / EDITS_PER_FIXTURE).max(1);
        for (number, path) in targets
            .iter()
            .step_by(step)
            .take(EDITS_PER_FIXTURE)
            .enumerate()
        {
            let mut edited = original.clone();
            let kind = number + files.iter().position(|it| it == file).unwrap();
            let what = edit(&mut edited, path, kind);
            let after = scene_of(&edited);
            let name = format!(
                "{} ({what} at {path:?})",
                file.file_name().unwrap().to_string_lossy()
            );

            let areas = match scene_changes(&before, &after) {
                SceneChanges::Everything => {
                    whole += 1;
                    continue;
                }
                SceneChanges::Nothing => Vec::new(),
                SceneChanges::Areas(areas) => areas,
            };
            partial += 1;

            let mut patched = before_pixels.clone();
            for area in &areas {
                let region = PixelRect::covering(*area, 1.0);
                repainted += f64::from(region.width) * f64::from(region.height);
                let (_, _, tile) = paint_scene_region_to_rgba(&after, region, None, None).unwrap();
                for y in 0..region.height {
                    for x in 0..region.width {
                        let (px, py) = (region.x + x as i32, region.y + y as i32);
                        if px < 0 || py < 0 || px >= width as i32 || py >= height as i32 {
                            continue;
                        }
                        let from = ((y * region.width + x) * 4) as usize;
                        let to = ((py as u32 * width + px as u32) * 4) as usize;
                        patched[to..to + 4].copy_from_slice(&tile[from..from + 4]);
                    }
                }
            }
            total += f64::from(width) * f64::from(height);

            let (_, _, expected) = paint_scene_to_rgba(&after, None, None).unwrap();
            'pixels: for y in 0..height {
                for x in 0..width {
                    let want = premultiplied(&expected, width, x, y);
                    let got = premultiplied(&patched, width, x, y);
                    if distance(want, got) <= ROUNDING {
                        continue;
                    }
                    let mut contrast = 0;
                    for dy in -1i32..=1 {
                        for dx in -1i32..=1 {
                            let (nx, ny) = (x as i32 + dx, y as i32 + dy);
                            if nx >= 0 && ny >= 0 && nx < width as i32 && ny < height as i32 {
                                contrast = contrast.max(distance(
                                    want,
                                    premultiplied(&expected, width, nx as u32, ny as u32),
                                ));
                            }
                        }
                    }
                    if contrast < EDGE_CONTRAST {
                        failures.push(format!(
                            "{name}: ({x}, {y}) is {got:?} after repainting {} area(s), the edited page has {want:?}",
                            areas.len()
                        ));
                        break 'pixels;
                    }
                }
            }
        }
    }

    eprintln!(
        "{partial} edits repainted in part ({:.0}% of their pages), {whole} whole",
        100.0 * repainted / total.max(1.0)
    );
    assert!(
        partial > whole,
        "most edits should repaint only part of the page"
    );
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
