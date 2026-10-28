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
//!
//! RFC-0044 adds slots. A layout container carrying `slot="name"` inside a
//! component body is a hole; an instance fills it with
//! `<slot name="name">…</slot>` children. The container's own children are the
//! fallback, and a slot left empty takes up no space at all.
//!
//! RFC-0042 adds a second place a component can come from: the package's
//! `library.guix`. A document sees its own components and the library's; a
//! library component's body sees only the library's, because the library
//! depends on nothing.

use crate::issues::Issues;
use crate::model::GuiNode;
use std::collections::BTreeMap;

/// How deep an instance may nest before expansion gives up.
///
/// A component whose body instantiates itself would otherwise expand forever.
/// The limit is generous: real component trees are a handful deep.
const MAX_DEPTH: usize = 16;

/// The containers that may be a slot: a slot's layout is its container's.
const SLOT_CONTAINERS: &[&str] = &["frame", "stack", "row", "col", "grid"];

/// Attributes an instance applies to the expanded body's root rather than
/// treating as a prop override.
///
/// These place and size the instance itself, so they belong to the box the
/// component becomes.
const POSITIONAL: &[&str] = &[
    "component",
    "name",
    "id",
    "source-id",
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

/// Every component and component set one `<components>` scope declares.
#[derive(Debug, Clone, Default)]
pub(crate) struct Components {
    by_id: BTreeMap<String, Component>,
    /// A component set's id and its variants' ids, which `slot-accept` expands
    /// a set id to.
    sets: BTreeMap<String, Vec<String>>,
}

impl Components {
    /// Every id this scope declares, components and sets alike.
    pub(crate) fn ids(&self) -> impl Iterator<Item = &String> {
        self.by_id.keys().chain(self.sets.keys())
    }

    pub(crate) fn contains(&self, id: &str) -> bool {
        self.by_id.contains_key(id) || self.sets.contains_key(id)
    }

    /// Each component's body, for checking what it refers to.
    pub(crate) fn bodies(&self) -> impl Iterator<Item = (&String, &GuiNode)> {
        self.by_id
            .iter()
            .map(|(id, component)| (id, &component.body))
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Component {
    props: Vec<Prop>,
    body: GuiNode,
    /// The names of the slots the body declares.
    slots: Vec<String>,
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

/// Where instance lookups resolve: the document's own components, then the
/// library's.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Scope<'a> {
    pub(crate) local: &'a Components,
    pub(crate) library: Option<&'a Components>,
}

impl<'a> Scope<'a> {
    /// The component `id` names, and the scope its own body resolves in.
    fn lookup(self, id: &str) -> Option<(&'a Component, Scope<'a>)> {
        if let Some(component) = self.local.by_id.get(id) {
            return Some((component, self));
        }
        let library = self.library?;
        let component = library.by_id.get(id)?;
        Some((
            component,
            Scope {
                local: library,
                library: None,
            },
        ))
    }

    /// The component ids an entry in `slot-accept` stands for: itself, or
    /// every variant when it names a set.
    fn accepted(self, id: &str) -> Vec<String> {
        let set = self
            .local
            .sets
            .get(id)
            .or_else(|| self.library.and_then(|library| library.sets.get(id)));
        match set {
            Some(variants) => variants.clone(),
            None => vec![id.to_owned()],
        }
    }
}

/// Collects every component and variant a `<components>` block declares.
///
/// A `<component-set>`'s `<variant>` children are components in their own
/// right — an instance references a variant by its own id, not the set's.
pub(crate) fn read_components(blocks: &[GuiNode], issues: &mut Issues) -> Components {
    let mut components = Components::default();

    for block in blocks {
        for child in &block.children {
            match child.tag.as_str() {
                "component" => insert_component(&mut components, child, issues),
                "component-set" => {
                    let mut variants = Vec::new();
                    for variant in &child.children {
                        if variant.tag == "variant" {
                            if let Some(id) = variant.attributes.get("id") {
                                variants.push(id.clone());
                            }
                            insert_component(&mut components, variant, issues);
                        }
                    }
                    if let Some(id) = child.attributes.get("id") {
                        components.sets.insert(id.clone(), variants);
                    }
                }
                _ => {}
            }
        }
    }

    components
}

fn insert_component(components: &mut Components, node: &GuiNode, issues: &mut Issues) {
    let Some(id) = node.attributes.get("id") else {
        return;
    };
    // The body is the one child that is not the props block.
    let Some(body) = node.children.iter().find(|child| child.tag != "props") else {
        return;
    };

    components.by_id.insert(
        id.clone(),
        Component {
            props: read_props(id, node, body, issues),
            slots: read_slots(id, body, issues),
            body: body.clone(),
        },
    );
}

/// The slots a component body declares, checked against RFC-0044's rules for
/// declaring one.
fn read_slots(component: &str, body: &GuiNode, issues: &mut Issues) -> Vec<String> {
    if body.attributes.contains_key("slot") {
        issues.violation(format!(
            "component '{component}': a slot may not be the component's root"
        ));
    }

    let mut slots = Vec::new();
    collect_slots(component, body, false, &mut slots, issues);
    slots
}

fn collect_slots(
    component: &str,
    node: &GuiNode,
    inside_slot: bool,
    slots: &mut Vec<String>,
    issues: &mut Issues,
) {
    for child in &node.children {
        let Some(name) = child.attributes.get("slot") else {
            collect_slots(component, child, inside_slot, slots, issues);
            continue;
        };

        if inside_slot {
            issues.violation(format!(
                "component '{component}': slot '{name}' is declared inside another slot"
            ));
        }
        if !SLOT_CONTAINERS.contains(&child.tag.as_str()) {
            issues.violation(format!(
                "component '{component}': slot '{name}' is on a <{}>, but only a layout container can be a slot",
                child.tag
            ));
        }
        if name.trim().is_empty() {
            issues.violation(format!("component '{component}': a slot has an empty name"));
        } else if slots.contains(name) {
            issues.violation(format!(
                "component '{component}': slot '{name}' is declared twice"
            ));
        } else {
            slots.push(name.clone());
        }

        collect_slots(component, child, true, slots, issues);
    }
}

/// RFC-0034's prop types: data types.
const PROP_TYPES: &[&str] = &[
    "string",
    "boolean",
    "number",
    "color",
    "image",
    "component",
    "style",
];
/// The 0.1 names, still rendered, and what replaced them.
const LEGACY_PROP_TYPES: &[(&str, &str)] = &[
    ("text", "string"),
    ("visible", "boolean"),
    ("fill", "color"),
    ("src", "image"),
];
const STYLE_BINDS: &[&str] = &["text-style", "fill-style", "effect-style", "stroke-style"];

/// A component's props, checked against RFC-0034. A string prop may omit
/// `target` when its name is the id of exactly one `<text>` in the body.
fn read_props(component: &str, node: &GuiNode, body: &GuiNode, issues: &mut Issues) -> Vec<Prop> {
    let Some(props) = node.children.iter().find(|child| child.tag == "props") else {
        return Vec::new();
    };

    let mut out = Vec::new();
    for prop in props.children.iter().filter(|child| child.tag == "prop") {
        let (Some(name), Some(kind)) = (prop.attributes.get("name"), prop.attributes.get("type"))
        else {
            continue;
        };
        let bind = prop.attributes.get("bind").cloned();
        if matches!(kind.as_str(), "color" | "fill") && bind.as_deref() == Some("stroke") {
            issues.advise(format!(
                "component '{component}': <prop bind=\"stroke\"> is the old name; use bind=\"border-color\""
            ));
        }

        if let Some((_, new)) = LEGACY_PROP_TYPES.iter().find(|(old, _)| old == kind) {
            issues.advise(format!(
                "component '{component}': <prop type=\"{kind}\"> is the old name; use type=\"{new}\""
            ));
        } else if !PROP_TYPES.contains(&kind.as_str()) {
            issues.violation(format!(
                "component '{component}': <prop type=\"{kind}\"> must be one of {}",
                PROP_TYPES.join(", ")
            ));
        }
        if matches!(kind.as_str(), "number" | "style") && bind.is_none() {
            issues.violation(format!(
                "component '{component}': <prop name=\"{name}\" type=\"{kind}\"> needs bind: a {kind} could mean several attributes"
            ));
        }
        if let Some(bind) = bind.as_deref().filter(|_| kind == "style") {
            if !STYLE_BINDS.contains(&bind) {
                issues.violation(format!(
                    "component '{component}': <prop type=\"style\"> bind must be one of {}, got \"{bind}\"",
                    STYLE_BINDS.join(", ")
                ));
            }
        }

        let targets: Vec<String> = match prop.attributes.get("target") {
            Some(target) => target.split_whitespace().map(ToOwned::to_owned).collect(),
            None if matches!(kind.as_str(), "string" | "text")
                && count_text_ids(body, name) == 1 =>
            {
                vec![name.clone()]
            }
            None => {
                issues.violation(format!(
                    "component '{component}': <prop name=\"{name}\"> needs target: only a string prop named for exactly one <text> id may omit it"
                ));
                continue;
            }
        };

        out.push(Prop {
            name: name.clone(),
            kind: kind.clone(),
            targets,
            bind,
        });
    }
    out
}

fn count_text_ids(node: &GuiNode, id: &str) -> usize {
    let own =
        usize::from(node.tag == "text" && node.attributes.get("id").is_some_and(|it| it == id));
    own + node
        .children
        .iter()
        .map(|child| count_text_ids(child, id))
        .sum::<usize>()
}

/// Layout containers, the only tags that may say what they were detached from.
const DETACHED_TAGS: &[&str] = &["frame", "stack", "row", "col", "grid", "group"];

/// RFC-0035: `detached-from` names a component or variant the document or its
/// library declares, and sits on layout containers only; `component` belongs
/// to `<instance>` alone. Errors from 0.3.8 (`strict`), advice before. The
/// document's tree and its own component bodies are checked as written,
/// before instances are expanded.
pub(crate) fn check_detached(root: &GuiNode, scope: Scope, strict: bool, issues: &mut Issues) {
    fn visit(node: &GuiNode, scope: Scope, report: &mut dyn FnMut(String)) {
        if let Some(origin) = node.attributes.get("detached-from") {
            if !DETACHED_TAGS.contains(&node.tag.as_str()) {
                report(format!(
                    "<{}> cannot carry detached-from; only a layout container can",
                    node.tag
                ));
            } else if scope.lookup(origin).is_none() {
                report(format!(
                    "detached-from=\"{origin}\" names no component or variant declared in the document or its library"
                ));
            }
        }
        if node.tag != "instance" {
            if let Some(id) = node.attributes.get("component") {
                report(format!(
                    "<{} component=\"{id}\">: component is only valid on <instance>; a detached container uses detached-from",
                    node.tag
                ));
            }
        }
        for child in &node.children {
            visit(child, scope, report);
        }
    }

    let mut report = |message: String| {
        if strict {
            issues.error(message);
        } else {
            issues.advise(message);
        }
    };
    visit(root, scope, &mut report);
    for (_, body) in scope.local.bodies() {
        visit(body, scope, &mut report);
    }
}

/// Replaces every `<instance>` in the tree with the component it names.
pub(crate) fn expand(node: &mut GuiNode, scope: Scope, issues: &mut Issues) {
    let mut expander = Expander {
        issues,
        stack: Vec::new(),
    };
    expander.expand_at(node, scope, 0);
}

/// One instance's `<slot>` content: as written, for checking `slot-accept`,
/// and expanded in the scope of the document that wrote it.
struct Fill {
    written: Vec<GuiNode>,
    expanded: Vec<GuiNode>,
}

struct Expander<'i> {
    issues: &'i mut Issues,
    /// The components being instantiated, outermost first.
    stack: Vec<String>,
}

impl Expander<'_> {
    fn expand_at(&mut self, node: &mut GuiNode, scope: Scope, depth: usize) {
        if depth > MAX_DEPTH {
            return;
        }

        // Most nodes hold no instance at all, and rebuilding their children
        // would be an allocation per node for nothing.
        if !node
            .children
            .iter()
            .any(|child| matches!(child.tag.as_str(), "instance" | "slot"))
        {
            for child in &mut node.children {
                self.expand_at(child, scope, depth);
            }
            return;
        }

        let mut expanded = Vec::with_capacity(node.children.len());
        for mut child in std::mem::take(&mut node.children) {
            match child.tag.as_str() {
                // An instance that cannot be expanded is dropped rather than
                // left in the tree, where it would lay out as an unknown block.
                "instance" => expanded.extend(self.instantiate(&child, scope, depth, &[])),
                "slot" => self.issues.violation(format!(
                    "<slot name=\"{}\"> is only valid directly inside an <instance>",
                    child.attributes.get("name").map_or("", String::as_str)
                )),
                _ => {
                    self.expand_at(&mut child, scope, depth);
                    expanded.push(child);
                }
            }
        }
        node.children = expanded;
    }

    fn instantiate(
        &mut self,
        node: &GuiNode,
        scope: Scope,
        depth: usize,
        handed: &[Handed],
    ) -> Option<GuiNode> {
        let Some(id) = node.attributes.get("component") else {
            self.issues
                .violation("an <instance> has no component attribute");
            return None;
        };
        let Some((component, body_scope)) = scope.lookup(id) else {
            self.issues
                .violation(format!("<instance> names unknown component '{id}'"));
            return None;
        };
        // RFC-0044 makes a component inside its own slot content an error, and
        // with it any other self-containment. A 0.2 document keeps the old
        // behaviour: expansion stops at the depth limit.
        if self.issues.strict() && self.stack.contains(id) {
            self.issues.violation(format!(
                "component '{id}' contains itself: {} → {id}",
                self.stack.join(" → ")
            ));
            return None;
        }

        let fills = self.read_fills(node, id, component);

        let mut body = component.body.clone();
        let mut down = apply_declared_props(&mut body, node, &component.props);
        apply_ad_hoc_overrides(&mut body, node, &component.props);
        // Values handed down a path by an enclosing component come last, so
        // they win over this instance's own.
        for value in handed {
            match value.path.split_once('/') {
                None => apply_override(
                    &mut body,
                    &value.path,
                    &value.kind,
                    &value.value,
                    value.bind.as_deref(),
                ),
                Some((first, rest)) => down.push((first.to_owned(), value.rest(rest))),
            }
        }
        apply_positional(&mut body, node);
        scale_to_instance(&mut body, &component.body);

        self.stack.push(id.clone());

        // Content is expanded where it was written, before it is inserted: a
        // document fills a library component's slot with its own components,
        // which the library's scope cannot see. Props are already applied, so
        // content never reads them.
        let mut fills: BTreeMap<String, Fill> = fills
            .into_iter()
            .map(|(name, written)| {
                let mut holder = GuiNode::new("slot");
                holder.children = written.clone();
                self.expand_at(&mut holder, scope, depth + 1);
                (
                    name,
                    Fill {
                        written,
                        expanded: holder.children,
                    },
                )
            })
            .collect();
        // Almost every component declares no slot, and its body need not be
        // walked for one.
        if !component.slots.is_empty() {
            self.fill_slots(&mut body, &mut fills, id, body_scope);
        }

        // Instances a path reaches into are expanded first, with the values
        // meant for them; the rest of the body after.
        if !down.is_empty() {
            self.expand_targeted(&mut body, &down, body_scope, depth + 1);
        }
        // A component body may itself hold instances.
        self.expand_at(&mut body, body_scope, depth + 1);
        self.stack.pop();
        Some(body)
    }

    /// Expands each instance in `node` that a path names, handing it the values
    /// addressed to it. An instance's children are slot content, written in
    /// another scope, so they are not searched.
    fn expand_targeted(
        &mut self,
        node: &mut GuiNode,
        down: &[(String, Handed)],
        scope: Scope,
        depth: usize,
    ) {
        let mut children = Vec::with_capacity(node.children.len());
        for mut child in std::mem::take(&mut node.children) {
            let id = child.attributes.get("id");
            let handed: Vec<Handed> = match id {
                Some(id) if child.tag == "instance" => down
                    .iter()
                    .filter(|(first, _)| first == id)
                    .map(|(_, value)| value.clone())
                    .collect(),
                _ => Vec::new(),
            };
            if !handed.is_empty() {
                children.extend(self.instantiate(&child, scope, depth, &handed));
                continue;
            }
            if child.tag != "instance" {
                self.expand_targeted(&mut child, down, scope, depth);
            }
            children.push(child);
        }
        node.children = children;
    }

    /// An instance's children, by slot name, checked against the rules for
    /// filling one.
    fn read_fills(
        &mut self,
        instance: &GuiNode,
        id: &str,
        component: &Component,
    ) -> BTreeMap<String, Vec<GuiNode>> {
        let mut fills = BTreeMap::new();

        for child in &instance.children {
            if child.tag != "slot" {
                self.issues.violation(format!(
                    "<instance component=\"{id}\"> may only contain <slot> elements, found <{}>",
                    child.tag
                ));
                continue;
            }
            let Some(name) = child
                .attributes
                .get("name")
                .filter(|n| !n.trim().is_empty())
            else {
                self.issues.violation(format!(
                    "<instance component=\"{id}\"> has a <slot> without a name"
                ));
                continue;
            };
            if !component.slots.contains(name) {
                self.issues.violation(format!(
                    "<instance component=\"{id}\"> fills slot '{name}', which the component does not declare"
                ));
                continue;
            }
            if fills.contains_key(name) {
                self.issues.violation(format!(
                    "<instance component=\"{id}\"> fills slot '{name}' twice"
                ));
                continue;
            }
            fills.insert(name.clone(), child.children.clone());
        }

        fills
    }

    /// Puts each fill into the slot it names, keeps the fallback of slots
    /// left unfilled, and removes every slot that ends up empty.
    fn fill_slots(
        &mut self,
        node: &mut GuiNode,
        fills: &mut BTreeMap<String, Fill>,
        component: &str,
        scope: Scope,
    ) {
        let mut kept = Vec::with_capacity(node.children.len());

        for mut child in std::mem::take(&mut node.children) {
            let Some(name) = child.attributes.get("slot").cloned() else {
                self.fill_slots(&mut child, fills, component, scope);
                kept.push(child);
                continue;
            };

            if let Some(fill) = fills.remove(&name) {
                self.check_accept(&child, &fill.written, component, &name, scope);
                child.children = fill.expanded;
            }

            // An empty slot is not an empty box: no size, no padding, and no
            // share of its parent's gap.
            if !child.children.is_empty() {
                kept.push(child);
            }
        }

        node.children = kept;
    }

    fn check_accept(
        &mut self,
        slot: &GuiNode,
        content: &[GuiNode],
        component: &str,
        name: &str,
        scope: Scope,
    ) {
        if let Some(accept) = slot.attributes.get("slot-accept") {
            let accepted: Vec<String> = accept
                .split_whitespace()
                .flat_map(|id| scope.accepted(id))
                .collect();

            for child in content {
                // A container detached from an accepted component still fits
                // (RFC-0035).
                let origin = if child.tag == "instance" {
                    child.attributes.get("component")
                } else {
                    child.attributes.get("detached-from")
                };
                let fits = origin.is_some_and(|id| accepted.contains(id));
                if !fits {
                    let what = match child.attributes.get("component") {
                        Some(id) if child.tag == "instance" => format!("an instance of '{id}'"),
                        _ => format!("a <{}>", child.tag),
                    };
                    self.issues.violation(format!(
                        "slot '{name}' of component '{component}' accepts only instances of {accept}, but was given {what}"
                    ));
                }
            }
        }

        let count = content.len();
        let bound = |attr: &str| slot.attributes.get(attr)?.trim().parse::<usize>().ok();
        if let Some(min) = bound("slot-min").filter(|min| count < *min) {
            self.issues.advise(format!(
                "slot '{name}' of component '{component}' has {count} item(s), fewer than slot-min {min}"
            ));
        }
        if let Some(max) = bound("slot-max").filter(|max| count > *max) {
            self.issues.advise(format!(
                "slot '{name}' of component '{component}' has {count} item(s), more than slot-max {max}"
            ));
        }
    }
}

/// A prop value on its way down a path (RFC-0046): `tab-1/label` cannot be
/// reached until `tab-1` is expanded, so it travels with that instance.
#[derive(Debug, Clone)]
struct Handed {
    /// What is left of the path, below the instance it is handed to.
    path: String,
    kind: String,
    bind: Option<String>,
    value: String,
}

impl Handed {
    fn rest(&self, path: &str) -> Handed {
        Handed {
            path: path.to_owned(),
            ..self.clone()
        }
    }
}

/// Applies each declared prop the instance sets. A target that is a path is
/// returned instead, keyed by the instance it passes through.
fn apply_declared_props(
    body: &mut GuiNode,
    instance: &GuiNode,
    props: &[Prop],
) -> Vec<(String, Handed)> {
    let mut down = Vec::new();
    for prop in props {
        let Some(value) = instance.attributes.get(&prop.name) else {
            continue;
        };
        for target in &prop.targets {
            match target.split_once('/') {
                None => apply_override(body, target, &prop.kind, value, prop.bind.as_deref()),
                Some((first, rest)) => down.push((
                    first.to_owned(),
                    Handed {
                        path: rest.to_owned(),
                        kind: prop.kind.clone(),
                        bind: prop.bind.clone(),
                        value: value.clone(),
                    },
                )),
            }
        }
    }
    down
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
        // `stroke` is the old name for `border-color` (RFC-0022).
        "color" | "fill" => match bind {
            Some("stroke") => "border-color",
            Some(bind) => bind,
            None => "fill",
        },
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
        // `component` names what to expand, and `id` and `source-id` identify
        // the instance; none belongs on the box that replaces it.
        if matches!(*name, "component" | "id" | "source-id") {
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

    /// A chip component with the given props, placed once with `instance`.
    fn chip(version: &str, props: &str, instance: &str) -> String {
        format!(
            r##"<gui version="{version}">
              <components>
                <component id="comp-chip">
                  <props>{props}</props>
                  <row><text id="label" value="Default label" /><text id="note" value="n" /><rect id="surface" w="10" h="10" /></row>
                </component>
              </components>
              <col w="200">{instance}</col>
            </gui>"##
        )
    }

    #[test]
    fn a_string_prop_named_for_one_text_id_needs_no_target() {
        let root = root_of(&chip(
            "0.3",
            r#"<prop name="label" type="string" />"#,
            r#"<instance component="comp-chip" label="Implicit works" />"#,
        ));
        let label = find(&root.children[0], "label").unwrap();
        assert_eq!(label.attributes["value"], "Implicit works");
    }

    #[test]
    fn prop_rules_are_errors_in_0_3() {
        let cases = [
            (
                r#"<prop name="r" type="number" target="surface" />"#,
                "needs bind: a number",
            ),
            (
                r#"<prop name="ts" type="style" target="label" />"#,
                "needs bind: a style",
            ),
            (
                r#"<prop name="b" type="style" target="label" bind="radius" />"#,
                "bind must be one of",
            ),
            (
                r#"<prop name="x" type="enum" target="label" />"#,
                r#"<prop type="enum"> must be one of"#,
            ),
            (r#"<prop name="title" type="string" />"#, "needs target"),
            (r#"<prop name="label" type="color" />"#, "needs target"),
        ];
        for (prop, expected) in cases {
            let err = parse_err(&chip("0.3", prop, r#"<instance component="comp-chip" />"#));
            assert!(err.contains(expected), "{prop}\n→ {err}");
        }
    }

    #[test]
    fn every_data_type_and_the_old_names_still_render() {
        let props = r#"
            <prop name="label" type="string" target="label" />
            <prop name="off" type="boolean" target="surface" />
            <prop name="bg" type="color" target="surface" />
            <prop name="r" type="number" target="surface" bind="radius" />
            <prop name="ts" type="style" target="label" bind="text-style" />
            <prop name="both" type="color" target="label note" />
            <prop name="old" type="text" target="note" />"#;
        let doc = parse_gui_xml(&chip(
            "0.3",
            props,
            r#"<instance component="comp-chip" old="Old name" />"#,
        ))
        .expect("valid props render");
        assert!(
            doc.warnings
                .iter()
                .any(|w| w.contains(r#"type="text"> is the old name"#)),
            "{:?}",
            doc.warnings
        );
        assert_eq!(
            find(&doc.root.children[0], "note").unwrap().attributes["value"],
            "Old name"
        );
    }

    #[test]
    fn prop_rules_only_warn_in_0_2() {
        assert!(parse_gui_xml(&chip(
            "0.2",
            r#"<prop name="r" type="number" target="surface" />"#,
            r#"<instance component="comp-chip" />"#
        ))
        .is_ok());
    }

    fn parse_err(xml: &str) -> String {
        match parse_gui_xml(xml) {
            Ok(_) => panic!("expected the document to be refused"),
            Err(err) => err.to_string(),
        }
    }

    /// RFC-0044's scaffold: a title prop, a free `body` slot and a `tabs`
    /// slot that accepts only tab items.
    const SCREEN: &str = r##"
        <components>
          <component-set id="compset-tab-item" name="Tab">
            <variant id="comp-tab-item">
              <text value="Tab" />
            </variant>
            <variant id="comp-tab-item-active">
              <text value="Tab" font-weight="700" />
            </variant>
          </component-set>
          <component id="comp-section">
            <text id="label" value="Section" />
          </component>
          <component name="Screen" id="comp-screen">
            <props>
              <prop name="title" type="string" target="title" />
            </props>
            <col w="390" gap="22">
              <text id="title" value="Title" />
              <col id="body" slot="body" w="fill" p="0 16" gap="24" />
              <row id="tabs" slot="tabs" slot-accept="compset-tab-item" slot-min="2" slot-max="5" w="fill" gap="8" />
              <row id="actions" slot="actions" gap="12">
                <text id="close" value="Close" />
              </row>
            </col>
          </component>
        </components>
    "##;

    fn screen(version: &str, instance: &str) -> String {
        format!(
            r##"
            <gui version="{version}">
              {SCREEN}
              <frame w="390" h="800">
                {instance}
              </frame>
            </gui>
            "##
        )
    }

    #[test]
    fn slot_content_lands_in_the_container_and_takes_its_layout() {
        let root = root_of(&screen(
            "0.3",
            r#"
            <instance component="comp-screen" title="Audio Settings">
              <slot name="body">
                <instance component="comp-section" label="Playback" />
                <row id="toggles" gap="12"><text value="Lossless" /></row>
              </slot>
              <slot name="tabs">
                <instance component="comp-tab-item" />
                <instance component="comp-tab-item-active" />
              </slot>
            </instance>
            "#,
        ));

        let screen = &root.children[0];
        let body = find(screen, "body").expect("the body slot is kept");
        assert_eq!(
            body.attributes["gap"], "24",
            "the slot keeps its own layout"
        );
        assert_eq!(body.children.len(), 2);
        assert_eq!(
            body.children[0].tag, "text",
            "an instance in content is expanded"
        );
        assert_eq!(body.children[0].attributes["value"], "Playback");
        assert_eq!(body.children[1].attributes["id"], "toggles");

        assert_eq!(find(screen, "tabs").unwrap().children.len(), 2);
        assert_eq!(
            find(screen, "title").unwrap().attributes["value"],
            "Audio Settings"
        );
    }

    #[test]
    fn an_unfilled_slot_keeps_its_fallback() {
        let root = root_of(&screen("0.3", r#"<instance component="comp-screen" />"#));
        let actions = find(&root.children[0], "actions").expect("fallback keeps the slot");
        assert_eq!(actions.children[0].attributes["value"], "Close");
    }

    #[test]
    fn a_filled_slot_replaces_its_fallback() {
        let root = root_of(&screen(
            "0.3",
            r#"
            <instance component="comp-screen">
              <slot name="actions"><text id="save" value="Save" /></slot>
            </instance>
            "#,
        ));
        let actions = find(&root.children[0], "actions").unwrap();
        assert_eq!(actions.children.len(), 1);
        assert_eq!(actions.children[0].attributes["id"], "save");
    }

    #[test]
    fn an_empty_slot_leaves_no_box_behind() {
        let root = root_of(&screen(
            "0.3",
            r#"
            <instance component="comp-screen">
              <slot name="actions" />
            </instance>
            "#,
        ));
        let screen = &root.children[0];
        assert!(
            find(screen, "body").is_none(),
            "unfilled, no fallback: gone"
        );
        assert!(find(screen, "tabs").is_none());
        assert!(
            find(screen, "actions").is_none(),
            "filled with nothing: gone"
        );
        assert_eq!(
            screen.children.len(),
            1,
            "only the title is left to take a gap"
        );
    }

    #[test]
    fn slot_content_does_not_read_the_components_props() {
        // `title` targets the layer with id="title"; content carrying the same
        // id is the document's, and the prop must not reach it.
        let root = root_of(&screen(
            "0.3",
            r#"
            <instance component="comp-screen" title="From prop">
              <slot name="body"><text id="title" value="From content" /></slot>
            </instance>
            "#,
        ));
        let body = find(&root.children[0], "body").unwrap();
        assert_eq!(body.children[0].attributes["value"], "From content");
    }

    #[test]
    fn slot_rules_are_errors_in_0_3() {
        let cases = [
            (
                r#"<instance component="comp-screen"><text value="bare" /></instance>"#,
                "may only contain <slot>",
            ),
            (
                r#"<instance component="comp-screen"><slot><text value="x" /></slot></instance>"#,
                "without a name",
            ),
            (
                r#"<instance component="comp-screen"><slot name="footer"><text value="x" /></slot></instance>"#,
                "does not declare",
            ),
            (
                r#"<instance component="comp-screen"><slot name="tabs"><text value="x" /></slot></instance>"#,
                "accepts only instances of compset-tab-item, but was given a <text>",
            ),
            (
                r#"<instance component="comp-screen"><slot name="tabs"><instance component="comp-section" /></slot></instance>"#,
                "was given an instance of 'comp-section'",
            ),
            (
                r#"<slot name="body"><text value="x" /></slot>"#,
                "only valid directly inside",
            ),
            (
                r#"<instance component="nope" />"#,
                "unknown component 'nope'",
            ),
        ];

        for (instance, expected) in cases {
            let err = parse_err(&screen("0.3", instance));
            assert!(err.contains(expected), "{instance}\n→ {err}");
        }
    }

    #[test]
    fn the_same_findings_only_warn_in_0_2() {
        let document = parse_gui_xml(&screen(
            "0.2",
            r#"<instance component="comp-screen"><text value="bare" /></instance>"#,
        ))
        .expect("a 0.2 document still renders");
        assert!(document
            .warnings
            .iter()
            .any(|w| w.contains("may only contain <slot>")));
    }

    #[test]
    fn slot_min_and_max_advise_but_do_not_fail() {
        let document = parse_gui_xml(&screen(
            "0.3",
            r#"
            <instance component="comp-screen">
              <slot name="tabs"><instance component="comp-tab-item" /></slot>
            </instance>
            "#,
        ))
        .expect("slot-min is advisory");
        assert!(document
            .warnings
            .iter()
            .any(|w| w.contains("fewer than slot-min 2")));
    }

    #[test]
    fn a_slot_declaration_is_checked_where_it_is_declared() {
        let declare = |body: &str| {
            parse_err(&format!(
                r#"<gui version="0.3">
                  <components><component id="c">{body}</component></components>
                  <frame w="10" h="10"><instance component="c" /></frame>
                </gui>"#
            ))
        };

        assert!(declare(r#"<col slot="root"><text value="x" /></col>"#)
            .contains("may not be the component's root"));
        assert!(
            declare(r#"<col><col slot="a"><row slot="b" /></col></col>"#)
                .contains("inside another slot")
        );
        assert!(declare(r#"<col><text slot="a" value="x" /></col>"#)
            .contains("only a layout container"));
        assert!(
            declare(r#"<col><col slot="a" /><row slot="a" /></col>"#).contains("declared twice")
        );
    }

    #[test]
    fn a_component_inside_its_own_slot_content_is_an_error() {
        let err = parse_err(
            r#"<gui version="0.3">
              <components>
                <component id="card"><col><col slot="body" /></col></component>
              </components>
              <frame w="10" h="10">
                <instance component="card">
                  <slot name="body"><instance component="card" /></slot>
                </instance>
              </frame>
            </gui>"#,
        );
        assert!(err.contains("contains itself"), "{err}");
    }

    #[test]
    fn a_component_can_pass_its_own_slot_through_to_another() {
        // `page` declares `content` inside the content it gives `card`.
        let root = root_of(
            r#"<gui version="0.3">
              <components>
                <component id="card"><col id="card"><col id="card-body" slot="body" p="8" /></col></component>
                <component id="page">
                  <col id="page">
                    <instance component="card">
                      <slot name="body"><col id="content" slot="content" gap="4" /></slot>
                    </instance>
                  </col>
                </component>
              </components>
              <frame w="100" h="100">
                <instance component="page">
                  <slot name="content"><text id="hello" value="Hello" /></slot>
                </instance>
                <instance component="page" />
              </frame>
            </gui>"#,
        );

        let filled = &root.children[0];
        let content = find(filled, "content").expect("the pass-through slot is filled");
        assert_eq!(content.children[0].attributes["id"], "hello");
        assert!(find(filled, "card-body").is_some());

        let empty = &root.children[1];
        assert!(find(empty, "content").is_none(), "unfilled, it collapses");
        assert!(
            find(empty, "card-body").is_none(),
            "and so does the slot it was the only content of"
        );
    }

    /// RFC-0046: a tab bar exposing its tabs' labels and dot colors by path.
    fn tabbar(props: &str, instance: &str) -> String {
        format!(
            r##"
            <gui version="0.3.7">
              <components>
                <component id="comp-tab-item">
                  <props><prop name="tint" type="color" target="dot" /></props>
                  <row id="tab"><rect id="dot" w="8" h="8" fill="#000000" /><text id="label" value="Tab" /></row>
                </component>
                <component id="comp-tabbar">
                  <props>{props}</props>
                  <row id="bar" gap="8">
                    <instance id="tab-1" component="comp-tab-item" />
                    <instance id="tab-2" component="comp-tab-item" tint="#00FF00" />
                  </row>
                </component>
              </components>
              <col w="400">{instance}</col>
            </gui>
            "##
        )
    }

    fn tabs(root: &crate::GuiNode) -> Vec<&crate::GuiNode> {
        root.children[0].children.iter().collect()
    }

    fn attr<'a>(node: &'a crate::GuiNode, id: &str, name: &str) -> Option<&'a str> {
        find(node, id)
            .and_then(|it| it.attributes.get(name))
            .map(String::as_str)
    }

    #[test]
    fn a_path_sets_a_node_inside_a_nested_instance() {
        let root = root_of(&tabbar(
            r#"<prop name="first-label" type="string" target="tab-1/label" />
               <prop name="second-label" type="string" target="tab-2/label" />"#,
            r#"<instance component="comp-tabbar" first-label="Library" second-label="Settings" />"#,
        ));
        let tabs = tabs(&root);
        assert_eq!(tabs.len(), 2);
        assert_eq!(attr(tabs[0], "label", "value"), Some("Library"));
        assert_eq!(attr(tabs[1], "label", "value"), Some("Settings"));
    }

    #[test]
    fn a_value_handed_down_a_path_wins_over_the_nested_instances_own() {
        let root = root_of(&tabbar(
            r#"<prop name="second-tint" type="color" target="tab-2/dot" />"#,
            r##"<instance component="comp-tabbar" second-tint="#FF0000" />"##,
        ));
        let tabs = tabs(&root);
        assert_eq!(attr(tabs[0], "dot", "fill"), Some("#000000"));
        assert_eq!(attr(tabs[1], "dot", "fill"), Some("#FF0000"));
    }

    #[test]
    fn a_path_that_does_not_reach_an_instance_changes_nothing() {
        let root = root_of(&tabbar(
            r#"<prop name="x" type="string" target="bar/label" />"#,
            r#"<instance component="comp-tabbar" x="Nope" />"#,
        ));
        for tab in tabs(&root) {
            assert_eq!(attr(tab, "label", "value"), Some("Tab"));
        }
    }

    #[test]
    fn an_instances_id_and_source_id_stay_off_the_body() {
        let root = root_of(
            &tabbar(
                "",
                r#"<instance id="nav" source-id="1:2" component="comp-tabbar" />"#,
            )
            .replace(
                r#"<gui version="0.3.7">"#,
                r#"<gui version="0.3.7" source="figma">"#,
            ),
        );
        let bar = &root.children[0];
        assert_eq!(bar.attributes.get("id").map(String::as_str), Some("bar"));
        assert!(!bar.attributes.contains_key("source-id"));
    }

    /// RFC-0035: a heading component, a tab set, and a tab bar whose slot
    /// accepts only tab items.
    fn detached(version: &str, body: &str) -> String {
        format!(
            r##"
            <gui version="{version}">
              <components>
                <component id="comp-heading"><col><text value="Heading" /></col></component>
                <component-set id="compset-tab-item">
                  <variant id="comp-tab-item"><row><text value="Tab" /></row></variant>
                  <variant id="comp-tab-item-active"><row><text value="Tab" /></row></variant>
                </component-set>
                <component id="comp-other"><row /></component>
                <component id="comp-tabbar"><col><row slot="tabs" slot-accept="compset-tab-item" /></col></component>
              </components>
              <col w="390">{body}</col>
            </gui>
            "##
        )
    }

    #[test]
    fn detached_from_changes_nothing_on_screen() {
        let with = root_of(&detached(
            "0.3.8",
            r#"<col detached-from="comp-heading"><text value="SEO" /></col>"#,
        ));
        let without = root_of(&detached("0.3.8", r#"<col><text value="SEO" /></col>"#));
        assert_eq!(with.children[0].children, without.children[0].children);
        assert_eq!(
            with.children[0]
                .attributes
                .get("detached-from")
                .map(String::as_str),
            Some("comp-heading")
        );
    }

    #[test]
    fn detached_from_must_name_a_declared_component_from_0_3_8() {
        for origin in ["comp-missing", "compset-tab-item"] {
            let body = format!(r#"<col detached-from="{origin}" />"#);
            assert!(
                parse_err(&detached("0.3.8", &body)).contains("names no component"),
                "{origin}"
            );
            let older = parse_gui_xml(&detached("0.3.7", &body)).expect("only advice before 0.3.8");
            assert!(older
                .warnings
                .iter()
                .any(|w| w.contains("names no component")));
        }
        assert!(parse_gui_xml(&detached(
            "0.3.8",
            r#"<row detached-from="comp-tab-item-active" />"#
        ))
        .is_ok());
    }

    #[test]
    fn detached_from_and_component_stay_on_their_own_tags() {
        assert!(parse_err(&detached(
            "0.3.8",
            r#"<text value="x" detached-from="comp-heading" />"#
        ))
        .contains("cannot carry detached-from"));
        assert!(
            parse_err(&detached("0.3.8", r#"<col component="comp-heading" />"#))
                .contains("component is only valid on <instance>")
        );
    }

    #[test]
    fn slot_accept_takes_a_container_detached_from_an_accepted_component() {
        let fill = |child: &str| {
            detached(
                "0.3.8",
                &format!(
                    r#"<instance component="comp-tabbar"><slot name="tabs">{child}</slot></instance>"#
                ),
            )
        };
        assert!(parse_gui_xml(&fill(
            r#"<row detached-from="comp-tab-item-active"><text value="Hot" /></row>"#
        ))
        .is_ok());
        assert!(parse_err(&fill(r#"<row detached-from="comp-other" />"#)).contains("accepts only"));
        assert!(parse_err(&fill("<row />")).contains("accepts only"));
    }

    #[test]
    fn a_color_prop_bound_to_border_color_or_stroke_recolors_the_border() {
        for bind in ["border-color", "stroke"] {
            let doc = parse_gui_xml(&format!(
                r##"
                <gui version="0.3.10">
                  <components><component id="comp-a">
                    <props><prop name="edge" type="color" target="box" bind="{bind}" /></props>
                    <col id="box" w="40" h="40" border="2 #000000" />
                  </component></components>
                  <col w="400"><instance component="comp-a" edge="#FF0000" /></col>
                </gui>
                "##
            ))
            .expect("valid gui");
            let body = &doc.root.children[0];
            assert_eq!(
                body.attributes.get("border-color").map(String::as_str),
                Some("#FF0000"),
                "{bind}"
            );
            assert_eq!(
                doc.warnings.iter().any(|w| w.contains("bind=\"stroke\"")),
                bind == "stroke"
            );
        }
    }
}
