//! Generates `COVERAGE.md` and fails when the committed copy is stale.
//!
//! The table joins the vendored `spec/spec.json` against the declaration in
//! `dotgui_renderer::coverage`. Neither side is inferred: the spec is the
//! generated source of truth from `dotgui/core`, and the declaration is a
//! claim maintained by whoever implements an attribute.
//!
//! Regenerate with:
//!
//! ```text
//! UPDATE_COVERAGE=1 cargo test -p dotgui-renderer --test spec_coverage
//! ```

use dotgui_renderer::coverage;
use serde_json::Value;
use std::{fmt::Write as _, fs, path::PathBuf};

/// Elements that describe the document rather than draw anything, so coverage
/// of their attributes is a parser concern, not a rendering one.
const NON_VISUAL: &[&str] = &["gui", "tokens", "styles", "fonts", "components"];

#[test]
fn coverage_report_is_up_to_date() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();

    let spec_path = root.join("spec/spec.json");
    let spec: Value = serde_json::from_slice(&fs::read(&spec_path).unwrap_or_else(|err| {
        panic!(
            "vendored spec {} is missing ({err}); refresh it from dotgui/core",
            spec_path.display()
        )
    }))
    .expect("spec.json parses");

    let source = fs::read_to_string(root.join("spec/SOURCE")).expect("spec/SOURCE is readable");
    let commit = source
        .lines()
        .find_map(|line| line.strip_prefix("commit = "))
        .unwrap_or("unknown")
        .trim();

    let report = render_report(&spec, commit);
    let report_path = root.join("COVERAGE.md");

    if std::env::var_os("UPDATE_COVERAGE").is_some() {
        fs::write(&report_path, &report).expect("COVERAGE.md is writable");
        println!("wrote {}", report_path.display());
        return;
    }

    let committed = fs::read_to_string(&report_path).unwrap_or_default();
    assert_eq!(
        committed.trim(),
        report.trim(),
        "COVERAGE.md is out of date. Regenerate with \
         UPDATE_COVERAGE=1 cargo test -p dotgui-renderer --test spec_coverage"
    );
}

/// One element's attributes, split into what is and is not implemented.
struct ElementCoverage {
    tag: String,
    implemented: Vec<String>,
    missing: Vec<String>,
}

fn render_report(spec: &Value, commit: &str) -> String {
    let shared: Vec<String> = spec["sharedAttributes"]
        .as_array()
        .map(|attrs| attrs.iter().filter_map(attribute_name).collect())
        .unwrap_or_default();

    let mut elements = Vec::new();
    for element in spec["elements"].as_array().into_iter().flatten() {
        let Some(tag) = element["tag"].as_str() else {
            continue;
        };
        if NON_VISUAL.contains(&tag) {
            continue;
        }

        let shared_applies = element["sharedAttrs"] == Value::Bool(true);
        let mut names: Vec<String> = element["attributes"]
            .as_array()
            .map(|attrs| attrs.iter().filter_map(attribute_name).collect())
            .unwrap_or_default();
        if shared_applies {
            names.extend(shared.iter().cloned());
        }
        names.sort();
        names.dedup();

        let (implemented, missing) = names
            .into_iter()
            .partition(|name| coverage::is_supported(tag, name, shared_applies));

        elements.push(ElementCoverage {
            tag: tag.to_owned(),
            implemented,
            missing,
        });
    }

    let total: usize = elements
        .iter()
        .map(|e| e.implemented.len() + e.missing.len())
        .sum();
    let done: usize = elements.iter().map(|e| e.implemented.len()).sum();

    let mut out = String::new();
    let _ = writeln!(out, "# Spec coverage\n");
    let _ = writeln!(
        out,
        "Generated — do not edit by hand. Run `UPDATE_COVERAGE=1 cargo test -p dotgui-renderer \
         --test spec_coverage` and commit the result.\n"
    );
    let _ = writeln!(
        out,
        "Measured against `spec/spec.json`, vendored from `dotgui/core` at commit `{}`. Coverage \
         is declared in `crates/renderer/src/coverage.rs`, not inferred from the sources, so a \
         row says the renderer reads that attribute and acts on it.\n",
        &commit[..commit.len().min(12)]
    );
    let _ = writeln!(
        out,
        "**{done} of {total}** element/attribute pairs implemented. Pairs, not unique \
         attributes — an attribute shared by eight elements counts eight times, because \
         supporting it on one is not supporting it on the others.\n"
    );

    let _ = writeln!(out, "| Element | Implemented | Total |");
    let _ = writeln!(out, "|---|---|---|");
    for element in &elements {
        let element_total = element.implemented.len() + element.missing.len();
        let _ = writeln!(
            out,
            "| `<{}>` | {} | {} |",
            element.tag,
            element.implemented.len(),
            element_total
        );
    }

    let _ = writeln!(out, "\n## Not implemented\n");
    for element in &elements {
        if element.missing.is_empty() {
            continue;
        }
        let _ = writeln!(out, "**`<{}>`**\n", element.tag);
        let _ = writeln!(
            out,
            "{}\n",
            element
                .missing
                .iter()
                .map(|name| format!("`{name}`"))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }

    let _ = writeln!(out, "## Ahead of the vendored spec\n");
    let _ = writeln!(
        out,
        "Supported here but not yet described by `spec.json`, which predates RFC-0032 and still \
         documents `<grid>` as `columns`/`rows` only.\n"
    );
    for (tag, attributes) in coverage::AHEAD_OF_SPEC {
        let _ = writeln!(
            out,
            "- `<{tag}>` — {}",
            attributes
                .iter()
                .map(|name| format!("`{name}`"))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }

    out
}

fn attribute_name(attribute: &Value) -> Option<String> {
    let name = attribute["name"].as_str()?;

    // `<appearance>` lists child elements rather than attributes, so `<fill>`
    // and friends are kept as-is.
    let body = name
        .strip_prefix('<')
        .and_then(|rest| rest.strip_suffix('>'));
    let letters = body.unwrap_or(name);

    // The spec also carries prose rows ("Hex opaque") in the same array.
    letters
        .chars()
        .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-')
        .then(|| name.to_owned())
}
