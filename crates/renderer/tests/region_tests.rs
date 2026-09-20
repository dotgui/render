//! A region of a page paints what that rectangle of the whole page paints.
//!
//! Every fixture is painted whole, then again as a grid of odd-sized tiles that
//! straddle its shadows, blurs, masks, rotations and edges, at a scale that
//! puts every edge between whole pixels.
//! Fonts are left out so the test needs nothing from the host; text still
//! paints, as placeholders, through the same layers and clips.
//!
//! The match is exact except on anti-aliased edges. The rasteriser clips a
//! shape that crosses the canvas's edge and re-divides its curves where it is
//! cut, so a shape that crosses a tile's edge can come out a few levels
//! different along its outline. That is invisible, and it is the only thing
//! allowed to differ: a pixel in a flat part of the page must match, beyond a
//! level or two of rounding, which is what catches content that is missing,
//! moved, or blurred wrongly.

use dotgui_renderer::{
    build_scene, compute_taffy_layout, paint_scene_region_to_rgba, paint_scene_to_rgba,
    parse_gui_xml, scene_pixel_size, PixelRect, Scene,
};
use std::{fs, path::PathBuf};

/// How far any pixel may move, premultiplied: rounding in a blur or a
/// gradient's interpolation, a level or two.
const ROUNDING: i32 = 2;
/// How much a pixel may differ from a neighbour for it to count as an edge.
const EDGE_CONTRAST: i32 = 8;

fn scene_of(path: &PathBuf, scale: f32) -> Scene {
    let xml = fs::read_to_string(path).expect("fixture reads");
    let document = parse_gui_xml(&xml).expect("fixture parses");
    let layout = compute_taffy_layout(&document).expect("fixture lays out");
    let scene = build_scene(&document, &layout);
    if scale == 1.0 {
        scene
    } else {
        scene.scaled(scale)
    }
}

struct Image<'a> {
    width: u32,
    height: u32,
    pixels: &'a [u8],
}

impl Image<'_> {
    /// Premultiplied, so a transparent pixel's colour cannot count.
    fn pixel(&self, x: u32, y: u32) -> [i32; 4] {
        let i = ((y * self.width + x) * 4) as usize;
        let alpha = i32::from(self.pixels[i + 3]);
        let channel = |c: usize| i32::from(self.pixels[i + c]) * alpha / 255;
        [channel(0), channel(1), channel(2), alpha]
    }

    fn contrast_around(&self, x: u32, y: u32) -> i32 {
        let centre = self.pixel(x, y);
        let mut most = 0;
        for dy in -1i32..=1 {
            for dx in -1i32..=1 {
                let (nx, ny) = (x as i32 + dx, y as i32 + dy);
                if nx < 0 || ny < 0 || nx >= self.width as i32 || ny >= self.height as i32 {
                    continue;
                }
                most = most.max(distance(centre, self.pixel(nx as u32, ny as u32)));
            }
        }
        most
    }
}

fn distance(a: [i32; 4], b: [i32; 4]) -> i32 {
    (0..4).map(|c| (a[c] - b[c]).abs()).max().unwrap_or(0)
}

/// The first pixel of a tile that differs from the whole page somewhere
/// anti-aliasing cannot explain it.
fn first_mismatch(whole: &Image, tile: &Image, region: PixelRect) -> Option<String> {
    for y in 0..region.height {
        for x in 0..region.width {
            let (px, py) = (x + region.x as u32, y + region.y as u32);
            let expected = whole.pixel(px, py);
            let actual = tile.pixel(x, y);
            let difference = distance(expected, actual);
            if difference <= ROUNDING {
                continue;
            }
            // An edge pixel may differ by any amount: besides clipped
            // outlines, a conic gradient's centre is a single pixel whose
            // angle is undefined, and rounding decides its colour.
            if whole.contrast_around(px, py) < EDGE_CONTRAST {
                return Some(format!(
                    "({px}, {py}) is {actual:?} in a flat area, whole page has {expected:?}"
                ));
            }
        }
    }
    None
}

#[test]
fn a_region_paints_what_the_whole_page_paints_there() {
    let fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let mut paths: Vec<_> = fs::read_dir(&fixtures)
        .expect("fixtures directory reads")
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "guix"))
        .collect();
    paths.sort();
    assert!(!paths.is_empty());

    let mut failures = Vec::new();
    for path in &paths {
        for scale in [1.25] {
            let scene = scene_of(path, scale);
            let (width, height) = scene_pixel_size(&scene);
            let (whole_width, whole_height, pixels) =
                paint_scene_to_rgba(&scene, None, None).expect("whole page paints");
            let whole = Image {
                width: whole_width,
                height: whole_height,
                pixels: &pixels,
            };

            // Odd sizes, so tile edges land everywhere relative to content
            // laid out on round numbers.
            let (tile_width, tile_height) = (211, 149);
            'tiles: for y in (0..height).step_by(tile_height as usize) {
                for x in (0..width).step_by(tile_width as usize) {
                    let region = PixelRect {
                        x: x as i32,
                        y: y as i32,
                        width: tile_width.min(width - x),
                        height: tile_height.min(height - y),
                    };
                    let (_, _, painted) = paint_scene_region_to_rgba(&scene, region, None, None)
                        .expect("region paints");
                    let tile = Image {
                        width: region.width,
                        height: region.height,
                        pixels: &painted,
                    };
                    if let Some(mismatch) = first_mismatch(&whole, &tile, region) {
                        failures.push(format!(
                            "{} at {scale}x, tile {region:?}: {mismatch}",
                            path.file_name().unwrap().to_string_lossy()
                        ));
                        break 'tiles;
                    }
                }
            }
        }
    }

    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn a_region_past_the_page_is_transparent_there() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/transforms.guix");
    let scene = scene_of(&path, 1.0);
    let (width, _) = scene_pixel_size(&scene);
    let region = PixelRect {
        x: width as i32 - 10,
        y: -10,
        width: 30,
        height: 30,
    };
    let (w, h, pixels) = paint_scene_region_to_rgba(&scene, region, None, None).unwrap();
    assert_eq!((w, h), (30, 30));
    // The top-left corner is above the page, the bottom-right past its edge.
    assert_eq!(&pixels[..4], &[0, 0, 0, 0]);
    let last = pixels.len() - 4;
    assert_eq!(&pixels[last..], &[0, 0, 0, 0]);
}

#[test]
fn a_region_wholly_off_the_page_is_empty() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/transforms.guix");
    let scene = scene_of(&path, 1.0);
    let region = PixelRect {
        x: -500,
        y: -500,
        width: 20,
        height: 20,
    };
    let (_, _, pixels) = paint_scene_region_to_rgba(&scene, region, None, None).unwrap();
    assert!(pixels.iter().all(|byte| *byte == 0));
}
