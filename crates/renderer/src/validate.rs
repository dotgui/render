//! Checks on what a document refers to, once its instances are expanded.
//!
//! - RFC-0042: every name a document uses resolves, locally or in the
//!   library. Because resolution is closed inside the package, this is always
//!   decidable at parse time.
//! - RFC-0043: an asset reference is never a `data:` URI, and a document served
//!   on its own references assets only by absolute URL.

use crate::issues::Issues;
use crate::model::{GuiMetadata, GuiNode};
use crate::text_style::resolve_token;

/// Attributes that carry prose or identity rather than a value to resolve, so
/// a `$` in them is just a dollar sign.
const PROSE: &[&str] = &["name", "id", "alt", "title", "placeholder", "data-uid"];

/// Tags whose `value` is the text they draw.
const TEXT_TAGS: &[&str] = &["text", "segment"];

/// Attributes that reference an asset by source.
const ASSET_ATTRIBUTES: &[&str] = &["src", "mask-src"];

/// Reports every `$token`, `text-style` and `effect-style` in the tree that
/// the metadata does not declare, and every `$token` a named style uses.
pub(crate) fn check_references(root: &GuiNode, metadata: &GuiMetadata, issues: &mut Issues) {
    check_styles(metadata, issues);
    check_tree(root, metadata, issues);
}

/// Reports every `$token` a named style uses that the metadata does not declare.
pub(crate) fn check_styles(metadata: &GuiMetadata, issues: &mut Issues) {
    for (name, style) in &metadata.styles {
        for value in style.values() {
            check_tokens_in(value, metadata, &format!("text style '{name}'"), issues);
        }
    }
    for (name, effects) in &metadata.effect_styles {
        for value in effects.iter().flat_map(|effect| effect.values()) {
            check_tokens_in(value, metadata, &format!("effect style '{name}'"), issues);
        }
    }
}

/// Reports every `$token`, `text-style` and `effect-style` in the tree that
/// the metadata does not declare.
pub(crate) fn check_tree(root: &GuiNode, metadata: &GuiMetadata, issues: &mut Issues) {
    check_node(root, metadata, issues);
}

fn check_node(node: &GuiNode, metadata: &GuiMetadata, issues: &mut Issues) {
    let is_text = TEXT_TAGS.contains(&node.tag.as_str());

    for (attribute, value) in &node.attributes {
        let prose = PROSE.contains(&attribute.as_str())
            || attribute.starts_with("data-")
            || (is_text && attribute == "value");
        if !prose {
            check_tokens_in(value, metadata, &format!("<{}>", node.tag), issues);
        }
    }

    let text_style = node
        .attributes
        .get("text-style")
        .or_else(|| is_text.then(|| node.attributes.get("style")).flatten());
    if let Some(style) = text_style {
        if !metadata.styles.contains_key(style) {
            issues.violation(format!(
                "<{}> uses text style '{style}', which is not declared",
                node.tag
            ));
        }
    }
    if let Some(style) = node.attributes.get("effect-style") {
        if !metadata.effect_styles.contains_key(style) {
            issues.violation(format!(
                "<{}> uses effect style '{style}', which is not declared",
                node.tag
            ));
        }
    }

    for child in &node.children {
        check_node(child, metadata, issues);
    }
}

fn check_tokens_in(value: &str, metadata: &GuiMetadata, place: &str, issues: &mut Issues) {
    for name in token_references(value) {
        if !metadata.tokens.contains_key(name) {
            issues.violation(format!(
                "{place} uses token '${name}', which is not declared"
            ));
        }
    }
}

/// The token names a value refers to, as [`resolve_token`] reads them: each
/// whitespace-separated part that starts with `$` and then a name. `$4.99` is
/// a price, not a token.
pub(crate) fn token_references(value: &str) -> impl Iterator<Item = &str> {
    value.split_whitespace().filter_map(|part| {
        let name = part.strip_prefix('$')?;
        name.chars()
            .next()
            .is_some_and(|first| first.is_alphabetic() || first == '_' || first == '-')
            .then_some(name)
    })
}

/// Reports `data:` asset references and, for a standalone document, local
/// ones.
pub(crate) fn check_assets(
    root: &GuiNode,
    metadata: &GuiMetadata,
    standalone: bool,
    issues: &mut Issues,
) {
    for (name, value) in &metadata.tokens {
        if is_data_uri(value) || url_functions(value).any(is_data_uri) {
            issues.violation(format!(
                "token '{name}' holds a data: URI; assets are referenced, never inlined"
            ));
        }
    }
    check_node_assets(root, metadata, standalone, issues);
}

fn check_node_assets(
    node: &GuiNode,
    metadata: &GuiMetadata,
    standalone: bool,
    issues: &mut Issues,
) {
    for (attribute, value) in &node.attributes {
        let value = resolve_token(value, metadata);
        let mut sources: Vec<&str> = url_functions(&value).collect();
        if ASSET_ATTRIBUTES.contains(&attribute.as_str()) {
            sources.push(value.trim());
        }

        for source in sources {
            // An unresolved token is reported as one, not as a path.
            if source.is_empty() || source.starts_with('$') {
                continue;
            }
            if is_data_uri(source) {
                issues.violation(format!(
                    "<{} {attribute}> is a data: URI; assets are referenced, never inlined",
                    node.tag
                ));
            } else if standalone && !is_absolute_url(source) {
                issues.violation(format!(
                    "<{} {attribute}=\"{source}\"> is a local asset path, which a standalone document cannot have; use an absolute URL or package the document",
                    node.tag
                ));
            }
        }
    }

    for child in &node.children {
        check_node_assets(child, metadata, standalone, issues);
    }
}

fn is_data_uri(value: &str) -> bool {
    value
        .trim_start()
        .get(..5)
        .is_some_and(|scheme| scheme.eq_ignore_ascii_case("data:"))
}

fn is_absolute_url(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    lower.starts_with("https://") || lower.starts_with("http://")
}

/// The arguments of every `url(...)` in a value, unquoted.
fn url_functions(value: &str) -> impl Iterator<Item = &str> {
    value.match_indices("url(").filter_map(move |(start, _)| {
        let rest = &value[start + 4..];
        let end = rest.find(')')?;
        Some(rest[..end].trim().trim_matches(|c| c == '"' || c == '\''))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_price_is_not_a_token() {
        let names: Vec<_> = token_references("1 $rule $4.99 $sp-2").collect();
        assert_eq!(names, ["rule", "sp-2"]);
    }

    #[test]
    fn url_functions_are_unquoted() {
        let urls: Vec<_> = url_functions("url('a.png') url(\"https://x/b.png\")").collect();
        assert_eq!(urls, ["a.png", "https://x/b.png"]);
    }
}
