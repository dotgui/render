//! `<components>` definitions and `<instance>` expansion.
//!
//! An instance is expanded while the document is parsed, so nothing downstream
//! ever sees one: layout, the scene and painting work on the same node tree
//! they would have if the document had been written out longhand. kit inlines
//! instances in its parser for the same reason.
//!
//! The vocabulary is RFC-0034's, which the spec summarises as: a `<component>`
//! declares `<props>`, an `<instance>` passes overrides as attributes, and
//! "ad-hoc overrides skip the props block and match by sanitized layer name".

use crate::model::GuiNode;
use std::collections::BTreeMap;

/// How deep an instance may nest before expansion gives up.
///
/// A component whose body instantiates itself would otherwise expand forever.
/// The limit is generous: real component trees are a handful deep.
const MAX_DEPTH: usize = 16;

/// Attributes an instance applies to the expanded body's root rather than
/// treating as a prop override.
///
/// These place and size the instance itself, so they belong to the box the
/// component becomes.
const POSITIONAL: &[&str] = &[
    "component",
    "name",
    "id",
    "x",
    "y",
    "w",
    "h",
    "abs",
    "constraint-h",
    "constraint-v",
    "rotation",
    "opacity",
    "blend",
    "visible",
    "min-width",
    "max-width",
    "min-height",
    "max-height",
];

#[derive(Debug, Clone)]
pub(crate) struct Component {
    props: Vec<Prop>,
    body: GuiNode,
}

#[derive(Debug, Clone)]
struct Prop {
    name: String,
    kind: String,
    /// One prop may drive several layers; RFC-0034 makes `target` a list.
    targets: Vec<String>,
    /// Which attribute the value lands on, when the type alone does not say.
    bind: Option<String>,
}

/// Collects every component and variant a `<components>` block declares.
///
/// A `<component-set>`'s `<variant>` children are components in their own
/// right — an instance references a variant by its own id, not the set's.
pub(crate) fn read_components(blocks: &[GuiNode]) -> BTreeMap<String, Component> {
    let mut components = BTreeMap::new();

    for block in blocks {
        for child in &block.children {
            match child.tag.as_str() {
                "component" => insert_component(&mut components, child),
                "component-set" => {
                    for variant in &child.children {
                        if variant.tag == "variant" {
                            insert_component(&mut components, variant);
                        }
                    }
                }
                _ => {}
            }
        }
    }

    components
}

fn insert_component(components: &mut BTreeMap<String, Component>, node: &GuiNode) {
    let Some(id) = node.attributes.get("id") else {
        return;
    };
    // The body is the one child that is not the props block.
    let Some(body) = node.children.iter().find(|child| child.tag != "props") else {
        return;
    };

    components.insert(
        id.clone(),
        Component {
            props: read_props(node),
            body: body.clone(),
        },
    );
}

fn read_props(component: &GuiNode) -> Vec<Prop> {
    let Some(props) = component.children.iter().find(|child| child.tag == "props") else {
        return Vec::new();
    };

    props
        .children
        .iter()
        .filter(|child| child.tag == "prop")
        .filter_map(|prop| {
            Some(Prop {
                name: prop.attributes.get("name")?.clone(),
                kind: prop.attributes.get("type")?.clone(),
                targets: prop
                    .attributes
                    .get("target")?
                    .split_whitespace()
                    .map(ToOwned::to_owned)
                    .collect(),
                bind: prop.attributes.get("bind").cloned(),
            })
        })
        .collect()
}

/// Replaces every `<instance>` in the tree with the component it names.
pub(crate) fn expand(node: &mut GuiNode, components: &BTreeMap<String, Component>) {
    expand_at(node, components, 0);
}

fn expand_at(node: &mut GuiNode, components: &BTreeMap<String, Component>, depth: usize) {
    if depth > MAX_DEPTH {
        return;
    }

    // Most nodes hold no instance at all, and rebuilding their children would
    // be an allocation per node for nothing.
    if !node.children.iter().any(|child| child.tag == "instance") {
        for child in &mut node.children {
            expand_at(child, components, depth);
        }
        return;
    }

    let mut expanded = Vec::with_capacity(node.children.len());
    for child in std::mem::take(&mut node.children) {
        match instantiate(&child, components, depth) {
            // An instance naming a component nothing declares is dropped
            // rather than left in the tree, where it would lay out as an
            // unknown block.
            Some(body) => expanded.push(body),
            None if child.tag == "instance" => {}
            None => {
                let mut child = child;
                expand_at(&mut child, components, depth);
                expanded.push(child);
            }
        }
    }
    node.children = expanded;
}

fn instantiate(
    node: &GuiNode,
    components: &BTreeMap<String, Component>,
    depth: usize,
) -> Option<GuiNode> {
    if node.tag != "instance" {
        return None;
    }
    let component = components.get(node.attributes.get("component")?)?;
    let mut body = component.body.clone();

    apply_declared_props(&mut body, node, &component.props);
    apply_ad_hoc_overrides(&mut body, node, &component.props);
    apply_positional(&mut body, node);
    scale_to_instance(&mut body, &component.body);

    // A component body may itself hold instances.
    expand_at(&mut body, components, depth + 1);
    Some(body)
}

fn apply_declared_props(body: &mut GuiNode, instance: &GuiNode, props: &[Prop]) {
    for prop in props {
        let Some(value) = instance.attributes.get(&prop.name) else {
            continue;
        };
        for target in &prop.targets {
            apply_override(body, target, &prop.kind, value, prop.bind.as_deref());
        }
    }
}

/// Instance attributes that name a layer directly, with no `<prop>` declaring
/// them.
///
/// The type is inferred from what the target is, which is what kit does: a
/// `<text>` takes a string, a node with a `src` takes an image, `true`/`false`
/// is a visibility toggle.
fn apply_ad_hoc_overrides(body: &mut GuiNode, instance: &GuiNode, props: &[Prop]) {
    for (name, value) in &instance.attributes {
        if POSITIONAL.contains(&name.as_str()) {
            continue;
        }
        if props.iter().any(|prop| prop.name == *name) {
            continue;
        }
        let Some(target) = find_by_id(body, name) else {
            continue;
        };

        let kind = if value == "true" || value == "false" {
            "boolean"
        } else if target.tag == "text" {
            "string"
        } else if target.attributes.contains_key("src") {
            "image"
        } else {
            "string"
        };

        apply_override(body, name, kind, value, None);
    }
}

fn apply_override(body: &mut GuiNode, target: &str, kind: &str, value: &str, bind: Option<&str>) {
    // `visible="false"` removes the layer, so it is handled against the parent
    // rather than the node itself.
    if matches!(kind, "boolean" | "visible") && value == "false" {
        remove_by_id(body, target);
        return;
    }

    // A string aimed at a container means the text inside it, which is how a
    // card's title is overridden without naming the `<text>` itself.
    let descend = matches!(kind, "string" | "text");
    let Some(node) = find_by_id_mut(body, target) else {
        return;
    };
    let node = if descend && node.tag != "text" {
        match first_text_descendant(node) {
            Some(text) => text,
            None => node,
        }
    } else {
        node
    };

    let attribute = match kind {
        "string" | "text" => bind.unwrap_or("value"),
        "color" | "fill" => bind.unwrap_or("fill"),
        "image" | "src" => "src",
        "component" => "component",
        // A number or a named style has no natural home, so the prop has to
        // say which attribute it drives.
        "number" | "style" => match bind {
            Some(bind) => bind,
            None => return,
        },
        _ => return,
    };

    node.attributes
        .insert(attribute.to_owned(), value.to_owned());

    // A named text style is shadowed by any typography attribute the body sets
    // directly, so overriding the style has to clear them.
    if kind == "style" && bind == Some("text-style") {
        for typography in TYPOGRAPHY {
            node.attributes.remove(*typography);
        }
    }
}

const TYPOGRAPHY: &[&str] = &[
    "font-family",
    "font-size",
    "font-weight",
    "font-style",
    "line-height",
    "letter-spacing",
    "font-stretch",
    "font-postscript",
    "font-style-name",
];

/// Copies the instance's own placement onto the body it expands to.
fn apply_positional(body: &mut GuiNode, instance: &GuiNode) {
    for name in POSITIONAL {
        // `component` names what to expand and `id` identifies the instance;
        // neither belongs on the box that replaces it.
        if matches!(*name, "component" | "id") {
            continue;
        }
        if let Some(value) = instance.attributes.get(*name) {
            body.attributes.insert((*name).to_owned(), value.clone());
        }
    }
}

/// Scales a resized instance's contents, per `constraint-h` / `constraint-v`.
///
/// This is the one thing those constraints do in kit: an instance that
/// declares a different `w`/`h` than its component body stretches the children
/// that opt in, and leaves the rest where they are.
fn scale_to_instance(body: &mut GuiNode, original: &GuiNode) {
    let ratio = |name: &str| {
        let from = original.attributes.get(name)?.parse::<f32>().ok()?;
        let to = body.attributes.get(name)?.parse::<f32>().ok()?;
        (from > 0.0 && to > 0.0 && (to - from).abs() > f32::EPSILON).then_some(to / from)
    };

    let scale_x = ratio("w");
    let scale_y = ratio("h");
    if scale_x.is_none() && scale_y.is_none() {
        return;
    }

    scale_children(body, scale_x.unwrap_or(1.0), scale_y.unwrap_or(1.0));
}

fn scale_children(node: &mut GuiNode, scale_x: f32, scale_y: f32) {
    for child in &mut node.children {
        // kit accepts `left-right` and `top-bottom` alongside `scale`, which
        // the spec's enums do not list. Both are honoured; see the tracking
        // issue for which is meant to be right.
        let horizontal = matches!(
            child.attributes.get("constraint-h").map(String::as_str),
            Some("scale" | "left-right")
        );
        let vertical = matches!(
            child.attributes.get("constraint-v").map(String::as_str),
            Some("scale" | "top-bottom")
        );

        if horizontal {
            scale_attribute(child, "x", scale_x);
            scale_attribute(child, "w", scale_x);
        }
        if vertical {
            scale_attribute(child, "y", scale_y);
            scale_attribute(child, "h", scale_y);
        }

        scale_children(child, scale_x, scale_y);
    }
}

fn scale_attribute(node: &mut GuiNode, name: &str, scale: f32) {
    let Some(value) = node
        .attributes
        .get(name)
        .and_then(|it| it.parse::<f32>().ok())
    else {
        return;
    };
    if value == 0.0 {
        return;
    }

    node.attributes
        .insert(name.to_owned(), format!("{}", (value * scale).round()));
}

fn find_by_id<'a>(node: &'a GuiNode, id: &str) -> Option<&'a GuiNode> {
    if node.attributes.get("id").is_some_and(|it| it == id) {
        return Some(node);
    }
    node.children.iter().find_map(|child| find_by_id(child, id))
}

fn find_by_id_mut<'a>(node: &'a mut GuiNode, id: &str) -> Option<&'a mut GuiNode> {
    if node.attributes.get("id").is_some_and(|it| it == id) {
        return Some(node);
    }
    node.children
        .iter_mut()
        .find_map(|child| find_by_id_mut(child, id))
}

fn remove_by_id(node: &mut GuiNode, id: &str) {
    node.children
        .retain(|child| child.attributes.get("id").is_none_or(|it| it != id));
    for child in &mut node.children {
        remove_by_id(child, id);
    }
}

fn first_text_descendant(node: &mut GuiNode) -> Option<&mut GuiNode> {
    let index = node
        .children
        .iter()
        .position(|child| child.tag == "text" || has_text_descendant(child))?;
    let child = &mut node.children[index];
    if child.tag == "text" {
        Some(child)
    } else {
        first_text_descendant(child)
    }
}

fn has_text_descendant(node: &GuiNode) -> bool {
    node.tag == "text" || node.children.iter().any(has_text_descendant)
}

#[cfg(test)]
mod tests {
    use crate::parse_gui_xml;

    /// The tree an instance expands to, as tag/attribute pairs, for asserting
    /// against without threading through layout.
    fn root_of(xml: &str) -> crate::GuiNode {
        parse_gui_xml(xml).expect("valid gui").root
    }

    fn find<'a>(node: &'a crate::GuiNode, id: &str) -> Option<&'a crate::GuiNode> {
        if node.attributes.get("id").is_some_and(|it| it == id) {
            return Some(node);
        }
        node.children.iter().find_map(|child| find(child, id))
    }

    const CARD: &str = r##"
        <components>
          <component name="Card/Product" id="comp-card">
            <props>
              <prop name="title" type="text" target="title" />
            </props>
            <col w="320" radius="12" fill="#fff" p="16" gap="8">
              <text id="title" value="Product Name" font-size="16" />
            </col>
          </component>
        </components>
    "##;

    #[test]
    fn an_instance_becomes_the_component_body() {
        // The spec's own example.
        let root = root_of(&format!(
            r##"
            <gui version="0.2">
              {CARD}
              <frame w="400" h="400">
                <instance component="comp-card" title="Nike Air Max 90" x="24" y="120" />
              </frame>
            </gui>
            "##
        ));

        let card = &root.children[0];
        assert_eq!(card.tag, "col", "the instance is replaced by the body");
        assert_eq!(
            card.attributes.get("radius").map(String::as_str),
            Some("12")
        );
        assert_eq!(
            find(card, "title")
                .unwrap()
                .attributes
                .get("value")
                .map(String::as_str),
            Some("Nike Air Max 90"),
            "the declared prop overrode the target's value"
        );
    }

    #[test]
    fn an_instance_places_the_body_where_the_instance_sat() {
        let root = root_of(&format!(
            r##"
            <gui version="0.2">
              {CARD}
              <frame w="400" h="400">
                <instance component="comp-card" x="24" y="120" opacity="0.5" />
              </frame>
            </gui>
            "##
        ));

        let card = &root.children[0];
        assert_eq!(card.attributes.get("x").map(String::as_str), Some("24"));
        assert_eq!(card.attributes.get("y").map(String::as_str), Some("120"));
        assert_eq!(
            card.attributes.get("opacity").map(String::as_str),
            Some("0.5")
        );
        assert_eq!(
            card.attributes.get("component"),
            None,
            "the reference itself does not survive onto the box"
        );
    }

    #[test]
    fn an_instance_leaves_the_component_untouched_for_the_next_one() {
        let root = root_of(&format!(
            r##"
            <gui version="0.2">
              {CARD}
              <frame w="400" h="400">
                <instance component="comp-card" title="First" />
                <instance component="comp-card" title="Second" />
                <instance component="comp-card" />
              </frame>
            </gui>
            "##
        ));

        let title = |index: usize| {
            find(&root.children[index], "title")
                .unwrap()
                .attributes
                .get("value")
                .cloned()
                .unwrap()
        };

        assert_eq!(title(0), "First");
        assert_eq!(title(1), "Second");
        assert_eq!(title(2), "Product Name", "and the default still stands");
    }

    #[test]
    fn an_ad_hoc_override_matches_a_layer_by_id() {
        // No <prop> declares `subtitle`; it names the layer directly.
        let root = root_of(
            r##"
            <gui version="0.2">
              <components>
                <component id="c">
                  <col w="200">
                    <text id="subtitle" value="Default" />
                  </col>
                </component>
              </components>
              <frame w="400" h="400">
                <instance component="c" subtitle="Overridden" />
              </frame>
            </gui>
            "##,
        );

        assert_eq!(
            find(&root.children[0], "subtitle")
                .unwrap()
                .attributes
                .get("value")
                .map(String::as_str),
            Some("Overridden")
        );
    }

    #[test]
    fn a_false_override_removes_its_layer() {
        let root = root_of(
            r##"
            <gui version="0.2">
              <components>
                <component id="c">
                  <col w="200">
                    <text id="badge" value="New" />
                    <text id="label" value="Item" />
                  </col>
                </component>
              </components>
              <frame w="400" h="400">
                <instance component="c" badge="false" />
              </frame>
            </gui>
            "##,
        );

        let card = &root.children[0];
        assert!(find(card, "badge").is_none(), "the layer is gone");
        assert!(find(card, "label").is_some(), "its sibling is not");
    }

    #[test]
    fn a_string_prop_aimed_at_a_container_finds_the_text_inside_it() {
        let root = root_of(
            r##"
            <gui version="0.2">
              <components>
                <component id="c">
                  <props>
                    <prop name="label" type="string" target="slot" />
                  </props>
                  <col w="200">
                    <row id="slot">
                      <text value="Default" />
                    </row>
                  </col>
                </component>
              </components>
              <frame w="400" h="400">
                <instance component="c" label="Pressed" />
              </frame>
            </gui>
            "##,
        );

        let slot = find(&root.children[0], "slot").unwrap();
        assert_eq!(
            slot.children[0].attributes.get("value").map(String::as_str),
            Some("Pressed")
        );
    }

    #[test]
    fn a_prop_can_drive_several_targets_and_bind_a_named_attribute() {
        let root = root_of(
            r##"
            <gui version="0.2">
              <components>
                <component id="c">
                  <props>
                    <prop name="tint" type="color" target="a b" />
                    <prop name="size" type="number" target="a" bind="radius" />
                  </props>
                  <col w="200">
                    <rect id="a" w="10" h="10" fill="#000000" />
                    <rect id="b" w="10" h="10" fill="#000000" />
                  </col>
                </component>
              </components>
              <frame w="400" h="400">
                <instance component="c" tint="#ff0000" size="8" />
              </frame>
            </gui>
            "##,
        );

        let card = &root.children[0];
        assert_eq!(
            find(card, "a")
                .unwrap()
                .attributes
                .get("fill")
                .map(String::as_str),
            Some("#ff0000")
        );
        assert_eq!(
            find(card, "b")
                .unwrap()
                .attributes
                .get("fill")
                .map(String::as_str),
            Some("#ff0000")
        );
        assert_eq!(
            find(card, "a")
                .unwrap()
                .attributes
                .get("radius")
                .map(String::as_str),
            Some("8")
        );
    }

    #[test]
    fn a_variant_is_referenced_by_its_own_id() {
        let root = root_of(
            r##"
            <gui version="0.2">
              <components>
                <component-set id="set" name="Button">
                  <variant id="btn-primary">
                    <rect w="100" h="40" fill="#0d99ff" />
                  </variant>
                  <variant id="btn-ghost">
                    <rect w="100" h="40" fill="#ffffff" />
                  </variant>
                </component-set>
              </components>
              <frame w="400" h="400">
                <instance component="btn-ghost" />
              </frame>
            </gui>
            "##,
        );

        assert_eq!(
            root.children[0].attributes.get("fill").map(String::as_str),
            Some("#ffffff")
        );
    }

    #[test]
    fn an_instance_inside_a_component_is_expanded_too() {
        let root = root_of(
            r##"
            <gui version="0.2">
              <components>
                <component id="inner">
                  <text id="t" value="inner" />
                </component>
                <component id="outer">
                  <col w="200">
                    <instance component="inner" t="from outer" />
                  </col>
                </component>
              </components>
              <frame w="400" h="400">
                <instance component="outer" />
              </frame>
            </gui>
            "##,
        );

        let inner = &root.children[0].children[0];
        assert_eq!(inner.tag, "text");
        assert_eq!(
            inner.attributes.get("value").map(String::as_str),
            Some("from outer")
        );
    }

    #[test]
    fn a_component_that_instantiates_itself_stops_rather_than_hanging() {
        let root = root_of(
            r##"
            <gui version="0.2">
              <components>
                <component id="loop">
                  <col w="200">
                    <instance component="loop" />
                  </col>
                </component>
              </components>
              <frame w="400" h="400">
                <instance component="loop" />
              </frame>
            </gui>
            "##,
        );

        // Bounded, and the nesting is the depth limit rather than unbounded.
        let mut depth = 0;
        let mut node = &root.children[0];
        while let Some(child) = node.children.first() {
            depth += 1;
            node = child;
            assert!(depth < 64, "expansion did not stop");
        }
        assert!(depth > 0);
    }

    #[test]
    fn an_unknown_component_leaves_nothing_behind() {
        let root = root_of(
            r##"
            <gui version="0.2">
              <frame w="400" h="400">
                <instance component="nope" />
                <rect w="10" h="10" />
              </frame>
            </gui>
            "##,
        );

        assert_eq!(root.children.len(), 1, "the instance is dropped");
        assert_eq!(root.children[0].tag, "rect");
    }

    #[test]
    fn a_resized_instance_scales_the_children_that_opt_in() {
        let root = root_of(
            r##"
            <gui version="0.2">
              <components>
                <component id="c">
                  <frame w="100" h="50">
                    <rect id="track" x="10" y="0" w="80" h="4" constraint-h="scale" />
                    <rect id="knob" x="10" y="0" w="12" h="12" />
                  </frame>
                </component>
              </components>
              <frame w="400" h="400">
                <instance component="c" w="200" h="50" />
              </frame>
            </gui>
            "##,
        );

        let card = &root.children[0];
        let track = find(card, "track").unwrap();
        assert_eq!(
            track.attributes.get("w").map(String::as_str),
            Some("160"),
            "doubled"
        );
        assert_eq!(track.attributes.get("x").map(String::as_str), Some("20"));

        let knob = find(card, "knob").unwrap();
        assert_eq!(
            knob.attributes.get("w").map(String::as_str),
            Some("12"),
            "no constraint, so it keeps its size"
        );
    }

    #[test]
    fn an_instance_at_its_components_own_size_scales_nothing() {
        let root = root_of(
            r##"
            <gui version="0.2">
              <components>
                <component id="c">
                  <frame w="100" h="50">
                    <rect id="track" x="10" y="0" w="80" h="4" constraint-h="scale" />
                  </frame>
                </component>
              </components>
              <frame w="400" h="400">
                <instance component="c" w="100" h="50" />
              </frame>
            </gui>
            "##,
        );

        let track = find(&root.children[0], "track").unwrap();
        assert_eq!(track.attributes.get("w").map(String::as_str), Some("80"));
    }
}
