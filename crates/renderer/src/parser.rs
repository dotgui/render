use crate::{FontInfo, GuiDocument, GuiMetadata, GuiNode};
use roxmltree::{Document, Node};
use std::collections::BTreeMap;
use thiserror::Error;

const ROOT_LAYOUT_TAGS: &[&str] = &["frame", "stack", "row", "col", "grid"];
const PRESENCE_ATTRS: &[(&str, &str)] = &[
    ("abs", "true"),
    ("clip", "true"),
    ("gap", "auto"),
    // The spec writes `isolation` as presence-only. CSS spells the value
    // `isolate`, and that is what a document carrying one writes too.
    ("isolation", "isolate"),
    ("mask", "true"),
    ("reverse-z", "true"),
    ("truncate", "true"),
    ("wrap", "true"),
];

#[derive(Debug, Error)]
pub enum ParseError {
    #[error("invalid XML: {0}")]
    Xml(#[from] roxmltree::Error),

    #[error("expected <gui> as the document root, found <{0}>")]
    WrongRoot(String),

    #[error("missing renderer root layout node")]
    MissingRootLayout,

    #[error("multiple root layout nodes found; expected exactly one")]
    MultipleRootLayouts,
}

pub fn parse_gui_xml(xml: &str) -> Result<GuiDocument, ParseError> {
    let normalized = normalize_presence_attrs(xml);
    let doc = Document::parse(&normalized)?;
    let root = doc.root_element();
    if root.tag_name().name() != "gui" {
        return Err(ParseError::WrongRoot(root.tag_name().name().to_owned()));
    }

    let mut metadata = GuiMetadata::default();
    let mut layout_root: Option<GuiNode> = None;

    for child in root.children().filter(Node::is_element) {
        let tag = child.tag_name().name();
        match tag {
            "tokens" => read_tokens(child, &mut metadata),
            "fonts" => read_fonts(child, &mut metadata),
            "styles" => read_styles(child, &mut metadata),
            t if ROOT_LAYOUT_TAGS.contains(&t) => {
                if layout_root.is_some() {
                    return Err(ParseError::MultipleRootLayouts);
                }
                layout_root = Some(read_node(child));
            }
            _ => {}
        }
    }

    Ok(GuiDocument {
        version: attr(root, "version").unwrap_or_else(|| "0.2".to_owned()),
        name: attr(root, "name"),
        metadata,
        root: layout_root.ok_or(ParseError::MissingRootLayout)?,
    })
}

fn normalize_presence_attrs(xml: &str) -> String {
    let mut out = String::with_capacity(xml.len());
    let mut cursor = 0;

    while let Some(tag_start_offset) = xml[cursor..].find('<') {
        let tag_start = cursor + tag_start_offset;
        out.push_str(&xml[cursor..tag_start]);

        if xml[tag_start..].starts_with("<!--") {
            let end = xml[tag_start + 4..]
                .find("-->")
                .map(|offset| tag_start + 4 + offset + 3)
                .unwrap_or(xml.len());
            out.push_str(&xml[tag_start..end]);
            cursor = end;
            continue;
        }

        let Some(tag_end) = find_tag_end(xml, tag_start) else {
            out.push_str(&xml[tag_start..]);
            return out;
        };

        let tag = &xml[tag_start..tag_end];
        if tag.starts_with("</") || tag.starts_with("<?") || tag.starts_with("<!") {
            out.push_str(tag);
        } else {
            out.push_str(&normalize_tag_presence_attrs(tag));
        }
        cursor = tag_end;
    }

    out.push_str(&xml[cursor..]);
    out
}

fn find_tag_end(xml: &str, tag_start: usize) -> Option<usize> {
    let mut quote = None;
    for (offset, ch) in xml[tag_start..].char_indices() {
        match quote {
            Some(q) if ch == q => quote = None,
            Some(_) => {}
            None if ch == '"' || ch == '\'' => quote = Some(ch),
            None if ch == '>' => return Some(tag_start + offset + 1),
            None => {}
        }
    }
    None
}

fn normalize_tag_presence_attrs(tag: &str) -> String {
    let mut out = String::with_capacity(tag.len());
    let chars: Vec<char> = tag.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        let ch = chars[i];
        if ch == '"' || ch == '\'' {
            let quote = ch;
            out.push(ch);
            i += 1;
            while i < chars.len() {
                let inner = chars[i];
                out.push(inner);
                i += 1;
                if inner == quote {
                    break;
                }
            }
            continue;
        }

        if !ch.is_whitespace() {
            out.push(ch);
            i += 1;
            continue;
        }

        while i < chars.len() && chars[i].is_whitespace() {
            out.push(chars[i]);
            i += 1;
        }

        let name_start = i;
        if !chars.get(i).is_some_and(|c| c.is_ascii_alphabetic()) {
            continue;
        }

        while i < chars.len()
            && (chars[i].is_ascii_alphanumeric() || chars[i] == '-' || chars[i] == '_')
        {
            i += 1;
        }

        let name: String = chars[name_start..i].iter().collect();
        let value = presence_value(&name);
        let after = chars.get(i).copied();
        if let Some(value) = value {
            if after.is_none() || after.is_some_and(|c| c.is_whitespace() || c == '/' || c == '>') {
                out.push_str(&name);
                out.push_str("=\"");
                out.push_str(value);
                out.push('"');
                continue;
            }
        }

        out.push_str(&name);
    }

    out
}

fn presence_value(name: &str) -> Option<&'static str> {
    PRESENCE_ATTRS
        .iter()
        .find_map(|(attr, value)| (*attr == name).then_some(*value))
}

fn read_tokens(tokens_el: Node, metadata: &mut GuiMetadata) {
    for child in tokens_el.children().filter(Node::is_element) {
        let Some(name) = attr(child, "name").or_else(|| attr(child, "id")) else {
            continue;
        };
        let value = attr(child, "value").or_else(|| {
            child
                .text()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
        });
        if let Some(value) = value {
            metadata.tokens.insert(name, value);
        }
    }
}

/// Splits a `<styles>` block into text styles and effect styles.
///
/// A `<text-style>` is read as a bag of attributes. An `<effect-style>` holds
/// ordered `<effect>` children instead, so it is collected separately.
fn read_styles(styles_el: Node, metadata: &mut GuiMetadata) {
    for child in styles_el.children().filter(Node::is_element) {
        let Some(name) = attr(child, "name").or_else(|| attr(child, "id")) else {
            continue;
        };

        if child.tag_name().name() == "effect-style" {
            metadata.effect_styles.insert(
                name,
                child
                    .children()
                    .filter(Node::is_element)
                    .filter(|effect| effect.tag_name().name() == "effect")
                    .map(read_attributes)
                    .collect(),
            );
        } else {
            metadata.styles.insert(name, read_attributes(child));
        }
    }
}

fn read_fonts(fonts_el: Node, metadata: &mut GuiMetadata) {
    for child in fonts_el.children().filter(Node::is_element) {
        if child.tag_name().name() != "font" {
            continue;
        }

        let Some(family) = attr(child, "family") else {
            continue;
        };
        let Some(source) = attr(child, "source") else {
            continue;
        };

        metadata.fonts.insert(
            family,
            FontInfo {
                source,
                category: attr(child, "category"),
                weights: attr(child, "weights"),
                styles: attr(child, "styles"),
                variants: attr(child, "variants"),
            },
        );
    }
}

fn read_node(node: Node) -> GuiNode {
    let mut gui_node = GuiNode::new(node.tag_name().name());
    gui_node.attributes = read_attributes(node);

    let text = node
        .children()
        .filter(Node::is_text)
        .filter_map(|n| n.text())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("");
    if !text.is_empty() {
        gui_node.text = Some(text);
    }

    gui_node.children = node
        .children()
        .filter(Node::is_element)
        .map(read_node)
        .collect();

    gui_node
}

fn read_attributes(node: Node) -> BTreeMap<String, String> {
    node.attributes()
        .map(|attr| (attr.name().to_owned(), attr.value().to_owned()))
        .collect()
}

fn attr(node: Node, name: &str) -> Option<String> {
    node.attribute(name).map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal_gui_document() {
        let xml = r##"
          <gui version="0.2" name="Smoke">
            <tokens>
              <color name="surface" value="#ffffff" />
              <token name="space.4" value="16" />
            </tokens>
            <fonts>
              <font family="Inter" source="google" weights="400 700" styles="normal" />
            </fonts>
            <styles>
              <text-style name="title" font="Inter" size="24" />
            </styles>
            <col w="390" p="24" gap="12" fill="$surface">
              <text value="Hello GUI" style="title" />
              <rect w="120" h="44" radius="8" fill="#0d99ff" />
            </col>
          </gui>
        "##;

        let parsed = parse_gui_xml(xml).expect("valid gui");

        assert_eq!(parsed.version, "0.2");
        assert_eq!(parsed.name.as_deref(), Some("Smoke"));
        assert_eq!(parsed.metadata.tokens["surface"], "#ffffff");
        assert_eq!(parsed.metadata.tokens["space.4"], "16");
        assert_eq!(parsed.metadata.fonts["Inter"].source, "google");
        assert_eq!(
            parsed.metadata.fonts["Inter"].weights.as_deref(),
            Some("400 700")
        );
        assert_eq!(parsed.metadata.styles["title"]["size"], "24");
        assert_eq!(parsed.root.tag, "col");
        assert_eq!(parsed.root.children.len(), 2);
        assert_eq!(parsed.root.children[0].tag, "text");
        assert_eq!(parsed.root.children[0].attributes["value"], "Hello GUI");
    }

    #[test]
    fn rejects_documents_without_a_layout_root() {
        let err = parse_gui_xml(r#"<gui version="0.2"><tokens /></gui>"#).unwrap_err();
        assert!(matches!(err, ParseError::MissingRootLayout));
    }

    #[test]
    fn normalizes_presence_attributes_before_xml_parse() {
        let xml = r#"
          <gui version="0.2" name="Presence">
            <frame w="320" h="240" clip>
              <col abs x="0" y="0" wrap />
            </frame>
          </gui>
        "#;

        let parsed = parse_gui_xml(xml).expect("presence attrs should parse");

        assert_eq!(parsed.root.attributes["clip"], "true");
        assert_eq!(parsed.root.children[0].attributes["abs"], "true");
        assert_eq!(parsed.root.children[0].attributes["wrap"], "true");
    }

    #[test]
    fn parses_fonts_by_family_and_source() {
        let xml = r#"
          <gui version="0.2" name="Fonts">
            <fonts>
              <font family="Roboto" source="google" category="sans-serif" weights="400 500 700" styles="normal italic" />
              <font family="SF Pro" source="system" weights="400 600" styles="normal" />
            </fonts>
            <col w="390" />
          </gui>
        "#;

        let parsed = parse_gui_xml(xml).expect("valid gui");

        assert_eq!(parsed.metadata.fonts["Roboto"].source, "google");
        assert_eq!(
            parsed.metadata.fonts["Roboto"].category.as_deref(),
            Some("sans-serif")
        );
        assert_eq!(
            parsed.metadata.fonts["Roboto"].weights.as_deref(),
            Some("400 500 700")
        );
        assert_eq!(parsed.metadata.fonts["SF Pro"].source, "system");
    }
}
