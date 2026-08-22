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
    read_gui_package, AssetCache, FontStore,
};
use std::{
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
    let ours = render_native(input).map_err(Failure::Skipped)?;
    let KitRender { png, substituted } = render_kit(root, input)?;

    let ours = Bitmap::decode(&ours).map_err(Failure::Skipped)?;
    let theirs = Bitmap::decode(&png).map_err(Failure::Skipped)?;

    Ok(Report::new(input.to_path_buf(), theirs, ours, substituted))
}

fn render_native(input: &Path) -> Result<Vec<u8>, String> {
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
    let scene = build_scene(&document, &layout);
    paint_scene_to_png_bytes(&scene, Some(&cache), Some(&fonts)).map_err(|err| err.to_string())
}

/// kit's rendering of a document, and which of its fonts kit had to fake.
struct KitRender {
    png: Vec<u8>,
    /// Declared families this browser had no real font for. When this is not
    /// empty the two renderers drew different typefaces, so their geometry is
    /// not comparable and the numbers below are about fonts, not layout.
    substituted: Vec<String>,
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

    Ok(KitRender {
        png,
        substituted: substituted_fonts(&result.stdout),
    })
}

/// Reads the bridge's one line of JSON. A parse failure means no warning
/// rather than no comparison — the images are the point.
fn substituted_fonts(stdout: &[u8]) -> Vec<String> {
    serde_json::from_slice::<serde_json::Value>(stdout)
        .ok()
        .and_then(|value| value.get("unresolvedFonts").cloned())
        .and_then(|fonts| serde_json::from_value::<Vec<String>>(fonts).ok())
        .unwrap_or_default()
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
    substituted_fonts: Vec<String>,
}

impl Report {
    fn new(path: PathBuf, kit: Bitmap, ours: Bitmap, substituted_fonts: Vec<String>) -> Self {
        let dimensions = ((kit.width, kit.height), (ours.width, ours.height));

        if kit.height as usize > MAX_DIFFABLE_ROWS || ours.height as usize > MAX_DIFFABLE_ROWS {
            return Self {
                path,
                kit: dimensions.0,
                ours: dimensions.1,
                drifts: Vec::new(),
                residual: 0.0,
                too_large: true,
                substituted_fonts,
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
            substituted_fonts,
        }
    }

    fn width_differs(&self) -> bool {
        self.kit.0 != self.ours.0
    }

    /// Whether kit drew a different typeface than this renderer did.
    ///
    /// Such a document cannot be compared on geometry at all: a substituted
    /// face has its own advance widths, so a string wraps at a different point
    /// and every box that hugs it is a different size. Ranking one of these
    /// alongside real divergences sends people hunting a layout bug that is
    /// really a missing font — `harbor-report-post-ios` sat at the top of this
    /// list for exactly that reason.
    fn fonts_substituted(&self) -> bool {
        !self.substituted_fonts.is_empty()
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
            return format!("kit substituted {}", self.substituted_fonts.join(", "));
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

    let incomparable: Vec<&Report> = reports
        .iter()
        .filter(|report| report.fonts_substituted())
        .collect();
    if !incomparable.is_empty() {
        println!();
        println!("not comparable — kit had no copy of the declared font and substituted one,");
        println!("so the two renderers drew different typefaces. Their numbers are about");
        println!("fonts, not layout, and they are excluded from the ranking above.");
        println!("{}", "-".repeat(92));
        for report in &incomparable {
            println!(
                "{}  ({})",
                short(root, &report.path),
                report.substituted_fonts.join(", ")
            );
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

    #[test]
    fn substituted_fonts_are_read_from_the_bridge() {
        assert_eq!(
            substituted_fonts(br#"{"unresolvedFonts":["SF Pro Display","SF Mono"]}"#),
            vec!["SF Pro Display".to_owned(), "SF Mono".to_owned()],
        );
        assert!(substituted_fonts(br#"{"unresolvedFonts":[]}"#).is_empty());
    }

    #[test]
    fn a_bridge_that_says_nothing_useful_still_compares() {
        // The images are the point; losing the font warning must not lose the
        // comparison with it.
        assert!(substituted_fonts(b"").is_empty());
        assert!(substituted_fonts(b"not json at all").is_empty());
        assert!(substituted_fonts(br#"{"unresolvedFonts":"a string"}"#).is_empty());
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
