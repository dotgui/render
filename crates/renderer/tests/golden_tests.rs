use std::{fs, path::PathBuf};
use dotgui_renderer::{
    build_scene, compute_taffy_layout, paint_scene_to_png_bytes, parse_gui_xml,
    read_gui_package, AssetCache, FontStore,
};

fn run_golden_test(gui_filename: &str, golden_name: &str) {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let root_dir = manifest_dir.parent().unwrap().parent().unwrap();
    let gui_path = root_dir.join("examples").join(gui_filename);

    if !gui_path.exists() {
        return;
    }

    let bytes = fs::read(&gui_path).expect("failed to read gui file");

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
    let fonts =
        FontStore::from_document(&document, &cache).unwrap_or_else(|_| FontStore::default());
    let layout = compute_taffy_layout(&document).expect("failed to compute layout");
    let scene = build_scene(&document, &layout);

    let generated_png = paint_scene_to_png_bytes(&scene, Some(&cache), Some(&fonts))
        .expect("failed to paint scene to png bytes");

    let goldens_dir = manifest_dir.join("tests").join("goldens");
    if !goldens_dir.exists() {
        fs::create_dir_all(&goldens_dir).expect("failed to create goldens dir");
    }
    let golden_path = goldens_dir.join(format!("{}.png", golden_name));

    if std::env::var("UPDATE_GOLDENS").is_ok() || !golden_path.exists() {
        fs::write(&golden_path, &generated_png).expect("failed to write golden png");
        println!("Updated golden reference for {}", golden_name);
        return;
    }

    let golden_bytes = fs::read(&golden_path).expect("failed to read golden image");

    let generated_img = image::load_from_memory(&generated_png)
        .expect("failed to decode generated image")
        .to_rgba8();
    let golden_img = image::load_from_memory(&golden_bytes)
        .expect("failed to decode golden image")
        .to_rgba8();

    assert_eq!(
        generated_img.dimensions(),
        golden_img.dimensions(),
        "dimension mismatch for {}",
        golden_name
    );

    let diff = generated_img
        .as_raw()
        .iter()
        .zip(golden_img.as_raw().iter())
        .filter(|(a, b)| a != b)
        .count();

    assert_eq!(
        diff, 0,
        "visual regression detected for {}: {} pixels differ",
        golden_name, diff
    );
}

#[test]
fn test_golden_checkout() {
    run_golden_test("anvil-ash-checkout.gui", "checkout");
}

#[test]
fn test_golden_arcade() {
    run_golden_test("arcade-return-item-android.gui", "arcade");
}

#[test]
fn test_golden_harbor() {
    run_golden_test("harbor-report-post-ios.gui", "harbor");
}

#[test]
fn test_golden_grain() {
    run_golden_test("grain-photo-adjust.gui", "grain");
}
