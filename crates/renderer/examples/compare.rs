//! Compares this renderer's output against the kit HTML renderer.
//!
//! ```text
//! cargo run -p dotgui-renderer --example compare                  # whole corpus
//! cargo run -p dotgui-renderer --example compare examples/foo.gui # one document
//! ```
//!
//! kit is the behavioural reference, so a disagreement is a lead worth
//! following. It is **not** a verdict: kit has repeatedly disagreed with the
//! spec, and either side can be the wrong one. What this tool produces is a
//! ranked worklist, and each entry still has to be adjudicated against
//! `spec/spec.json` by hand.
//!
//! **This is a local tool, not a CI gate.** It shells out to a kit checkout,
//! which needs Chromium and a built render bundle, and most example packages
//! pull their icons over the network. Those are the same reasons the goldens
//! are `#[ignore]`d — see `tests/golden_tests.rs`.
//!
//! # What it measures
//!
//! Two rasterisers do not agree on glyph pixels, so a raw pixel diff is
//! swamped by antialiasing on every row that holds text. The signal that stays
//! clean is **geometry**: if one renderer makes a box five pixels taller, every
//! row below it shifts, and the two images stop lining up.
//!
//! So rows are reduced to a coarse signature (mean luminance plus an edge
//! count) and the two sequences are diffed the way `diff` diffs lines. Rows
//! present on one side only are where a layout divergence was *introduced*;
//! everything after realigns. That is what gets ranked.
//!
//! Pixel difference over the rows that did line up is reported too, but as a
//! secondary number, because it includes rasterisation differences this
//! renderer has no intention of matching.

use dotgui_renderer::{
    build_scene, compute_taffy_layout_with_text, paint_scene_to_png_bytes, parse_gui_xml,
    read_gui_package, AssetCache, FontAxes, FontStore, GuiDocument, LayoutBox, TextMeasurer,
};
use std::{
    collections::BTreeMap,
    env, fmt, fs,
    path::{Path, PathBuf},
    process::{self, Command},
};

/// Mean-luminance difference, in levels, still considered the same row.
const LUM_TOL: u16 = 6;

/// Difference in edge count still considered the same row.
const EDGE_TOL: u16 = 3;

/// Luminance step counted as an edge when signing a row.
const EDGE_THRESHOLD: i16 = 24;

/// Per-channel difference written off as rasteriser disagreement.
///
/// Deliberately looser than the goldens' tolerance of 2. That compares this
/// renderer against itself; this compares two different glyph rasterisers.
const CHANNEL_TOLERANCE: u8 = 16;

/// Above this, the quadratic row diff is not worth the memory.
const MAX_DIFFABLE_ROWS: usize = 6000;

fn main() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crate sits two levels below the repository root")
        .to_path_buf();

    let args: Vec<String> = env::args().skip(1).collect();

    if args.first().is_some_and(|arg| arg == "--boxes") {
        let Some(input) = args.get(1) else {
            eprintln!("usage: --boxes <document>");
            process::exit(2);
        };
        match diff_boxes(&root, Path::new(input)) {
            Ok(()) => return,
            Err(message) => {
                eprintln!("{message}");
                process::exit(1);
            }
        }
    }

    let inputs = if args.is_empty() {
        corpus(&root)
    } else {
        args.iter().map(PathBuf::from).collect()
    };

    if inputs.is_empty() {
        eprintln!("no documents to compare");
        process::exit(1);
    }

    let mut reports = Vec::new();
    for input in &inputs {
        eprint!("comparing {} ... ", short(&root, input));
        match compare(&root, input) {
            Ok(report) => {
                eprintln!("{}", report.headline());
                reports.push(report);
            }
            Err(Failure::KitUnavailable(message)) => {
                eprintln!("\n{message}");
                process::exit(3);
            }
            Err(Failure::Skipped(reason)) => eprintln!("skipped: {reason}"),
        }
    }

    print_report(&root, &mut reports);
}

/// Every example package and hand-written fixture, in that order.
fn corpus(root: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for (dir, extension) in [
        (root.join("examples"), "gui"),
        (root.join("crates/renderer/tests/fixtures"), "guix"),
    ] {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        let mut group: Vec<PathBuf> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == extension))
            .collect();
        group.sort();
        paths.extend(group);
    }
    paths
}

fn short(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .display()
        .to_string()
}

enum Failure {
    /// kit cannot run on this machine at all; nothing further will work.
    KitUnavailable(String),
    /// This one document could not be compared.
    Skipped(String),
}

// ─── rendering both sides ────────────────────────────────────────────────────

fn compare(root: &Path, input: &Path) -> Result<Report, Failure> {
    let native = layout_native(input).map_err(Failure::Skipped)?;
    let kit = render_kit(root, input)?;

    let scene = build_scene(&native.document, &native.layout);
    let ours = paint_scene_to_png_bytes(&scene, Some(&native.cache), Some(&native.fonts))
        .map_err(|err| Failure::Skipped(err.to_string()))?;

    let fonts = compare_fonts(&native, &kit);
    let ours = Bitmap::decode(&ours).map_err(Failure::Skipped)?;
    let theirs = Bitmap::decode(&kit.png).map_err(Failure::Skipped)?;

    Ok(Report::new(input.to_path_buf(), theirs, ours, fonts))
}

/// A document laid out exactly as `render_png` does, with real font metrics.
struct NativeLayout {
    document: GuiDocument,
    layout: LayoutBox,
    cache: AssetCache,
    fonts: FontStore,
}

fn layout_native(input: &Path) -> Result<NativeLayout, String> {
    let bytes = fs::read(input).map_err(|err| format!("cannot read {}: {err}", input.display()))?;

    let (xml, assets) =
        if input.extension().is_some_and(|ext| ext == "gui") || bytes.starts_with(b"PK\x03\x04") {
            let package = read_gui_package(&bytes).map_err(|err| err.to_string())?;
            (package.xml, package.assets)
        } else {
            let xml = String::from_utf8(bytes).map_err(|err| err.to_string())?;
            (xml, Default::default())
        };

    let document = parse_gui_xml(&xml).map_err(|err| err.to_string())?;
    let cache = AssetCache::new(".gui-render/cache").with_package_assets(assets);
    let fonts = FontStore::from_document(&document, &cache).unwrap_or_default();
    let layout =
        compute_taffy_layout_with_text(&document, &fonts).map_err(|err| err.to_string())?;
    Ok(NativeLayout {
        document,
        layout,
        cache,
        fonts,
    })
}

/// kit's rendering of a document, and what its text actually measured.
struct KitRender {
    png: Vec<u8>,
    /// Per declared family, the width kit got for the probe string.
    fonts: BTreeMap<String, KitFont>,
    probe: FontProbe,
}

#[derive(serde::Deserialize, Default)]
struct FontProbe {
    text: String,
    size: f32,
}

#[derive(serde::Deserialize)]
struct KitFont {
    /// Whether kit matched the family by name, as opposed to falling back.
    ///
    /// Reported because it is worth knowing, and deliberately *not* used to
    /// decide comparability: on macOS `system-ui` lands on SF Pro, so an
    /// `SF Pro Display` document renders in SF Pro either way. Judging by name
    /// wrote off five documents that agree with kit to the pixel.
    #[serde(rename = "resolvedByName")]
    resolved_by_name: bool,
    width: f32,
}

fn render_kit(root: &Path, input: &Path) -> Result<KitRender, Failure> {
    let output = env::temp_dir().join("dotgui-compare-kit.png");
    let _ = fs::remove_file(&output);

    let script = root.join("tools/kit-rasterize.ts");
    let result = Command::new("bun")
        .arg("run")
        .arg(&script)
        .arg(input)
        .arg(&output)
        .output();

    let result = match result {
        Ok(result) => result,
        Err(err) => {
            return Err(Failure::KitUnavailable(format!(
                "cannot run `bun`: {err}\n\
                 the comparison needs bun and a dotgui/kit checkout; \
                 install bun from https://bun.sh"
            )))
        }
    };

    if !result.status.success() {
        let message = String::from_utf8_lossy(&result.stderr).trim().to_owned();
        // Exit 3 is the bridge saying kit itself cannot run here — no browser,
        // no render bundle, no checkout. Retrying other documents is pointless.
        if result.status.code() == Some(3) {
            return Err(Failure::KitUnavailable(message));
        }
        return Err(Failure::Skipped(message));
    }

    let png =
        fs::read(&output).map_err(|err| Failure::Skipped(format!("kit wrote no image: {err}")))?;

    #[derive(serde::Deserialize, Default)]
    struct Dump {
        #[serde(default)]
        fonts: BTreeMap<String, KitFont>,
        #[serde(default)]
        probe: FontProbe,
    }

    // A parse failure means no font notes rather than no comparison — the
    // images are the point.
    let dump: Dump = serde_json::from_slice(&result.stdout).unwrap_or_default();

    Ok(KitRender {
        png,
        fonts: dump.fonts,
        probe: dump.probe,
    })
}

/// How far apart two renderers' text may measure and still be called the same
/// face.
///
/// Chosen from the corpus rather than picked: families both renderers resolve
/// by name land between 0.00% and 3.13%, and the one genuinely substituted
/// face — `SF Mono`, which kit falls back to a proportional stack for — is
/// 14.96% out. Nothing sits in between.
const SAME_FACE_TOLERANCE: f32 = 0.05;

/// What each declared family measured on both sides.
struct FontReading {
    family: String,
    resolved_by_name: bool,
    difference: f32,
}

impl FontReading {
    /// Whether kit drew a materially different typeface.
    fn substituted(&self) -> bool {
        self.difference.abs() > SAME_FACE_TOLERANCE
    }
}

/// Measures the probe string through this renderer and compares with kit's.
fn compare_fonts(native: &NativeLayout, kit: &KitRender) -> Vec<FontReading> {
    if kit.probe.text.is_empty() || kit.probe.size <= 0.0 {
        return Vec::new();
    }
    let axes = FontAxes::from_style(None, None, kit.probe.size);

    native
        .document
        .metadata
        .fonts
        .iter()
        .filter_map(|(family, info)| {
            let reading = kit.fonts.get(family)?;
            // Measured at a weight the document actually declares. Asking for
            // 400 from a family that only ships 500 and 700 gets the
            // character-count estimate instead of a face, and comparing kit
            // against an estimate says nothing — that is what let `SF Mono`,
            // 15% out, pass for the same typeface.
            let weight = info
                .weights
                .as_deref()
                .and_then(|weights| weights.split_whitespace().next())
                .unwrap_or("400");
            let ours = native.fonts.text_width(
                &kit.probe.text,
                Some(family),
                Some(weight),
                None,
                kit.probe.size,
                &axes,
            );
            if ours <= 0.0 {
                return None;
            }
            Some(FontReading {
                family: family.clone(),
                resolved_by_name: reading.resolved_by_name,
                difference: (reading.width - ours) / ours,
            })
        })
        .collect()
}

// ─── box diff ────────────────────────────────────────────────────────────────

/// One node's box, in the flat pre-order both renderers agree on.
#[derive(serde::Deserialize)]
struct Box2D {
    tag: String,
    depth: usize,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

/// Names the element behind a divergence, rather than the pixel row.
///
/// The row diff says a document went out of alignment at y≈331; this says
/// which box did it. That is the difference between knowing there is a bug and
/// knowing where it is — `<line>` sitting at h=0 against kit's h=1, three
/// times over, is what #58 turned out to be.
///
/// Both trees are walked pre-order and paired by position. They line up
/// because the two renderers build a node per element; when they do not, the
/// counts differ and that is itself the finding, so it is reported rather than
/// guessed around.
fn diff_boxes(root: &Path, input: &Path) -> Result<(), String> {
    let native = layout_native(input)?;
    let mut ours = Vec::new();
    flatten(&native.layout, 0, &mut ours);

    let theirs = kit_boxes(root, input)?;

    println!("{}", short(root, input));
    println!("kit {} boxes, ours {}", theirs.len(), ours.len());

    if theirs.len() != ours.len() {
        println!();
        println!("the two trees have different shapes, so boxes cannot be paired up.");
        println!("that is the finding: one renderer built nodes the other did not.");
        print_tag_counts(&theirs, &ours);
        return Ok(());
    }

    println!();
    println!("boxes whose size differs (the causes; everything below one is displaced)");
    println!("{}", "-".repeat(92));
    let mut found = false;
    for (index, (kit, our)) in theirs.iter().zip(&ours).enumerate() {
        let height = our.h - kit.h;
        let width = our.w - kit.w;
        if height.abs() < 0.01 && width.abs() < 0.01 {
            continue;
        }
        found = true;
        let indent = "  ".repeat(our.depth.min(12));
        // Our y, so a finding here lines up with the row diff's drift points.
        print!("  #{index:<4} y≈{:<7.0} {indent}<{}>", our.y, our.tag);
        if height.abs() >= 0.01 {
            print!("  h {} vs {} ({height:+.3})", kit.h, our.h);
        }
        if width.abs() >= 0.01 {
            print!(
                "  w {} vs {} ({width:+.3}) at x {} vs {}",
                kit.w, our.w, kit.x, our.x
            );
        }
        println!();
    }
    if !found {
        println!("  none — every box is the same size in both renderers");
    }

    Ok(())
}

fn flatten(node: &LayoutBox, depth: usize, out: &mut Vec<Box2D>) {
    // `<segment>` and `<appearance>` ride in the box tree without geometry, so
    // the scene builder can reach them — see `read_children`. They are not
    // boxes and kit has no counterpart for them, so including them made the
    // two trees different shapes and the diff refused to pair anything at all.
    // Any document with an `<appearance>` block hit this.
    if node.tag == "segment" || node.tag == "appearance" {
        return;
    }

    out.push(Box2D {
        tag: node.tag.clone(),
        depth,
        x: node.rect.x,
        y: node.rect.y,
        w: node.rect.width,
        h: node.rect.height,
    });
    for child in &node.children {
        flatten(child, depth + 1, out);
    }
}

fn print_tag_counts(theirs: &[Box2D], ours: &[Box2D]) {
    let count = |boxes: &[Box2D]| {
        let mut counts: BTreeMap<String, usize> = Default::default();
        for item in boxes {
            *counts.entry(item.tag.clone()).or_default() += 1;
        }
        counts
    };
    println!();
    println!("  kit : {:?}", count(theirs));
    println!("  ours: {:?}", count(ours));
    println!();
    println!("note that kit spells some elements differently: a `<rect>`, `<ellipse>`");
    println!("or `<line>` all arrive as `shape`.");
}

fn kit_boxes(root: &Path, input: &Path) -> Result<Vec<Box2D>, String> {
    let output = Command::new("bun")
        .arg("run")
        .arg(root.join("tools/kit-rasterize.ts"))
        .arg("--boxes")
        .arg(input)
        .output()
        .map_err(|err| format!("cannot run `bun`: {err}"))?;

    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
    }

    #[derive(serde::Deserialize)]
    struct Dump {
        boxes: Vec<Box2D>,
    }

    serde_json::from_slice::<Dump>(&output.stdout)
        .map(|dump| dump.boxes)
        .map_err(|err| format!("cannot read kit's box dump: {err}"))
}

// ─── bitmaps ─────────────────────────────────────────────────────────────────

/// An opaque RGB bitmap.
///
/// kit screenshots with `omitBackground`, so its PNGs carry alpha that this
/// renderer's do not. Both sides are flattened onto white so that difference
/// does not read as a content difference.
struct Bitmap {
    width: u32,
    height: u32,
    pixels: Vec<[u8; 3]>,
}

impl Bitmap {
    fn decode(bytes: &[u8]) -> Result<Self, String> {
        let image = image::load_from_memory(bytes)
            .map_err(|err| format!("cannot decode png: {err}"))?
            .to_rgba8();
        let (width, height) = image.dimensions();

        let pixels = image
            .pixels()
            .map(|pixel| {
                let [r, g, b, a] = pixel.0;
                let over_white = |channel: u8| {
                    let channel = channel as u32 * a as u32 + 255 * (255 - a as u32);
                    (channel / 255) as u8
                };
                [over_white(r), over_white(g), over_white(b)]
            })
            .collect();

        Ok(Self {
            width,
            height,
            pixels,
        })
    }

    fn at(&self, x: u32, y: u32) -> [u8; 3] {
        self.pixels[(y * self.width + x) as usize]
    }

    /// A coarse per-row descriptor: mean luminance, and how many horizontal
    /// steps in the row are big enough to be an edge.
    ///
    /// Both are chosen to survive a different glyph rasteriser but to move when
    /// the row holds different content.
    fn signatures(&self) -> Vec<RowSignature> {
        (0..self.height)
            .map(|y| {
                let mut total = 0_u32;
                let mut edges = 0_u16;
                let mut previous: Option<i16> = None;
                for x in 0..self.width {
                    let [r, g, b] = self.at(x, y);
                    let luminance = (r as u32 * 299 + g as u32 * 587 + b as u32 * 114) / 1000;
                    total += luminance;
                    if let Some(previous) = previous {
                        if (luminance as i16 - previous).abs() > EDGE_THRESHOLD {
                            edges += 1;
                        }
                    }
                    previous = Some(luminance as i16);
                }
                RowSignature {
                    luminance: (total / self.width.max(1)) as u16,
                    edges,
                }
            })
            .collect()
    }
}

#[derive(Clone, Copy)]
struct RowSignature {
    luminance: u16,
    edges: u16,
}

impl RowSignature {
    fn matches(self, other: Self) -> bool {
        self.luminance.abs_diff(other.luminance) <= LUM_TOL
            && self.edges.abs_diff(other.edges) <= EDGE_TOL
    }
}

// ─── row diff ────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq)]
enum Step {
    Both(u32, u32),
    OnlyKit(u32),
    OnlyOurs(u32),
}

/// Longest common subsequence over row signatures.
///
/// This is `diff`, with rows for lines and a tolerance for equality. Rows it
/// cannot pair up are rows one renderer produced and the other did not, which
/// is precisely where a layout divergence starts.
fn diff_rows(kit: &[RowSignature], ours: &[RowSignature]) -> Vec<Step> {
    let (n, m) = (kit.len(), ours.len());
    let mut table = vec![0_u32; (n + 1) * (m + 1)];
    let index = |i: usize, j: usize| i * (m + 1) + j;

    for i in (0..n).rev() {
        for j in (0..m).rev() {
            table[index(i, j)] = if kit[i].matches(ours[j]) {
                table[index(i + 1, j + 1)] + 1
            } else {
                table[index(i + 1, j)].max(table[index(i, j + 1)])
            };
        }
    }

    let mut steps = Vec::with_capacity(n.max(m));
    let (mut i, mut j) = (0, 0);
    while i < n && j < m {
        if kit[i].matches(ours[j]) {
            steps.push(Step::Both(i as u32, j as u32));
            i += 1;
            j += 1;
        } else if table[index(i + 1, j)] >= table[index(i, j + 1)] {
            steps.push(Step::OnlyKit(i as u32));
            i += 1;
        } else {
            steps.push(Step::OnlyOurs(j as u32));
            j += 1;
        }
    }
    steps.extend((i..n).map(|i| Step::OnlyKit(i as u32)));
    steps.extend((j..m).map(|j| Step::OnlyOurs(j as u32)));
    steps
}

/// A stretch of rows over which the two images hold a constant vertical offset.
///
/// A row the diff could not pair up is not by itself interesting: two glyph
/// rasterisers routinely disagree enough on one row of text that it fails to
/// match, and the row after it lines up again. What matters is whether the
/// alignment *stays* moved — that is a box with a different height, and
/// everything below it is displaced.
struct Band {
    /// Rows our render sits below kit's over this stretch.
    offset: i64,
    kit_start: u32,
    rows: u32,
}

/// Rows a band must hold for its offset to count as the real alignment rather
/// than a pair of rows the diff happened to mismatch.
const MIN_BAND_ROWS: u32 = 8;

/// The offset at each matched row, coalesced into stable bands.
fn bands(steps: &[Step]) -> Vec<Band> {
    let mut bands: Vec<Band> = Vec::new();
    for step in steps {
        let Step::Both(kit_y, our_y) = *step else {
            continue;
        };
        let offset = our_y as i64 - kit_y as i64;
        match bands.last_mut() {
            Some(band) if band.offset == offset => band.rows += 1,
            _ => bands.push(Band {
                offset,
                kit_start: kit_y,
                rows: 1,
            }),
        }
    }

    // Drop the flickers, then re-merge neighbours that agree once the noise
    // between them is gone.
    let mut stable: Vec<Band> = Vec::new();
    for band in bands.into_iter().filter(|band| band.rows >= MIN_BAND_ROWS) {
        match stable.last_mut() {
            Some(last) if last.offset == band.offset => last.rows += band.rows,
            _ => stable.push(band),
        }
    }
    stable
}

/// A point where the vertical alignment stepped and stayed stepped.
struct Drift {
    kit_y: u32,
    from: i64,
    to: i64,
}

impl fmt::Display for Drift {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let step = self.to - self.from;
        write!(
            formatter,
            "y≈{}: everything below shifts {step:+}px (offset {} → {})",
            self.kit_y, self.from, self.to
        )
    }
}

fn drifts(bands: &[Band]) -> Vec<Drift> {
    bands
        .windows(2)
        .map(|pair| Drift {
            kit_y: pair[1].kit_start,
            from: pair[0].offset,
            to: pair[1].offset,
        })
        .collect()
}

// ─── reporting ───────────────────────────────────────────────────────────────

struct Report {
    path: PathBuf,
    kit: (u32, u32),
    ours: (u32, u32),
    drifts: Vec<Drift>,
    /// Share of pixels differing beyond [`CHANNEL_TOLERANCE`] on rows that did
    /// line up. Includes glyph rasterisation, so it never reaches zero on text.
    residual: f64,
    too_large: bool,
    fonts: Vec<FontReading>,
}

impl Report {
    fn new(path: PathBuf, kit: Bitmap, ours: Bitmap, fonts: Vec<FontReading>) -> Self {
        let dimensions = ((kit.width, kit.height), (ours.width, ours.height));

        if kit.height as usize > MAX_DIFFABLE_ROWS || ours.height as usize > MAX_DIFFABLE_ROWS {
            return Self {
                path,
                kit: dimensions.0,
                ours: dimensions.1,
                drifts: Vec::new(),
                residual: 0.0,
                too_large: true,
                fonts,
            };
        }

        let steps = diff_rows(&kit.signatures(), &ours.signatures());

        // Compare only where both images have pixels; a width difference is
        // reported on its own rather than smeared across every row.
        let width = kit.width.min(ours.width);
        let (mut differing, mut total) = (0_u64, 0_u64);
        for step in &steps {
            let Step::Both(kit_y, our_y) = *step else {
                continue;
            };
            for x in 0..width {
                let theirs = kit.at(x, kit_y);
                let ours = ours.at(x, our_y);
                let apart = (0..3).any(|c| theirs[c].abs_diff(ours[c]) > CHANNEL_TOLERANCE);
                differing += u64::from(apart);
                total += 1;
            }
        }

        Self {
            path,
            kit: dimensions.0,
            ours: dimensions.1,
            drifts: drifts(&bands(&steps)),
            residual: if total == 0 {
                0.0
            } else {
                differing as f64 / total as f64
            },
            too_large: false,
            fonts,
        }
    }

    fn width_differs(&self) -> bool {
        self.kit.0 != self.ours.0
    }

    /// Whether kit drew a materially different typeface than this renderer.
    ///
    /// Decided by measuring both, not by whether kit matched the family name.
    /// A fallback can land on the same physical face — on macOS `system-ui`
    /// lands on SF Pro — and judging by name excluded five documents that
    /// agree with kit to the pixel.
    fn fonts_substituted(&self) -> bool {
        self.fonts.iter().any(FontReading::substituted)
    }

    /// Families worth mentioning: a different face, or the same one reached
    /// by a fallback, or metrics far enough apart to explain some drift.
    fn font_notes(&self) -> Vec<String> {
        self.fonts
            .iter()
            .filter(|reading| {
                reading.substituted()
                    || !reading.resolved_by_name
                    || reading.difference.abs() > 0.01
            })
            .map(|reading| {
                let how = if reading.substituted() {
                    "a different face"
                } else if !reading.resolved_by_name {
                    "the same face by fallback"
                } else {
                    "the same face"
                };
                format!(
                    "{} — {how}, text {:+.1}% against ours",
                    reading.family,
                    reading.difference * 100.0
                )
            })
            .collect()
    }

    fn height_delta(&self) -> i64 {
        self.ours.1 as i64 - self.kit.1 as i64
    }

    /// Total vertical movement, counting a step that later reverses.
    ///
    /// A document can end the same height as kit's and still have disagreed
    /// twice in the middle, so this is tracked apart from [`Self::height_delta`].
    fn drift_magnitude(&self) -> i64 {
        self.drifts
            .iter()
            .map(|drift| (drift.to - drift.from).abs())
            .sum()
    }

    fn agrees(&self) -> bool {
        !self.width_differs() && self.drifts.is_empty() && self.height_delta() == 0
    }

    fn headline(&self) -> String {
        if self.fonts_substituted() {
            let families: Vec<&str> = self
                .fonts
                .iter()
                .filter(|reading| reading.substituted())
                .map(|reading| reading.family.as_str())
                .collect();
            return format!("kit drew a different face for {}", families.join(", "));
        }
        if self.too_large {
            return "too tall to diff".to_owned();
        }
        if self.width_differs() {
            return format!("width {} vs {}", self.kit.0, self.ours.0);
        }
        if self.drifts.is_empty() {
            return format!(
                "aligned, {:+}px ({:.1}% pixels differ)",
                self.height_delta(),
                self.residual * 100.0
            );
        }
        format!(
            "{} drift point(s), {:+}px height",
            self.drifts.len(),
            self.height_delta()
        )
    }
}

fn print_report(root: &Path, reports: &mut [Report]) {
    // Width disagreement is the loudest signal, then the number of places the
    // layout stepped out of alignment, then whatever is left in the pixels.
    reports.sort_by(|a, b| {
        a.fonts_substituted()
            .cmp(&b.fonts_substituted())
            .then(b.width_differs().cmp(&a.width_differs()))
            .then(b.height_delta().abs().cmp(&a.height_delta().abs()))
            .then(b.drift_magnitude().cmp(&a.drift_magnitude()))
            .then(
                b.residual
                    .partial_cmp(&a.residual)
                    .unwrap_or(std::cmp::Ordering::Equal),
            )
    });

    println!();
    println!("kit is the reference, not the arbiter: adjudicate each of these");
    println!("against spec/spec.json before assuming the divergence is ours.");
    println!();
    println!(
        "{:<44} {:>11} {:>11} {:>7} {:>6} {:>8}",
        "document", "kit", "ours", "drifts", "Δh", "pixels"
    );
    println!("{}", "-".repeat(92));

    for report in reports.iter() {
        println!(
            "{:<44} {:>11} {:>11} {:>7} {:>6} {:>7.1}%",
            format!(
                "{}{}",
                short(root, &report.path),
                if report.fonts_substituted() {
                    "  [font]"
                } else {
                    ""
                }
            ),
            format!("{}x{}", report.kit.0, report.kit.1),
            format!("{}x{}", report.ours.0, report.ours.1),
            report.drifts.len(),
            format!("{:+}", report.height_delta()),
            report.residual * 100.0,
        );
    }

    println!();
    println!("where the geometry diverges");
    println!("{}", "-".repeat(92));
    let mut any = false;
    for report in reports
        .iter()
        .filter(|report| !report.agrees() && !report.fonts_substituted())
    {
        any = true;
        println!("{}", short(root, &report.path));
        if report.width_differs() {
            println!(
                "    width {} vs {} — the root box disagrees",
                report.kit.0, report.ours.0
            );
        }
        for drift in report.drifts.iter().take(8) {
            println!("    {drift}");
        }
        if report.drifts.len() > 8 {
            println!("    … and {} more", report.drifts.len() - 8);
        }
        // A height difference with no located step means the drift is below the
        // last row the two images still had in common.
        if report.drifts.is_empty() && report.height_delta() != 0 {
            println!(
                "    {:+}px overall, no located step — the divergence is past the last shared row",
                report.height_delta()
            );
        }
    }
    if !any {
        println!("    none — every document aligned row for row");
    }

    let noted: Vec<&Report> = reports
        .iter()
        .filter(|report| !report.fonts_substituted() && !report.font_notes().is_empty())
        .collect();
    if !noted.is_empty() {
        println!();
        println!("fonts worth knowing about, on documents that are still comparable");
        println!("{}", "-".repeat(92));
        for report in &noted {
            println!("{}", short(root, &report.path));
            for note in report.font_notes() {
                println!("    {note}");
            }
        }
    }

    let incomparable: Vec<&Report> = reports
        .iter()
        .filter(|report| report.fonts_substituted())
        .collect();
    if !incomparable.is_empty() {
        println!();
        println!("not comparable — kit drew a materially different typeface, so a string");
        println!("wraps somewhere else and every box that hugs one is a different size.");
        println!("Judged by measuring both, not by whether kit matched the family name:");
        println!("a fallback often lands on the same face, and on macOS it usually does.");
        println!("{}", "-".repeat(92));
        for report in &incomparable {
            println!("{}", short(root, &report.path));
            for note in report.font_notes() {
                println!("    {note}");
            }
        }
    }

    let comparable = reports.len() - incomparable.len();
    let agreeing = reports
        .iter()
        .filter(|report| report.agrees() && !report.fonts_substituted())
        .count();
    println!();
    println!("{agreeing}/{comparable} comparable documents align row for row");
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Rows spaced far enough apart in luminance to be individually
    /// recognisable, so the diff has something unambiguous to pair up.
    fn rows(luminances: impl IntoIterator<Item = u16>) -> Vec<RowSignature> {
        luminances
            .into_iter()
            .map(|luminance| RowSignature {
                luminance,
                edges: 0,
            })
            .collect()
    }

    fn distinct(count: u16) -> Vec<RowSignature> {
        rows((0..count).map(|row| row * 20))
    }

    fn analyse(kit: &[RowSignature], ours: &[RowSignature]) -> Vec<Drift> {
        drifts(&bands(&diff_rows(kit, ours)))
    }

    #[test]
    fn identical_documents_do_not_drift() {
        let document = distinct(40);
        assert!(analyse(&document, &document).is_empty());
    }

    #[test]
    fn a_taller_box_drifts_once_where_it_starts() {
        let kit = distinct(40);
        // Our render grows five rows at row 20; everything below it shifts.
        let mut ours = kit[..20].to_vec();
        ours.extend(rows([5000, 5020, 5040, 5060, 5080]));
        ours.extend_from_slice(&kit[20..]);

        let drifts = analyse(&kit, &ours);

        assert_eq!(
            drifts.len(),
            1,
            "one box changed height, so one drift point"
        );
        assert_eq!(drifts[0].kit_y, 20);
        assert_eq!(drifts[0].to - drifts[0].from, 5);
    }

    #[test]
    fn a_single_mismatched_row_is_not_a_drift() {
        // The noise case: two rasterisers disagree on one row of text, the row
        // after it lines up again, and nothing below has moved. Reporting that
        // as a layout divergence buries the real ones.
        let kit = distinct(40);
        let mut ours = kit.clone();
        ours[20].luminance = 9999;

        assert!(analyse(&kit, &ours).is_empty());
    }

    #[test]
    fn an_offset_that_does_not_hold_is_not_a_drift() {
        // Shifted for four rows, then back. Too short to be a box that
        // changed size; [`MIN_BAND_ROWS`] is what draws that line.
        let kit = distinct(40);
        let mut ours = kit[..20].to_vec();
        ours.extend(rows([5000, 5020, 5040]));
        ours.extend_from_slice(&kit[20..24]);
        ours.extend_from_slice(&kit[27..]);

        assert!(analyse(&kit, &ours).is_empty());
    }

    #[test]
    fn drift_magnitude_counts_a_step_that_later_reverses() {
        let kit = distinct(60);
        // Grows eight rows at row 20, gives them back at row 40.
        let mut ours = kit[..20].to_vec();
        ours.extend(rows((0..8).map(|row| 5000 + row * 20)));
        ours.extend_from_slice(&kit[20..40]);
        ours.extend_from_slice(&kit[48..]);

        let drifts = analyse(&kit, &ours);

        assert_eq!(drifts.len(), 2, "out of alignment and back is two points");
        let magnitude: i64 = drifts
            .iter()
            .map(|drift| (drift.to - drift.from).abs())
            .sum();
        assert_eq!(magnitude, 16, "eight rows out and eight back");
    }

    fn leaf(tag: &str, height: f32) -> LayoutBox {
        LayoutBox {
            tag: tag.to_owned(),
            attributes: Default::default(),
            text: None,
            rect: dotgui_renderer::LayoutRect {
                x: 0.0,
                y: 0.0,
                width: 10.0,
                height,
            },
            children: Vec::new(),
        }
    }

    #[test]
    fn flattening_walks_the_tree_in_document_order() {
        // Both sides are paired by position, so pre-order on this side has to
        // match the DOM walk on kit's. Depth rides along for the indent.
        let mut root = leaf("col", 100.0);
        let mut inner = leaf("row", 50.0);
        inner.children.push(leaf("text", 20.0));
        root.children.push(inner);
        root.children.push(leaf("rect", 10.0));

        let mut flat = Vec::new();
        flatten(&root, 0, &mut flat);

        let seen: Vec<(&str, usize)> = flat
            .iter()
            .map(|item| (item.tag.as_str(), item.depth))
            .collect();
        assert_eq!(seen, vec![("col", 0), ("row", 1), ("text", 2), ("rect", 1)]);
    }

    #[test]
    fn flattening_leaves_out_what_is_not_a_box() {
        // `<segment>` and `<appearance>` ride in the layout tree with no
        // geometry so the scene builder can reach them. kit has no counterpart
        // for either, so counting them made the two trees different shapes and
        // the diff refused to pair anything — on every document carrying an
        // `<appearance>` block, which is most of the paint-heavy ones.
        let mut root = leaf("rect", 100.0);
        let mut appearance = leaf("appearance", 0.0);
        appearance.children.push(leaf("fill", 0.0));
        root.children.push(appearance);
        root.children.push(leaf("segment", 0.0));
        root.children.push(leaf("text", 20.0));

        let mut flat = Vec::new();
        flatten(&root, 0, &mut flat);

        let seen: Vec<&str> = flat.iter().map(|item| item.tag.as_str()).collect();
        assert_eq!(
            seen,
            vec!["rect", "text"],
            "the paint description is not a box, and neither is its contents"
        );
    }

    #[test]
    fn kit_box_dumps_are_read_with_gui_prefixes_already_stripped() {
        // The bridge strips `gui-` and drops the inner <img>, so the two trees
        // pair up. Guarding the shape of that contract here, because a silent
        // change to it would show up as every box being misattributed.
        let dump = br#"{"boxes":[
            {"tag":"col","depth":0,"x":0,"y":0,"w":360,"h":841},
            {"tag":"text","depth":1,"x":16,"y":4,"w":23.6,"h":16}
        ]}"#;

        #[derive(serde::Deserialize)]
        struct Dump {
            boxes: Vec<Box2D>,
        }
        let parsed: Dump = serde_json::from_slice(dump).expect("dump parses");

        assert_eq!(parsed.boxes.len(), 2);
        assert_eq!(parsed.boxes[0].tag, "col");
        assert_eq!(parsed.boxes[1].h, 16.0);
    }

    fn reading(family: &str, resolved_by_name: bool, difference: f32) -> FontReading {
        FontReading {
            family: family.to_owned(),
            resolved_by_name,
            difference,
        }
    }

    fn with_fonts(fonts: Vec<FontReading>) -> Report {
        Report {
            path: PathBuf::from("probe.guix"),
            kit: (10, 10),
            ours: (10, 10),
            drifts: Vec::new(),
            residual: 0.0,
            too_large: false,
            fonts,
        }
    }

    #[test]
    fn a_fallback_onto_the_same_face_is_still_comparable() {
        // The case that wrongly excluded five documents: kit could not match
        // `SF Pro Display` by name, fell back through `system-ui`, and landed
        // on the same physical typeface. Measured 0.6% apart.
        let report = with_fonts(vec![reading("SF Pro Display", false, 0.006)]);

        assert!(
            !report.fonts_substituted(),
            "same metrics means same face, whatever kit called it"
        );
        assert_eq!(
            report.font_notes(),
            vec!["SF Pro Display — the same face by fallback, text +0.6% against ours"],
            "still worth saying, just not grounds for discarding the document"
        );
    }

    #[test]
    fn a_fallback_onto_a_different_face_is_not() {
        // `SF Mono` has no fallback that is monospaced, so kit draws it in a
        // proportional face and the text is 15% narrower.
        let report = with_fonts(vec![reading("SF Mono", false, -0.15)]);

        assert!(report.fonts_substituted());
        assert!(report.headline().contains("SF Mono"));
    }

    #[test]
    fn matching_by_name_does_not_prove_the_same_metrics() {
        // Geist resolves by name on both sides and still measures 3.1% apart,
        // because kit gets a variable webfont and this renderer gets a static
        // instance. Comparable, but worth a note.
        let report = with_fonts(vec![reading("Geist", true, -0.031)]);

        assert!(!report.fonts_substituted());
        assert_eq!(
            report.font_notes(),
            vec!["Geist — the same face, text -3.1% against ours"]
        );
    }

    #[test]
    fn a_face_that_measures_the_same_is_not_worth_mentioning() {
        let report = with_fonts(vec![reading("Georgia", true, 0.0)]);

        assert!(!report.fonts_substituted());
        assert!(report.font_notes().is_empty());
    }

    #[test]
    fn the_bridges_font_reading_is_read_as_declared() {
        // Guards the shape of the contract with `tools/kit-rasterize.ts`.
        let parsed: BTreeMap<String, KitFont> =
            serde_json::from_slice(br#"{"SF Mono":{"resolvedByName":false,"width":353.281}}"#)
                .expect("reading parses");

        assert_eq!(parsed["SF Mono"].width, 353.281);
        assert!(!parsed["SF Mono"].resolved_by_name);
    }

    #[test]
    fn white_shows_through_where_kit_screenshots_transparency() {
        // kit screenshots with `omitBackground`; this renderer's PNGs are
        // opaque. Flattening both is what stops that reading as content.
        let transparent = image::RgbaImage::from_pixel(1, 1, image::Rgba([0, 0, 0, 0]));
        let mut bytes = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(transparent)
            .write_to(&mut bytes, image::ImageFormat::Png)
            .expect("png encodes");

        let bitmap = Bitmap::decode(&bytes.into_inner()).expect("png decodes");

        assert_eq!(bitmap.at(0, 0), [255, 255, 255]);
    }
}
