//! Visual regression tests against committed reference PNGs.
//!
//! These run the same path as `cargo run --example render_png`: real font
//! metrics for layout, then painting.
//!
//! **They are a local tool, not a CI gate**, and are ignored by default. Every
//! one of them depends on something the repository does not control:
//!
//! - `grain` and `harbor` declare `source="system"` fonts. The host's copy of
//!   `SF Pro Display` changes with the OS version and cannot be redistributed.
//! - `checkout` and `arcade` resolve Google fonts through the GitHub API, and
//!   the examples pull icons from a remote host. Both are unauthenticated and
//!   rate limited; CI has hit `403` mid-run.
//!
//! Nothing is left unchecked by this. Geometry for every example is covered on
//! every platform by `layout_snapshots.rs`, which needs no fonts and no
//! network, and painting is covered by the hermetic pixel tests in
//! `crate::paint`. What the goldens add is a full-page visual reference, which
//! is worth looking at by hand and not worth a red build.
//!
//! ```text
//! cargo test -p dotgui-renderer --test golden_tests -- --ignored
//! UPDATE_GOLDENS=1 cargo test -p dotgui-renderer --test golden_tests -- --ignored
//! ```

use dotgui_renderer::{
    build_scene, compute_taffy_layout_with_text, paint_scene_to_png_bytes, parse_gui_xml,
    read_gui_package, AssetCache, FontStore,
};
use std::{
    fs,
    path::{Path, PathBuf},
};

/// Per-channel difference written off as encoder/antialiasing jitter.
const CHANNEL_TOLERANCE: u8 = 2;

/// Share of pixels allowed to exceed [`CHANNEL_TOLERANCE`] before the test fails.
const MAX_DIFFERING_FRACTION: f64 = 0.001;

fn run_golden_test(gui_filename: &str, golden_name: &str) {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let root_dir = manifest_dir.parent().unwrap().parent().unwrap();
    // Example packages and the hand-written fixtures are both valid inputs:
    // the fixtures are the only full-page reference for features no example
    // uses, which is most of what has been added recently.
    let example_path = root_dir.join("examples").join(gui_filename);
    let gui_path = if example_path.exists() {
        example_path
    } else {
        manifest_dir
            .join("tests")
            .join("fixtures")
            .join(gui_filename)
    };

    let bytes = fs::read(&gui_path).unwrap_or_else(|err| {
        panic!(
            "golden input {} is missing ({err}); the goldens cannot verify anything without it",
            gui_path.display()
        )
    });

    let (xml, package_assets) = if gui_path.extension().is_some_and(|ext| ext == "gui")
        || bytes.starts_with(b"PK\x03\x04")
    {
        let package = read_gui_package(&bytes).expect("failed to open packaged assets");
        (package.xml, package.assets)
    } else {
        let xml = String::from_utf8(bytes).expect("invalid utf8 xml");
        (xml, Default::default())
    };

    let document = parse_gui_xml(&xml).expect("failed to parse gui xml");
    let cache =
        AssetCache::new(root_dir.join(".gui-render/cache")).with_package_assets(package_assets);
    let fonts = FontStore::from_document(&document, &cache)
        .unwrap_or_else(|err| panic!("{golden_name} declares fonts that did not resolve: {err}"));
    assert!(
        document.metadata.fonts.is_empty() || !fonts.is_empty(),
        "{golden_name} declares fonts but none resolved; the render would silently fall back"
    );

    let goldens_dir = manifest_dir.join("tests").join("goldens");
    check_font_fingerprints(&goldens_dir, golden_name, &fonts);

    let layout =
        compute_taffy_layout_with_text(&document, &fonts).expect("failed to compute layout");
    let scene = build_scene(&document, &layout);
    let generated_png = paint_scene_to_png_bytes(&scene, Some(&cache), Some(&fonts))
        .expect("failed to paint scene to png bytes");

    let golden_path = goldens_dir.join(format!("{golden_name}.png"));

    if std::env::var_os("UPDATE_GOLDENS").is_some() {
        fs::create_dir_all(&goldens_dir).expect("failed to create goldens dir");
        fs::write(&golden_path, &generated_png).expect("failed to write golden png");
        println!("updated golden reference for {golden_name}");
        return;
    }

    let golden_bytes = fs::read(&golden_path).unwrap_or_else(|err| {
        panic!(
            "golden reference {} is missing ({err}); regenerate with \
             UPDATE_GOLDENS=1 cargo test -p dotgui-renderer --test golden_tests",
            golden_path.display()
        )
    });

    let generated_img = image::load_from_memory(&generated_png)
        .expect("failed to decode generated image")
        .to_rgba8();
    let golden_img = image::load_from_memory(&golden_bytes)
        .expect("failed to decode golden image")
        .to_rgba8();

    if generated_img.dimensions() != golden_img.dimensions() {
        let actual_path = write_actual(&goldens_dir, golden_name, &generated_png);
        panic!(
            "dimension mismatch for {golden_name}: rendered {:?}, golden {:?} (rendered output written to {})",
            generated_img.dimensions(),
            golden_img.dimensions(),
            actual_path.display()
        );
    }

    let differing = generated_img
        .as_raw()
        .as_chunks::<4>()
        .0
        .iter()
        .zip(golden_img.as_raw().as_chunks::<4>().0)
        .filter(|(rendered, golden)| {
            rendered
                .iter()
                .zip(golden.iter())
                .any(|(a, b)| a.abs_diff(*b) > CHANNEL_TOLERANCE)
        })
        .count();

    let total = (generated_img.width() * generated_img.height()) as f64;
    let fraction = differing as f64 / total;
    if fraction > MAX_DIFFERING_FRACTION {
        let actual_path = write_actual(&goldens_dir, golden_name, &generated_png);
        panic!(
            "visual regression in {golden_name}: {differing} of {total} pixels differ \
             ({:.3}% > {:.3}% allowed); rendered output written to {}",
            fraction * 100.0,
            MAX_DIFFERING_FRACTION * 100.0,
            actual_path.display()
        );
    }
}

/// Compares the fonts this host resolved against the ones the golden was
/// generated with, recorded in `<name>.fonts`.
///
/// Without this a newer macOS SF Pro, or a re-fetched Google face, surfaces as
/// an unexplained pixel diff. Here it says which face moved.
fn check_font_fingerprints(goldens_dir: &Path, golden_name: &str, fonts: &FontStore) {
    let lock_path = goldens_dir.join(format!("{golden_name}.fonts"));
    let actual = fonts
        .fingerprints()
        .into_iter()
        .map(|(key, fingerprint)| format!("{key} {fingerprint}"))
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";

    if std::env::var_os("UPDATE_GOLDENS").is_some() {
        fs::create_dir_all(goldens_dir).expect("failed to create goldens dir");
        fs::write(&lock_path, &actual).expect("failed to write font lock");
        return;
    }

    let expected = fs::read_to_string(&lock_path).unwrap_or_else(|err| {
        panic!(
            "font lock {} is missing ({err}); regenerate the goldens with \
             UPDATE_GOLDENS=1 cargo test -p dotgui-renderer --test golden_tests",
            lock_path.display()
        )
    });

    assert_eq!(
        expected.trim(),
        actual.trim(),
        "{golden_name} resolved different fonts than the golden was generated with. \
         This host has a different version of a declared family, so a pixel diff \
         below would be about fonts, not about the renderer. Review, then regenerate \
         with UPDATE_GOLDENS=1 cargo test -p dotgui-renderer --test golden_tests"
    );
}

/// Drops the rendered image beside the golden so a failure can be eyeballed.
fn write_actual(goldens_dir: &Path, golden_name: &str, png: &[u8]) -> PathBuf {
    let path = goldens_dir.join(format!("{golden_name}.actual.png"));
    let _ = fs::write(&path, png);
    path
}

macro_rules! golden_test {
    ($name:ident, $file:expr, $golden:expr) => {
        #[test]
        #[ignore = "needs host fonts and network-resolved assets; run with --ignored"]
        fn $name() {
            run_golden_test($file, $golden);
        }
    };
}

golden_test!(golden_checkout, "anvil-ash-checkout.gui", "checkout");
golden_test!(golden_arcade, "arcade-return-item-android.gui", "arcade");
golden_test!(golden_harbor, "harbor-report-post-ios.gui", "harbor");
golden_test!(golden_grain, "grain-photo-adjust.gui", "grain");

// Fixtures: a full-page reference for the features no example package uses.
golden_test!(golden_gradients, "gradients.guix", "gradients");
golden_test!(golden_compositing, "compositing.guix", "compositing");
golden_test!(golden_masks, "masks-clipping.guix", "masks-clipping");
golden_test!(golden_opacity, "opacity-groups.guix", "opacity-groups");
golden_test!(golden_transforms, "transforms.guix", "transforms");
golden_test!(golden_layer_blur, "layer-blur.guix", "layer-blur");
golden_test!(
    golden_text_decorations,
    "text-decorations.guix",
    "text-decorations"
);
golden_test!(
    golden_line_height_normal,
    "line-height-normal.guix",
    "line-height-normal"
);
golden_test!(golden_text_case, "text-case.guix", "text-case");
