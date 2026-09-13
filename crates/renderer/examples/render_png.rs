use std::{
    env, fs,
    path::{Path, PathBuf},
    process,
};

use dotgui_renderer::{
    parse_standalone_xml, read_gui_input, AssetCache, FontStore, GuiDocument, GuiInput, LayoutRect,
    Page, ParseError, RENDERER_VERSION, SUPPORTED_VERSION,
};

const USAGE: &str = "\
usage: cargo run -p dotgui-renderer --example render_png -- <file.gui|file.guix> <out.png> [options]

  --scale <n>              pixels per document pixel (default 1)
  --page <name>            only this page of a package, e.g. 02-signup.guix
  --area <x,y,w,h>         only this area, in document pixels
  --element <attr=value>   only this element, e.g. id=hero or name=Card
  --padding <n>            document pixels around --element (default 0)
  --version                print the renderer version";

/// Renders a `.gui` package or bare `.guix` markup to PNG: whole pages, or a
/// screenshot of one area or one element.
///
/// A package with one page writes `<out.png>`. A package with several — many
/// documents, or a library with a page of its own — writes one PNG per page
/// beside it, named `<out>-<document>.png`, unless `--page` picks one.
fn main() {
    let options = Options::parse(env::args().skip(1).collect()).unwrap_or_else(|err| {
        eprintln!("{err}\n\n{USAGE}");
        process::exit(2);
    });

    let bytes = fs::read(&options.input).unwrap_or_else(|err| {
        eprintln!("failed to read {}: {err}", options.input);
        process::exit(1);
    });

    let (mut pages, package_assets): (Vec<(String, Result<GuiDocument, ParseError>)>, _) =
        match read_gui_input(&bytes) {
            Ok(GuiInput::Package(package)) => (
                package
                    .pages()
                    .into_iter()
                    .map(|page| (page.name, page.document))
                    .collect(),
                package.assets,
            ),
            Ok(GuiInput::Markup(xml)) => {
                let name = Path::new(&options.input).file_name().map_or_else(
                    || options.input.clone(),
                    |name| name.to_string_lossy().into_owned(),
                );
                (vec![(name, parse_standalone_xml(&xml))], Default::default())
            }
            Err(err) => {
                eprintln!("failed to open {}: {err}", options.input);
                process::exit(1);
            }
        };

    if let Some(wanted) = &options.page {
        pages.retain(|(name, _)| name == wanted);
        if pages.is_empty() {
            eprintln!("{} has no page named {wanted}", options.input);
            process::exit(1);
        }
    }

    let cache = AssetCache::new(".gui-render/cache").with_package_assets(package_assets);
    let single = pages.len() == 1;
    let mut failed = false;

    for (name, document) in pages {
        let document = match document {
            Ok(document) => document,
            Err(err) => {
                // One broken document is reported by name; the rest still render.
                eprintln!("error: {name}: {err}");
                failed = true;
                continue;
            }
        };
        for warning in &document.warnings {
            eprintln!("warning: {name}: {warning}");
        }

        let target = if single {
            PathBuf::from(&options.output)
        } else {
            page_output(&options.output, &name)
        };
        if let Err(err) = render(document, &cache, &name, &target, &options) {
            eprintln!("error: {name}: {err}");
            failed = true;
        }
    }

    if failed {
        process::exit(1);
    }
}

fn render(
    document: GuiDocument,
    cache: &AssetCache,
    name: &str,
    output: &Path,
    options: &Options,
) -> Result<(), String> {
    let fonts = FontStore::from_document(&document, cache).unwrap_or_else(|err| {
        eprintln!("warning: {name}: failed to resolve declared fonts: {err}");
        FontStore::default()
    });
    // A render that quietly used the wrong typeface is worse than one that
    // says so.
    for warning in fonts.warnings() {
        eprintln!("warning: {name}: {warning}");
    }

    let page = Page::new(document, &fonts).map_err(|err| format!("failed to lay out: {err}"))?;
    let (assets, fonts) = (Some(cache), Some(&fonts));
    let png = match (&options.area, &options.element) {
        (Some(area), _) => page.paint_area_png(*area, options.scale, assets, fonts),
        (None, Some((attribute, value))) => page.paint_element_png(
            attribute,
            value,
            options.scale,
            options.padding,
            assets,
            fonts,
        ),
        (None, None) => page.paint_png(options.scale, assets, fonts),
    }
    .map_err(|err| err.to_string())?;

    fs::write(output, png).map_err(|err| format!("failed to write {}: {err}", output.display()))?;
    println!(
        "wrote {} using asset cache {}",
        output.display(),
        cache.root().display()
    );
    Ok(())
}

struct Options {
    input: String,
    output: String,
    scale: f32,
    page: Option<String>,
    area: Option<LayoutRect>,
    element: Option<(String, String)>,
    padding: f32,
}

impl Options {
    fn parse(args: Vec<String>) -> Result<Self, String> {
        if args.first().map(String::as_str) == Some("--version") {
            println!(
                "dotgui renderer {RENDERER_VERSION} (reads .gui up to version {SUPPORTED_VERSION})"
            );
            process::exit(0);
        }

        let mut positional = Vec::new();
        let mut options = Self {
            input: String::new(),
            output: String::new(),
            scale: 1.0,
            page: None,
            area: None,
            element: None,
            padding: 0.0,
        };
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            let mut value = |flag: &str| args.next().ok_or_else(|| format!("{flag} needs a value"));
            match arg.as_str() {
                "--scale" => {
                    options.scale = number(&value("--scale")?, "--scale")?;
                    if options.scale <= 0.0 {
                        return Err("--scale must be more than 0".to_owned());
                    }
                }
                "--page" => options.page = Some(value("--page")?),
                "--area" => {
                    let raw = value("--area")?;
                    let parts = raw
                        .split(',')
                        .map(|part| number(part, "--area"))
                        .collect::<Result<Vec<_>, _>>()?;
                    let [x, y, width, height] = parts[..] else {
                        return Err(format!("--area takes x,y,width,height, not {raw}"));
                    };
                    options.area = Some(LayoutRect {
                        x,
                        y,
                        width,
                        height,
                    });
                }
                "--element" => {
                    let raw = value("--element")?;
                    let (attribute, value) = raw
                        .split_once('=')
                        .ok_or_else(|| format!("--element takes attribute=value, not {raw}"))?;
                    options.element = Some((attribute.to_owned(), value.to_owned()));
                }
                "--padding" => options.padding = number(&value("--padding")?, "--padding")?,
                flag if flag.starts_with("--") => return Err(format!("unknown option {flag}")),
                _ => positional.push(arg),
            }
        }

        let [input, output] = <[String; 2]>::try_from(positional)
            .map_err(|_| "expected an input file and an output file".to_owned())?;
        if options.area.is_some() && options.element.is_some() {
            return Err("--area and --element each pick what to capture; use one".to_owned());
        }
        options.input = input;
        options.output = output;
        Ok(options)
    }
}

fn number(value: &str, flag: &str) -> Result<f32, String> {
    value
        .trim()
        .parse::<f32>()
        .ok()
        .filter(|number| number.is_finite())
        .ok_or_else(|| format!("{flag}: {value} is not a number"))
}

/// `out.png` and `02-signup.guix` make `out-02-signup.png`.
fn page_output(output: &str, document: &str) -> PathBuf {
    let output = Path::new(output);
    let stem = output
        .file_stem()
        .map_or_else(|| "page".into(), |stem| stem.to_string_lossy());
    let document = Path::new(document)
        .file_stem()
        .map_or_else(|| "page".into(), |stem| stem.to_string_lossy());
    output.with_file_name(format!("{stem}-{document}.png"))
}
