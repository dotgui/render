//! Token modes: `<mode>` axes, per-mode token values, `mode-{axis}` pins.
//!
//! RFC-0037. A token may hold one value per *mode* — light and dark being the
//! obvious pair, brand and density being others — and a subtree pins the mode
//! it is drawn in. The document declares which axes exist; the consumer may
//! name an active mode at render time; each axis has a default for when
//! neither does.
//!
//! Resolution happens once, here, right after components are expanded: every
//! `$token` whose value varies by mode is rewritten to the value its node's
//! active mode gives it. Everything downstream — layout, the scene, painting —
//! then reads the flat token map it always read, so the cascade lives in one
//! place instead of being threaded through every call that resolves a token.

use crate::{GuiMetadata, GuiNode};
use std::{borrow::Cow, collections::BTreeMap};

/// The mode in force at a node: axis name to the axis's active value.
pub type Modes = BTreeMap<String, String>;

/// One declared axis: `<mode name="theme" values="light dark" default="light" />`.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ModeAxis {
    pub name: String,
    pub values: Vec<String>,
    /// The value used when neither the tree nor the consumer names one.
    ///
    /// The spec falls back to the first declared value when `default` is
    /// absent, so this is only empty for an axis that declared no values
    /// either — one that can never resolve to anything.
    pub default: String,
}

/// The mode at the root: what the consumer asked for, else each axis's default.
///
/// An axis the consumer names that the document never declared is ignored
/// rather than invented; so is a value outside the axis's own list, because a
/// token only ever holds values the axis declared.
fn root_modes(metadata: &GuiMetadata, requested: Option<&Modes>) -> Modes {
    metadata
        .mode_axes
        .iter()
        .map(|axis| {
            let active = requested
                .and_then(|requested| requested.get(&axis.name))
                .filter(|value| axis.values.iter().any(|known| known == *value))
                .cloned()
                .unwrap_or_else(|| axis.default.clone());
            (axis.name.clone(), active)
        })
        .collect()
}

/// Resolves every mode-varying `$token` in the tree against its node's mode.
///
/// A document that declares no axes, or whose tokens all hold a single value,
/// leaves the tree untouched.
pub(crate) fn apply(root: &mut GuiNode, metadata: &GuiMetadata, requested: Option<&Modes>) {
    if metadata.mode_axes.is_empty() || metadata.token_modes.is_empty() {
        return;
    }

    let modes = root_modes(metadata, requested);
    resolve_node(root, metadata, &modes);
}

fn resolve_node(node: &mut GuiNode, metadata: &GuiMetadata, inherited: &Modes) {
    let active = pinned(node, inherited);

    for value in node.attributes.values_mut() {
        if let Some(resolved) = resolve_value(value, metadata, &active) {
            *value = resolved;
        }
    }

    for child in &mut node.children {
        resolve_node(child, metadata, &active);
    }
}

/// The mode at this node: the inherited one, with its own pins layered over.
///
/// Nearest ancestor-or-self wins, and axes compose, so a node pinning `theme`
/// keeps whatever `density` it inherited. Nothing is cloned for the usual node,
/// which pins nothing.
fn pinned<'a>(node: &GuiNode, inherited: &'a Modes) -> Cow<'a, Modes> {
    let mut active = Cow::Borrowed(inherited);
    for (attribute, value) in &node.attributes {
        if let Some(axis) = attribute.strip_prefix("mode-") {
            active
                .to_mut()
                .insert(axis.to_owned(), value.trim().to_owned());
        }
    }
    active
}

/// The attribute with its mode-varying tokens resolved, or `None` if none were.
///
/// Reads `$name` references exactly as `resolve_token` does — whitespace-
/// separated parts that start with `$` — so a value means the same thing here
/// as it does there, and a token left alone still resolves the old way.
fn resolve_value(value: &str, metadata: &GuiMetadata, active: &Modes) -> Option<String> {
    if !value.contains('$') {
        return None;
    }

    let mut changed = false;
    let resolved = value
        .split_whitespace()
        .map(|part| match part.strip_prefix('$') {
            Some(name) => match value_for(name, metadata, active) {
                Some(value) => {
                    changed = true;
                    value
                }
                None => part.to_owned(),
            },
            None => part.to_owned(),
        })
        .collect::<Vec<_>>()
        .join(" ");

    changed.then_some(resolved)
}

/// What a token holds in this mode, if it varies by mode at all.
///
/// A token declares its values along one axis or several; the first declared
/// axis it has any value for decides it. Within that axis the active value
/// wins, and a token that skips it falls back to the axis's default — which is
/// what makes a half-filled token still resolve rather than vanish.
fn value_for(name: &str, metadata: &GuiMetadata, active: &Modes) -> Option<String> {
    let per_mode = metadata.token_modes.get(name)?;

    for axis in &metadata.mode_axes {
        let wanted = active.get(&axis.name).unwrap_or(&axis.default);
        if let Some(value) = per_mode.get(&format!("{}-{}", axis.name, wanted)) {
            return Some(value.clone());
        }
        if let Some(value) = per_mode.get(&format!("{}-{}", axis.name, axis.default)) {
            return Some(value.clone());
        }
    }

    None
}

/// The value a per-mode token holds in every axis's default mode.
///
/// This is what goes into the flat token map, so a consumer that knows nothing
/// about modes — and every check that reads declared tokens — still finds the
/// token declared and holding the appearance the document leads with.
pub(crate) fn default_value(
    per_mode: &BTreeMap<String, String>,
    axes: &[ModeAxis],
) -> Option<String> {
    axes.iter()
        .find_map(|axis| per_mode.get(&format!("{}-{}", axis.name, axis.default)))
        .or_else(|| per_mode.values().next())
        .cloned()
}

#[cfg(test)]
mod tests {
    use crate::{parse_gui_xml, parse_gui_xml_with, GuiNode, Modes, ParseOptions};

    fn parse(xml: &str, mode: &Modes) -> GuiNode {
        parse_gui_xml_with(
            xml,
            ParseOptions {
                mode: (!mode.is_empty()).then_some(mode),
                ..ParseOptions::default()
            },
        )
        .expect("valid gui")
        .root
    }

    fn mode(pairs: &[(&str, &str)]) -> Modes {
        pairs
            .iter()
            .map(|(axis, value)| ((*axis).to_owned(), (*value).to_owned()))
            .collect()
    }

    const THEMED: &str = r##"
        <gui version="0.3">
          <mode name="theme" values="light dark" default="light" />
          <tokens>
            <color name="bg" theme-light="#ffffff" theme-dark="#000000" />
            <number name="radius" value="12" />
          </tokens>
          <col w="100" h="100" fill="$bg" radius="$radius">
            <col id="pinned" w="50" h="50" fill="$bg" mode-theme="light" />
          </col>
        </gui>
    "##;

    #[test]
    fn a_per_mode_token_resolves_to_the_default_mode() {
        // The whole point: `<color name="bg" theme-light=… theme-dark=… />`
        // carries no plain `value`, so it used to be read as declaring
        // nothing — leaving `$bg` undeclared and every fill that named it
        // empty.
        let root = parse(THEMED, &Modes::new());
        assert_eq!(root.attributes["fill"], "#ffffff");
    }

    #[test]
    fn a_token_with_a_plain_value_is_constant_across_modes() {
        for wanted in [Modes::new(), mode(&[("theme", "dark")])] {
            let root = parse(THEMED, &wanted);
            assert_eq!(
                root.attributes["radius"], "$radius",
                "left for the flat map"
            );
        }
    }

    #[test]
    fn the_consumer_chooses_the_mode_but_a_pin_outranks_it() {
        // Step 1 of the cascade beats step 2: a subtree that pins its own mode
        // keeps it, which is how a light card sits on a page turned dark.
        let root = parse(THEMED, &mode(&[("theme", "dark")]));
        assert_eq!(root.attributes["fill"], "#000000", "the page follows along");
        assert_eq!(
            root.children[0].attributes["fill"], "#ffffff",
            "the pinned card does not"
        );
    }

    #[test]
    fn a_pin_reaches_the_whole_subtree_under_it() {
        let root = parse(
            r##"
            <gui version="0.3">
              <mode name="theme" values="light dark" default="light" />
              <tokens><color name="ink" theme-light="#111111" theme-dark="#eeeeee" /></tokens>
              <col w="100" h="100" mode-theme="dark">
                <col w="80" h="80">
                  <text value="deep" fill="$ink" />
                </col>
              </col>
            </gui>
            "##,
            &Modes::new(),
        );

        assert_eq!(root.children[0].children[0].attributes["fill"], "#eeeeee");
    }

    #[test]
    fn an_unknown_axis_or_value_is_ignored_rather_than_invented() {
        // A consumer asking for something the document never declared gets the
        // document's own default, not an empty value.
        let root = parse(THEMED, &mode(&[("theme", "sepia"), ("density", "compact")]));
        assert_eq!(root.attributes["fill"], "#ffffff");
    }

    #[test]
    fn a_token_that_skips_a_mode_falls_back_to_the_axis_default() {
        // "A token that omits one of an axis's values falls back to that
        // axis's default at resolution time."
        let root = parse(
            r##"
            <gui version="0.3">
              <mode name="theme" values="light dark" default="light" />
              <tokens><color name="bg" theme-light="#ffffff" /></tokens>
              <col w="10" h="10" fill="$bg" />
            </gui>
            "##,
            &mode(&[("theme", "dark")]),
        );

        assert_eq!(root.attributes["fill"], "#ffffff");
    }

    #[test]
    fn two_axes_compose_and_are_resolved_independently() {
        let xml = r##"
            <gui version="0.3">
              <modes>
                <mode name="theme" values="light dark" default="light" />
                <mode name="density" values="comfortable compact" default="comfortable" />
              </modes>
              <tokens>
                <color  name="bg"  theme-light="#ffffff" theme-dark="#000000" />
                <number name="gap" density-comfortable="16" density-compact="4" />
              </tokens>
              <col w="100" h="100" fill="$bg" gap="$gap" />
            </gui>
        "##;

        let root = parse(xml, &mode(&[("theme", "dark")]));
        assert_eq!(root.attributes["fill"], "#000000");
        assert_eq!(
            root.attributes["gap"], "16",
            "the other axis keeps its own default"
        );

        let both = parse(xml, &mode(&[("theme", "dark"), ("density", "compact")]));
        assert_eq!(both.attributes["fill"], "#000000");
        assert_eq!(both.attributes["gap"], "4");
    }

    #[test]
    fn a_per_mode_token_counts_as_declared_under_0_3() {
        // 0.3 makes an undeclared `$token` an error, so a document using modes
        // did not merely lose its colours — it failed to parse at all.
        assert!(parse_gui_xml(THEMED).is_ok());
    }

    #[test]
    fn a_mode_token_resolves_inside_a_longer_value() {
        // Values are read part by part, exactly as `resolve_token` reads them,
        // so a token among literals still resolves and the literals survive.
        let root = parse(
            r##"
            <gui version="0.3">
              <mode name="theme" values="light dark" default="dark" />
              <tokens><number name="pad" theme-light="8" theme-dark="20" /></tokens>
              <col w="100" h="100" p="4 $pad 4 $pad" />
            </gui>
            "##,
            &Modes::new(),
        );

        assert_eq!(root.attributes["p"], "4 20 4 20");
    }

    #[test]
    fn an_axis_with_no_default_leads_with_its_first_value() {
        // "must be one of `values` (falls back to the first value if omitted)".
        let root = parse(
            r##"
            <gui version="0.3">
              <mode name="brand" values="acme globex" />
              <tokens><color name="bg" brand-acme="#ff0000" brand-globex="#00ff00" /></tokens>
              <col w="10" h="10" fill="$bg" />
            </gui>
            "##,
            &Modes::new(),
        );

        assert_eq!(root.attributes["fill"], "#ff0000");
    }
}
