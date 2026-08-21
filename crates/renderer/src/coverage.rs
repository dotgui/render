//! What this renderer implements, declared rather than inferred.
//!
//! Coverage against the `.gui` spec used to be measured by scanning these
//! sources for attribute names, which counts an attribute as implemented when
//! it appears in a comment. This module is the honest version: each entry is a
//! claim that the renderer reads the attribute and acts on it.
//!
//! Two things read this list:
//!
//! - `tests/spec_coverage.rs` joins it against the vendored `spec/spec.json`
//!   to generate `COVERAGE.md`, and fails when the committed file is stale.
//! - Diagnostics, so a document using something unimplemented can be told so
//!   rather than silently rendering wrong.
//!
//! Adding support for an attribute means adding it here in the same change.

/// Attributes honoured on every element that the spec marks as taking the
/// shared set.
///
/// The min/max constraints are read under their spec names; `min-w`, `max-w`,
/// `min-h` and `max-h` are accepted as aliases, and aliases are not spec
/// properties, so they are not listed here.
pub const SHARED: &[&str] = &[
    "abs",
    "opacity",
    "min-width",
    "max-width",
    "min-height",
    "max-height",
];

/// Attributes honoured per element, beyond [`SHARED`].
pub const BY_ELEMENT: &[(&str, &[&str])] = &[
    (
        "frame",
        &[
            "w",
            "h",
            "x",
            "y",
            "fill",
            "border",
            "radius",
            "clip",
            "corner-smoothing",
            "outline",
            "outline-offset",
            "shadow",
        ],
    ),
    (
        "stack",
        &[
            "w",
            "h",
            "x",
            "y",
            "fill",
            "border",
            "radius",
            "clip",
            "align",
            "direction",
            "gap",
            "p",
            "pt",
            "pr",
            "pb",
            "pl",
            "grid-columns",
            "grid-rows",
            "grid-col-gap",
            "grid-row-gap",
            "corner-smoothing",
            "outline",
            "outline-offset",
            "shadow",
        ],
    ),
    (
        "row",
        &[
            "w",
            "h",
            "x",
            "y",
            "fill",
            "border",
            "radius",
            "clip",
            "align",
            "gap",
            "p",
            "pt",
            "pr",
            "pb",
            "pl",
            "corner-smoothing",
            "outline",
            "outline-offset",
            "shadow",
        ],
    ),
    (
        "col",
        &[
            "w",
            "h",
            "x",
            "y",
            "fill",
            "border",
            "radius",
            "clip",
            "align",
            "gap",
            "p",
            "pt",
            "pr",
            "pb",
            "pl",
            "corner-smoothing",
            "outline",
            "outline-offset",
            "shadow",
        ],
    ),
    (
        "grid",
        &[
            "w",
            "h",
            "x",
            "y",
            "fill",
            "border",
            "radius",
            "clip",
            "p",
            "pt",
            "pr",
            "pb",
            "pl",
            "columns",
            "rows",
            "corner-smoothing",
            "outline",
            "outline-offset",
            "shadow",
        ],
    ),
    ("group", &["w", "h", "x", "y"]),
    (
        "text",
        &[
            "w",
            "h",
            "x",
            "y",
            "fill",
            "align",
            "value",
            "font-family",
            "font-size",
            "font-style",
            "font-weight",
            "letter-spacing",
            "line-height",
            "max-lines",
            "overflow",
            "text-style",
            "truncate",
        ],
    ),
    (
        "img",
        &[
            "w",
            "h",
            "x",
            "y",
            "src",
            "fit",
            "radius",
            "border",
            "corner-smoothing",
        ],
    ),
    (
        "rect",
        &[
            "w",
            "h",
            "x",
            "y",
            "fill",
            "border",
            "radius",
            "corner-smoothing",
            "shadow",
        ],
    ),
    ("ellipse", &["w", "h", "x", "y", "fill", "border", "shadow"]),
    ("line", &["w", "h", "x", "y", "fill"]),
    // `<appearance>` holds child elements rather than attributes. All three
    // stacks are read; gradient and image fills are carried into the scene but
    // not painted yet.
    ("appearance", &["<fill>", "<border>", "<effect>"]),
];

/// Attributes this renderer supports that the vendored spec does not yet
/// describe.
///
/// `spec.json` predates RFC-0032, so it still documents `<grid>` with
/// `columns` / `rows` / `col-gap` / `row-gap` and knows nothing of track
/// templates, unit grids, or child placement. These are listed so the coverage
/// report can say the renderer is ahead here rather than appearing to ignore
/// them.
pub const AHEAD_OF_SPEC: &[(&str, &[&str])] = &[
    ("grid", &["cols", "unit"]),
    ("*", &["gc", "gr", "col-span", "row-span", "segment"]),
];

/// Whether an attribute is implemented on an element.
pub fn is_supported(tag: &str, attribute: &str, shared_applies: bool) -> bool {
    if shared_applies && SHARED.contains(&attribute) {
        return true;
    }
    BY_ELEMENT
        .iter()
        .find(|(element, _)| *element == tag)
        .is_some_and(|(_, attributes)| attributes.contains(&attribute))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_attributes_apply_only_where_the_spec_says_so() {
        assert!(is_supported("row", "opacity", true));
        assert!(!is_supported("row", "opacity", false));
    }

    #[test]
    fn element_attributes_do_not_leak_between_elements() {
        assert!(is_supported("text", "line-height", true));
        assert!(!is_supported("rect", "line-height", true));
    }

    #[test]
    fn the_declaration_matches_what_the_renderer_reads() {
        // A spot check that this list is not drifting from the code. Each of
        // these is read in `taffy_layout`, `scene`, or `text_style`.
        for (tag, attribute) in [
            ("col", "gap"),
            ("row", "align"),
            ("grid", "columns"),
            ("text", "truncate"),
            ("img", "fit"),
            ("ellipse", "fill"),
            ("rect", "shadow"),
            ("rect", "corner-smoothing"),
            ("frame", "outline"),
            ("frame", "outline-offset"),
        ] {
            assert!(
                is_supported(tag, attribute, true),
                "<{tag}> {attribute} is read by the renderer but not declared"
            );
        }

        // And that known gaps stay declared as gaps.
        for (tag, attribute) in [
            ("frame", "border-image"),
            ("text", "decoration"),
            ("group", "mask-src"),
            ("row", "wrap"),
        ] {
            assert!(
                !is_supported(tag, attribute, true),
                "<{tag}> {attribute} is declared but the renderer does not read it"
            );
        }
    }
}
