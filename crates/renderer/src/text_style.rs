//! Resolving the font properties of `<text>` and its `<segment>` children.
//!
//! Layout works on [`GuiNode`], painting on [`LayoutBox`], and both have to
//! reach identical conclusions about what a run of text looks like. They share
//! this module through [`TextSource`] rather than each carrying a copy of the
//! attribute lookups.

use crate::{GuiMetadata, GuiNode, LayoutBox};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A node that can carry text: either a document node or a laid-out box.
pub(crate) trait TextSource {
    fn tag(&self) -> &str;
    fn attributes(&self) -> &BTreeMap<String, String>;
    fn body_text(&self) -> Option<&str>;
    fn text_children(&self) -> &[Self]
    where
        Self: Sized;
}

impl TextSource for GuiNode {
    fn tag(&self) -> &str {
        &self.tag
    }

    fn attributes(&self) -> &BTreeMap<String, String> {
        &self.attributes
    }

    fn body_text(&self) -> Option<&str> {
        self.text.as_deref()
    }

    fn text_children(&self) -> &[Self] {
        &self.children
    }
}

impl TextSource for LayoutBox {
    fn tag(&self) -> &str {
        &self.tag
    }

    fn attributes(&self) -> &BTreeMap<String, String> {
        &self.attributes
    }

    fn body_text(&self) -> Option<&str> {
        self.text.as_deref()
    }

    fn text_children(&self) -> &[Self] {
        &self.children
    }
}

/// Which rule a `<text>` draws through its glyphs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DecorationLine {
    Underline,
    Strikethrough,
}

/// How that rule is stroked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DecorationStyle {
    #[default]
    Solid,
    Dashed,
    Dotted,
    Wavy,
    Double,
}

/// How a run's letters are cased before it is measured or drawn.
///
/// `small-caps` and `small-caps-forced` differ in reach, not in look: the
/// first leaves letters that were already capitals at full size and shrinks
/// only the ones it had to capitalise, the second shrinks every letter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TextCase {
    Upper,
    Lower,
    Capitalize,
    SmallCaps,
    SmallCapsForced,
}

/// The size a synthesised small capital is drawn at, as a share of the run's
/// own size.
///
/// A face with a real `smcp` feature has drawn small capitals whose stems are
/// thickened to match the full-size ones. Nothing here reads OpenType
/// features, so these are scaled capitals instead, which is the same
/// substitution a browser makes for a face without the feature. 0.8 is the
/// ratio browsers synthesise at.
const SMALL_CAPS_SCALE: f32 = 0.8;

/// A `<text>`'s decoration, resolved.
///
/// Neither this renderer nor kit drew one before: `decoration` is in the
/// spec's table of properties implemented by neither. So the reference for
/// what these should look like is CSS, which is where the spec's own value
/// names come from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TextDecoration {
    pub line: DecorationLine,
    pub style: DecorationStyle,
    /// `decoration-color`; the text's own colour when unset.
    pub color: Option<String>,
    /// `decoration-thickness` in px; derived from the font when unset.
    pub thickness: Option<f32>,
    /// `text-underline-offset` in px below the baseline. Underline only —
    /// a strikethrough is positioned from the font's x-height instead.
    pub offset: Option<f32>,
    /// `text-decoration-skip-ink`: break the rule where a glyph crosses it.
    pub skip_ink: bool,
}

/// The font properties a run of text is drawn with, after tokens, `text-style`
/// lookups, and inheritance from the parent `<text>` have been applied.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct TextStyle {
    pub font_family: Option<String>,
    pub font_weight: Option<String>,
    pub font_style: Option<String>,
    pub font_size: f32,
    /// `line-height` as declared. `None` is CSS `normal`: the face's own
    /// metric, which only a [`TextMeasurer`](crate::TextMeasurer) can supply,
    /// so it stays unresolved until layout or painting asks.
    pub line_height: Option<f32>,
    pub letter_spacing: f32,
    pub color: Option<String>,
    /// `font-stretch`, as a CSS keyword or a percentage.
    pub font_stretch: Option<String>,
    /// `font-optical-sizing`: `auto` or `none`.
    pub font_optical_sizing: Option<String>,
    /// `font-variation`, as the CSS `font-variation-settings` string it is —
    /// `"wght" 700, "slnt" -10`. Parsed by [`FontAxes`](crate::FontAxes),
    /// which is where the axis tags mean anything.
    pub font_variation: Option<String>,
    /// `font-smoothing`: `antialiased`, `subpixel-antialiased` or `none`.
    pub font_smoothing: Option<String>,
    pub word_spacing: f32,
    /// Pixels the run's baseline moves up, from `baseline-shift`.
    pub baseline_shift: f32,
    /// The rule drawn through this run, from `decoration` and its controls.
    pub decoration: Option<TextDecoration>,
    /// `text-case`, applied to the run's text before anything measures it.
    pub text_case: Option<TextCase>,
}

impl TextStyle {
    /// The line height to lay this run out at.
    ///
    /// A declared `line-height` wins; otherwise the measurer supplies the
    /// face's own. Layout and painting both come through here so they cannot
    /// resolve `normal` to two different numbers and size a box to a height
    /// they then draw at another.
    pub(crate) fn resolved_line_height(&self, measurer: &dyn crate::TextMeasurer) -> f32 {
        self.line_height.unwrap_or_else(|| {
            measurer.normal_line_height(
                self.font_family.as_deref(),
                self.font_weight.as_deref(),
                self.font_style.as_deref(),
                self.font_size,
            )
        })
    }
}

/// One styled run of a `<text>` node.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct TextRunStyle {
    pub value: String,
    pub style: TextStyle,
}

const DEFAULT_FONT_SIZE: f32 = 16.0;

/// Resolves the style of a `<text>` node itself.
pub(crate) fn resolve_text_style<S: TextSource>(node: &S, metadata: &GuiMetadata) -> TextStyle {
    let font_size = style_number(node, metadata, "font-size")
        .or_else(|| style_number(node, metadata, "size"))
        .unwrap_or(DEFAULT_FONT_SIZE);

    TextStyle {
        font_family: style_value(node, metadata, "font-family"),
        font_weight: style_value(node, metadata, "font-weight"),
        font_style: style_value(node, metadata, "font-style"),
        font_size,
        line_height: style_number(node, metadata, "line-height"),
        letter_spacing: style_number(node, metadata, "letter-spacing").unwrap_or(0.0),
        color: node
            .attributes()
            .get("fill")
            .or_else(|| node.attributes().get("color"))
            .map(|value| resolve_token(value, metadata)),
        font_stretch: style_value(node, metadata, "font-stretch"),
        font_optical_sizing: style_value(node, metadata, "font-optical-sizing"),
        font_variation: style_value(node, metadata, "font-variation"),
        font_smoothing: style_value(node, metadata, "font-smoothing"),
        word_spacing: style_number(node, metadata, "word-spacing").unwrap_or(0.0),
        baseline_shift: style_number(node, metadata, "baseline-shift").unwrap_or(0.0),
        decoration: resolve_decoration(node, metadata),
        text_case: resolve_text_case(node, metadata),
    }
}

/// Reads `text-case`.
fn resolve_text_case<S: TextSource>(node: &S, metadata: &GuiMetadata) -> Option<TextCase> {
    match style_value(node, metadata, "text-case")?.trim() {
        "uppercase" => Some(TextCase::Upper),
        "lowercase" => Some(TextCase::Lower),
        "capitalize" => Some(TextCase::Capitalize),
        "small-caps" => Some(TextCase::SmallCaps),
        "small-caps-forced" => Some(TextCase::SmallCapsForced),
        // `original` is the spec's way of spelling "leave it alone", and an
        // unknown value is not a licence to mangle the text either.
        _ => None,
    }
}

/// Reads `decoration` and the attributes that shape it.
///
/// Everything else is inert without `decoration` itself: a `decoration-color`
/// with no line to colour describes nothing, which is why they resolve
/// together rather than as six independent properties.
fn resolve_decoration<S: TextSource>(node: &S, metadata: &GuiMetadata) -> Option<TextDecoration> {
    let line = match style_value(node, metadata, "decoration")?.trim() {
        "underline" => DecorationLine::Underline,
        "strikethrough" => DecorationLine::Strikethrough,
        // `none` is how a run opts out of a decoration it would inherit.
        _ => return None,
    };

    Some(TextDecoration {
        line,
        style: match style_value(node, metadata, "decoration-style")
            .as_deref()
            .map(str::trim)
        {
            Some("dashed") => DecorationStyle::Dashed,
            Some("dotted") => DecorationStyle::Dotted,
            Some("wavy") => DecorationStyle::Wavy,
            Some("double") => DecorationStyle::Double,
            _ => DecorationStyle::Solid,
        },
        color: style_value(node, metadata, "decoration-color"),
        thickness: style_number(node, metadata, "decoration-thickness"),
        offset: style_number(node, metadata, "text-underline-offset"),
        // CSS defaults this to `auto`, which skips. A document that wants a
        // rule straight through its descenders has to say so.
        skip_ink: style_value(node, metadata, "text-decoration-skip-ink")
            .as_deref()
            .map(str::trim)
            != Some("false"),
    })
}

/// Splits a `<text>` node into the runs it should be drawn as.
///
/// A node without `<segment>` children is a single run carrying its own style.
/// Segments inherit every property they do not override, so
/// `<text font-size="14"><segment font-weight="700" /></text>` keeps size 14.
///
/// A node's own `value`/body text is emitted before its segments, which lets
/// `<text value="Total: "><segment value="$12" font-weight="700"/></text>` read
/// naturally.
pub(crate) fn resolve_text_runs<S: TextSource>(
    node: &S,
    metadata: &GuiMetadata,
) -> Vec<TextRunStyle> {
    let parent = resolve_text_style(node, metadata);
    let mut runs = Vec::new();

    let own_value = node
        .attributes()
        .get("value")
        .map(String::as_str)
        .or_else(|| node.body_text())
        .unwrap_or("");
    if !own_value.is_empty() {
        runs.push(TextRunStyle {
            value: own_value.to_owned(),
            style: parent.clone(),
        });
    }

    for child in node.text_children() {
        if child.tag() != "segment" {
            continue;
        }
        let value = child
            .attributes()
            .get("value")
            .map(String::as_str)
            .or_else(|| child.body_text())
            .unwrap_or("");
        if value.is_empty() {
            continue;
        }
        runs.push(TextRunStyle {
            value: value.to_owned(),
            style: inherit_style(child, metadata, &parent),
        });
    }

    if runs.is_empty() {
        runs.push(TextRunStyle {
            value: String::new(),
            style: parent,
        });
    }

    apply_text_case(runs)
}

/// Rewrites each run's text for its `text-case`.
///
/// This happens here, in the one function both layout and painting call, so
/// the two cannot measure one string and draw another — an uppercased run is
/// wider than what the document wrote, and a box sized to the original would
/// clip it.
fn apply_text_case(runs: Vec<TextRunStyle>) -> Vec<TextRunStyle> {
    runs.into_iter().flat_map(cased_run).collect()
}

/// One run, cased — as several runs when the casing needs two sizes.
fn cased_run(run: TextRunStyle) -> Vec<TextRunStyle> {
    let Some(case) = run.style.text_case else {
        return vec![run];
    };

    match case {
        TextCase::Upper => vec![TextRunStyle {
            value: run.value.to_uppercase(),
            ..run
        }],
        TextCase::Lower => vec![TextRunStyle {
            value: run.value.to_lowercase(),
            ..run
        }],
        TextCase::Capitalize => vec![TextRunStyle {
            value: capitalized(&run.value),
            ..run
        }],
        // Small capitals are drawn at two sizes, and a run carries one size,
        // so the text is split where the size changes. `-forced` shrinks
        // every letter; plain `small-caps` shrinks only what it capitalised,
        // which leaves an acronym standing at full height.
        TextCase::SmallCaps => small_caps_runs(&run, false),
        TextCase::SmallCapsForced => small_caps_runs(&run, true),
    }
}

/// Upper-cases the first letter of each word and lower-cases the rest.
fn capitalized(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut at_word_start = true;
    for character in value.chars() {
        if character.is_alphabetic() {
            if at_word_start {
                out.extend(character.to_uppercase());
            } else {
                out.extend(character.to_lowercase());
            }
            at_word_start = false;
        } else {
            out.push(character);
            // An apostrophe is inside a word — "don't" is not "Don'T" — so
            // only whitespace and punctuation that separates words restarts.
            at_word_start = !matches!(character, '\'' | '\u{2019}');
        }
    }
    out
}

/// Splits a run into full-size and small-capital pieces.
///
/// Adjacent characters that take the same size stay in one run, so a word
/// that is entirely lower case still measures as a single string rather than
/// as one run per letter, which would lose the kerning between them.
fn small_caps_runs(run: &TextRunStyle, forced: bool) -> Vec<TextRunStyle> {
    let mut out: Vec<TextRunStyle> = Vec::new();

    for character in run.value.chars() {
        // A character with no upper case of its own — a digit, a space, a
        // comma — is left at full size, as a browser does.
        let shrinks = forced && character.is_alphabetic() || !forced && character.is_lowercase();
        let text: String = if shrinks {
            character.to_uppercase().collect()
        } else {
            character.to_string()
        };

        match out.last_mut() {
            Some(last) if (last.style.font_size < run.style.font_size) == shrinks => {
                last.value.push_str(&text);
            }
            _ => {
                let mut style = run.style.clone();
                if shrinks {
                    style.font_size = run.style.font_size * SMALL_CAPS_SCALE;
                    // The pieces sit on one baseline, so the shrunk run must
                    // not also pull the line height down with it.
                    style.line_height = style.line_height.or(Some(run.style.font_size));
                }
                out.push(TextRunStyle { value: text, style });
            }
        }
    }

    if out.is_empty() {
        return vec![run.clone()];
    }
    out
}

fn inherit_style<S: TextSource>(node: &S, metadata: &GuiMetadata, parent: &TextStyle) -> TextStyle {
    let font_size = style_number(node, metadata, "font-size")
        .or_else(|| style_number(node, metadata, "size"))
        .unwrap_or(parent.font_size);

    // A segment that changes size but not line height gets a line height
    // derived from its own size, so a larger run is not clipped. `None` means
    // the face decides, which already scales with the size.
    let inherited_line_height = if font_size == parent.font_size {
        parent.line_height
    } else {
        None
    };
    let line_height = style_number(node, metadata, "line-height").or(inherited_line_height);

    TextStyle {
        font_family: style_value(node, metadata, "font-family")
            .or_else(|| parent.font_family.clone()),
        font_weight: style_value(node, metadata, "font-weight")
            .or_else(|| parent.font_weight.clone()),
        font_style: style_value(node, metadata, "font-style").or_else(|| parent.font_style.clone()),
        font_size,
        line_height,
        letter_spacing: style_number(node, metadata, "letter-spacing")
            .unwrap_or(parent.letter_spacing),
        color: node
            .attributes()
            .get("fill")
            .or_else(|| node.attributes().get("color"))
            .map(|value| resolve_token(value, metadata))
            .or_else(|| parent.color.clone()),
        font_stretch: style_value(node, metadata, "font-stretch")
            .or_else(|| parent.font_stretch.clone()),
        font_optical_sizing: style_value(node, metadata, "font-optical-sizing")
            .or_else(|| parent.font_optical_sizing.clone()),
        font_variation: style_value(node, metadata, "font-variation")
            .or_else(|| parent.font_variation.clone()),
        font_smoothing: style_value(node, metadata, "font-smoothing")
            .or_else(|| parent.font_smoothing.clone()),
        word_spacing: style_number(node, metadata, "word-spacing").unwrap_or(parent.word_spacing),
        // A shift is a property of the run that declares it, not something a
        // nested run should inherit and double.
        baseline_shift: style_number(node, metadata, "baseline-shift").unwrap_or(0.0),
        // A rule runs under the whole `<text>`, segments included, unless a
        // segment names its own `decoration` — including `none` to opt out.
        decoration: match node.attributes().get("decoration") {
            Some(_) => resolve_decoration(node, metadata),
            None => parent.decoration.clone(),
        },
        // Casing applies to the whole `<text>` unless a segment names its own.
        text_case: resolve_text_case(node, metadata).or(parent.text_case),
    }
}

fn style_value<S: TextSource>(node: &S, metadata: &GuiMetadata, name: &str) -> Option<String> {
    node.attributes()
        .get(name)
        .map(|value| resolve_token(value, metadata))
        .or_else(|| named_style(node, metadata)?.get(name).cloned())
}

fn style_number<S: TextSource>(node: &S, metadata: &GuiMetadata, name: &str) -> Option<f32> {
    node.attributes()
        .get(name)
        .map(|value| resolve_token(value, metadata))
        .and_then(|value| parse_number(&value))
        .or_else(|| {
            named_style(node, metadata)?
                .get(name)
                .and_then(|value| parse_number(value))
        })
}

fn named_style<'a, S: TextSource>(
    node: &S,
    metadata: &'a GuiMetadata,
) -> Option<&'a BTreeMap<String, String>> {
    let name = node
        .attributes()
        .get("text-style")
        .or_else(|| node.attributes().get("style"))?;
    metadata.styles.get(name)
}

/// Substitutes `$name` token references.
///
/// Every whitespace-separated part is resolved independently, because compound
/// values carry tokens inside them: `border="1 $rule"`, `p="$sp-2 $sp-4"`.
pub(crate) fn resolve_token(value: &str, metadata: &GuiMetadata) -> String {
    if !value.contains('$') {
        return value.to_owned();
    }

    value
        .split_whitespace()
        .map(|part| match part.strip_prefix('$') {
            Some(name) => metadata
                .tokens
                .get(name)
                .cloned()
                .unwrap_or_else(|| part.to_owned()),
            None => part.to_owned(),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn parse_number(value: &str) -> Option<f32> {
    if matches!(value, "fill" | "hug" | "auto") {
        return None;
    }
    value
        .trim()
        .trim_end_matches("px")
        .trim_end_matches('%')
        .parse::<f32>()
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse_gui_xml;

    fn text_node(xml: &str) -> (GuiNode, GuiMetadata) {
        let document = parse_gui_xml(xml).expect("valid gui");
        let node = document.root.children[0].clone();
        (node, document.metadata)
    }

    fn cased(case: &str, value: &str) -> Vec<TextRunStyle> {
        let xml = format!(
            r##"
            <gui version="0.2">
              <col>
                <text value="{value}" font-size="10" text-case="{case}" />
              </col>
            </gui>
            "##
        );
        let (node, metadata) = text_node(&xml);
        resolve_text_runs(&node, &metadata)
    }

    #[test]
    fn text_case_rewrites_the_run_before_anything_measures_it() {
        assert_eq!(cased("uppercase", "Hello there")[0].value, "HELLO THERE");
        assert_eq!(cased("lowercase", "Hello There")[0].value, "hello there");
        assert_eq!(cased("capitalize", "hello there")[0].value, "Hello There");
    }

    #[test]
    fn capitalize_lowers_the_rest_of_each_word() {
        // CSS `capitalize` only touches the first letter, but the spec value
        // here is a case *conversion*, as in a design tool: "iPhone" titled is
        // "Iphone". An apostrophe stays inside its word.
        assert_eq!(cased("capitalize", "hELLO wORLD")[0].value, "Hello World");
        assert_eq!(cased("capitalize", "don't stop")[0].value, "Don't Stop");
    }

    #[test]
    fn text_case_leaves_a_value_it_does_not_recognise_alone() {
        assert_eq!(cased("original", "Hello")[0].value, "Hello");
        assert_eq!(cased("sideways", "Hello")[0].value, "Hello");
    }

    #[test]
    fn small_caps_shrinks_only_the_letters_it_capitalised() {
        // "GUI kit": the acronym was already capital and stays at full size,
        // the lower-case word becomes capitals at the smaller one.
        let runs = cased("small-caps", "GUI kit");
        let text: String = runs.iter().map(|run| run.value.as_str()).collect();
        assert_eq!(text, "GUI KIT");

        assert_eq!(runs[0].value, "GUI ", "the acronym and the space");
        assert_eq!(runs[0].style.font_size, 10.0);
        assert_eq!(runs[1].value, "KIT");
        assert_eq!(runs[1].style.font_size, 10.0 * SMALL_CAPS_SCALE);
    }

    #[test]
    fn small_caps_forced_shrinks_every_letter() {
        let runs = cased("small-caps-forced", "GUI kit");
        let text: String = runs.iter().map(|run| run.value.as_str()).collect();
        assert_eq!(text, "GUI KIT");

        // The space is not a letter, so it stays at full size and splits the
        // two shrunk pieces.
        let letters: Vec<f32> = runs
            .iter()
            .filter(|run| run.value.trim() != "")
            .map(|run| run.style.font_size)
            .collect();
        assert!(
            letters.iter().all(|size| *size == 10.0 * SMALL_CAPS_SCALE),
            "every letter shrinks: {letters:?}"
        );
    }

    #[test]
    fn a_segment_inherits_text_case_and_can_override_it() {
        let (node, metadata) = text_node(
            r##"
            <gui version="0.2">
              <col>
                <text value="one " text-case="uppercase">
                  <segment value="two " />
                  <segment value="three" text-case="lowercase" />
                </text>
              </col>
            </gui>
            "##,
        );

        let runs = resolve_text_runs(&node, &metadata);
        assert_eq!(runs[0].value, "ONE ");
        assert_eq!(runs[1].value, "TWO ", "inherited from the text node");
        assert_eq!(runs[2].value, "three", "its own wins");
    }

    #[test]
    fn a_plain_text_node_is_one_run() {
        let (node, metadata) = text_node(
            r##"
            <gui version="0.2">
              <col>
                <text value="Hello" font-size="14" />
              </col>
            </gui>
            "##,
        );

        let runs = resolve_text_runs(&node, &metadata);
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].value, "Hello");
        assert_eq!(runs[0].style.font_size, 14.0);
    }

    #[test]
    fn segments_inherit_what_they_do_not_override() {
        let (node, metadata) = text_node(
            r##"
            <gui version="0.2">
              <col>
                <text font-size="14" font-family="Inter" fill="#111111">
                  <segment value="Total: " />
                  <segment value="$12" font-weight="700" fill="#ff0000" />
                </text>
              </col>
            </gui>
            "##,
        );

        let runs = resolve_text_runs(&node, &metadata);
        assert_eq!(runs.len(), 2);

        assert_eq!(runs[0].value, "Total: ");
        assert_eq!(runs[0].style.font_weight, None);
        assert_eq!(runs[0].style.color.as_deref(), Some("#111111"));

        assert_eq!(runs[1].value, "$12");
        assert_eq!(runs[1].style.font_weight.as_deref(), Some("700"));
        assert_eq!(runs[1].style.color.as_deref(), Some("#ff0000"));
        // Inherited, not defaulted.
        assert_eq!(runs[1].style.font_family.as_deref(), Some("Inter"));
        assert_eq!(runs[1].style.font_size, 14.0);
    }

    #[test]
    fn a_text_value_precedes_its_segments() {
        let (node, metadata) = text_node(
            r##"
            <gui version="0.2">
              <col>
                <text value="Total: " font-size="14">
                  <segment value="$12" font-weight="700" />
                </text>
              </col>
            </gui>
            "##,
        );

        let runs = resolve_text_runs(&node, &metadata);
        assert_eq!(
            runs.iter()
                .map(|run| run.value.as_str())
                .collect::<Vec<_>>(),
            vec!["Total: ", "$12"]
        );
    }

    #[test]
    fn a_resized_segment_gets_its_own_line_height() {
        let (node, metadata) = text_node(
            r##"
            <gui version="0.2">
              <col>
                <text font-size="10" line-height="12">
                  <segment value="small" />
                  <segment value="big" font-size="30" />
                </text>
              </col>
            </gui>
            "##,
        );

        let runs = resolve_text_runs(&node, &metadata);

        // The declared 12 belongs to the run that declared it. The resized
        // segment does not inherit it — it falls back to `normal`, which
        // scales with its own size instead of being clipped by its parent's.
        assert_eq!(runs[0].style.line_height, Some(12.0));
        assert_eq!(runs[1].style.line_height, None);

        let measurer = crate::ApproxTextMeasurer;
        assert_eq!(runs[0].style.resolved_line_height(&measurer), 12.0);
        assert_eq!(
            runs[1].style.resolved_line_height(&measurer),
            36.0,
            "the 30px segment reserves room for 30px of text"
        );
    }

    #[test]
    fn segments_resolve_tokens_and_named_styles() {
        let (node, metadata) = text_node(
            r##"
            <gui version="0.2">
              <tokens><color name="danger" value="#ff0000" /></tokens>
              <styles><text-style name="strong" font-weight="700" /></styles>
              <col>
                <text font-size="14">
                  <segment value="oops" text-style="strong" fill="$danger" />
                </text>
              </col>
            </gui>
            "##,
        );

        let runs = resolve_text_runs(&node, &metadata);
        assert_eq!(runs[0].style.font_weight.as_deref(), Some("700"));
        assert_eq!(runs[0].style.color.as_deref(), Some("#ff0000"));
    }
}
