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

/// One property, and the elements the spec allows it on.
struct Property {
    name: String,
    allowed_on: Vec<String>,
    implemented_on: Vec<String>,
}

impl Property {
    fn missing_on(&self) -> Vec<&str> {
        self.allowed_on
            .iter()
            .filter(|tag| !self.implemented_on.contains(tag))
            .map(String::as_str)
            .collect()
    }

    fn status(&self) -> Status {
        match self.implemented_on.len() {
            0 => Status::Missing,
            n if n == self.allowed_on.len() => Status::Done,
            _ => Status::Partial,
        }
    }
}

#[derive(PartialEq)]
enum Status {
    Done,
    Partial,
    Missing,
}

/// Collects the spec into one row per property rather than per element.
///
/// A property is the unit of work: implementing `radius` is one job that
/// touches every element allowing it, not one job per element. Listing it per
/// element repeats every shared attribute a dozen times and makes the work look
/// far larger than it is.
fn properties(spec: &Value) -> Vec<Property> {
    let shared: Vec<String> = spec["sharedAttributes"]
        .as_array()
        .map(|attrs| attrs.iter().filter_map(attribute_name).collect())
        .unwrap_or_default();

    let mut order: Vec<String> = Vec::new();
    let mut allowed: std::collections::BTreeMap<String, Vec<String>> = Default::default();
    let mut implemented: std::collections::BTreeMap<String, Vec<String>> = Default::default();

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

        for name in names {
            if !allowed.contains_key(&name) {
                order.push(name.clone());
            }
            allowed
                .entry(name.clone())
                .or_default()
                .push(tag.to_owned());
            if coverage::is_supported(tag, &name, shared_applies) {
                implemented.entry(name).or_default().push(tag.to_owned());
            }
        }
    }

    order.sort();
    order
        .into_iter()
        .map(|name| Property {
            allowed_on: allowed.remove(&name).unwrap_or_default(),
            implemented_on: implemented.remove(&name).unwrap_or_default(),
            name,
        })
        .collect()
}

fn tags(list: &[impl AsRef<str>]) -> String {
    if list.is_empty() {
        return "—".to_owned();
    }
    list.iter()
        .map(|tag| format!("`<{}>`", tag.as_ref()))
        .collect::<Vec<_>>()
        .join(", ")
}

fn render_report(spec: &Value, commit: &str) -> String {
    let properties = properties(spec);
    let count = |status: Status| properties.iter().filter(|p| p.status() == status).count();
    let (done, partial, missing) = (
        count(Status::Done),
        count(Status::Partial),
        count(Status::Missing),
    );

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
         row says the renderer reads that property and acts on it.\n",
        &commit[..commit.len().min(12)]
    );
    let _ = writeln!(
        out,
        "Listed by property, because that is the unit of work: implementing `radius` is one job \
         across every element that allows it, not one job per element.\n"
    );
    let _ = writeln!(
        out,
        "| | Properties |\n|---|---|\n| Implemented | **{done}** |\n| Partial | **{partial}** \
         |\n| Not implemented | **{missing}** |\n| Total | **{}** |\n",
        done + partial + missing
    );

    let section = |out: &mut String, title: &str, note: &str, rows: Vec<String>| {
        if rows.is_empty() {
            return;
        }
        let _ = writeln!(out, "## {title}\n");
        if !note.is_empty() {
            let _ = writeln!(out, "{note}\n");
        }
        let _ = writeln!(out, "| Property | Elements |\n|---|---|");
        for row in rows {
            let _ = writeln!(out, "{row}");
        }
        let _ = writeln!(out);
    };

    section(
        &mut out,
        "Not implemented",
        "The work list. Each row is one property to add, and the elements it has to work on.",
        properties
            .iter()
            .filter(|p| p.status() == Status::Missing)
            .map(|p| format!("| `{}` | {} |", p.name, tags(&p.allowed_on)))
            .collect(),
    );

    if properties.iter().any(|p| p.status() == Status::Partial) {
        let _ = writeln!(out, "## Partially implemented\n");
        let _ = writeln!(
            out,
            "Read on some elements but not others — usually cheaper to finish than to start.\n"
        );
        let _ = writeln!(
            out,
            "| Property | Implemented on | Missing on |\n|---|---|---|"
        );
        for property in properties.iter().filter(|p| p.status() == Status::Partial) {
            let _ = writeln!(
                out,
                "| `{}` | {} | {} |",
                property.name,
                tags(&property.implemented_on),
                tags(&property.missing_on())
            );
        }
        let _ = writeln!(out);
    }

    section(
        &mut out,
        "Implemented",
        "",
        properties
            .iter()
            .filter(|p| p.status() == Status::Done)
            .map(|p| format!("| `{}` | {} |", p.name, tags(&p.allowed_on)))
            .collect(),
    );

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
