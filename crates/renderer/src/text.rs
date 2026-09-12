//! Shared text line breaking.
//!
//! Layout and painting must break a string into the same lines. When they
//! disagree a box is sized for one line count and drawn with another, so both
//! sides call into this module rather than carrying their own wrapper.
//!
//! Every `measure` callback here is expected to already include letter spacing.

/// A stretch of text drawn with one style, identified by its index into
/// whatever style list the caller is working from.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Run {
    pub text: String,
    pub style: usize,
}

/// A wrapped line: one or more runs sharing a baseline.
pub(crate) type Line = Vec<Run>;

/// How a word that will not fit may be broken.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum WordBreak {
    /// Break between words, and after the punctuation in `BREAK_AFTER`. A word
    /// too long for the line overflows rather than splitting.
    #[default]
    Normal,
    /// Break between any two characters.
    BreakAll,
    /// Never break inside a word, not even at punctuation.
    KeepAll,
    /// Like `Normal`, but a word with a line to itself and still too long is
    /// split rather than allowed to overflow.
    BreakWord,
}

/// The line-breaking rules a `<text>` node asks for.
#[derive(Debug, Clone, Copy)]
pub(crate) struct WrapOptions {
    /// Whether the text wraps at all. `white-space: nowrap` and
    /// `text-wrap: nowrap` both turn this off.
    pub wrap: bool,
    pub word_break: WordBreak,
    /// First-line indent in pixels, from `paragraph-indent`. It takes room
    /// from the first line only, so the rest of the block is unaffected.
    pub indent: f32,
}

impl Default for WrapOptions {
    fn default() -> Self {
        Self {
            wrap: true,
            word_break: WordBreak::Normal,
            indent: 0.0,
        }
    }
}

impl WrapOptions {
    /// Reads the options out of a `<text>` node's attributes.
    pub fn new(
        white_space: Option<&str>,
        text_wrap: Option<&str>,
        word_break: Option<&str>,
        indent: f32,
    ) -> Self {
        Self {
            // Two attributes can each forbid wrapping, and either one is
            // enough; `pre` is the white-space value that also does.
            wrap: !matches!(white_space, Some("nowrap") | Some("pre"))
                && text_wrap != Some("nowrap"),
            word_break: match word_break {
                Some("break-all") => WordBreak::BreakAll,
                Some("keep-all") => WordBreak::KeepAll,
                Some("break-word") => WordBreak::BreakWord,
                _ => WordBreak::Normal,
            },
            indent,
        }
    }
}

/// Breaks styled runs into rendered lines, wrapping across style boundaries.
///
/// `measure` is given a string and the style index it belongs to. Text flows
/// continuously between runs, so a bold word mid-sentence does not force a
/// break, and a break can fall inside a run.
pub(crate) fn wrap_runs(
    runs: &[Run],
    max_width: Option<f32>,
    measure: &dyn Fn(&str, usize) -> f32,
    options: WrapOptions,
) -> Vec<Line> {
    // A hard newline still opens a line when wrapping is off: `nowrap` stops
    // the renderer choosing breaks, not the document declaring them.
    let max_width = options.wrap.then_some(max_width).flatten();
    // Flatten to chunks so a hard break inside one run and a break between two
    // runs are handled by the same loop.
    struct Chunk<'a> {
        text: &'a str,
        style: usize,
        hard_break_before: bool,
    }

    let mut chunks = Vec::new();
    for run in runs {
        for (index, segment) in run.text.split('\n').enumerate() {
            chunks.push(Chunk {
                text: segment,
                style: run.style,
                hard_break_before: index > 0,
            });
        }
    }

    let mut lines: Vec<Line> = Vec::new();
    let mut current: Line = Vec::new();
    // Whitespace at a run boundary belongs between the runs, not inside either,
    // so `<segment value="Total: "/><segment value="$12"/>` keeps its space.
    let mut pending_space = false;

    for chunk in chunks {
        if chunk.hard_break_before {
            lines.push(std::mem::take(&mut current));
            pending_space = false;
        }

        let starts_with_space = chunk.text.starts_with(char::is_whitespace);
        for (index, piece) in break_pieces(chunk.text, options.word_break)
            .into_iter()
            .enumerate()
        {
            let separated = if index == 0 {
                starts_with_space || pending_space
            } else {
                piece.leading_space
            };

            let mut spaced = !current.is_empty() && separated;

            // The indent takes room from the first line, so that line fills
            // less before it has to break.
            let line_width = max_width.map(|max_width| {
                if lines.is_empty() {
                    (max_width - options.indent).max(1.0)
                } else {
                    max_width.max(1.0)
                }
            });

            // The line is measured as it would be drawn, with the piece joined
            // on, rather than as a sum of pieces measured apart. Shaping kerns
            // across the joins, so the sum runs wider than the line — and
            // layout sizes a hugging box to the line. Judged by the sum, the
            // last piece would not fit the box made for it.
            let overflows = line_width.is_some_and(|line_width| {
                !current.is_empty()
                    && joined_width(&current, piece.text, spaced, chunk.style, measure)
                        > line_width + 0.5
            });

            if overflows {
                // The separating space is dropped, as it would be at the start
                // of a line in HTML. A piece broken off mid-word never had one.
                lines.push(std::mem::take(&mut current));
                spaced = false;
            }

            // `break-word` splits a word that will not fit on a line of its
            // own — whether it reached one by overflowing off the line before,
            // or by being the first thing on this one.
            let must_split = options.word_break == WordBreak::BreakWord
                && current.is_empty()
                && line_width
                    .is_some_and(|line_width| measure(piece.text, chunk.style) > line_width);

            if must_split {
                split_across_lines(
                    &mut lines,
                    &mut current,
                    piece.text,
                    chunk.style,
                    line_width.unwrap_or(f32::MAX),
                    measure,
                );
            } else {
                if spaced {
                    push_piece(&mut current, " ", chunk.style);
                }
                push_piece(&mut current, piece.text, chunk.style);
            }
            pending_space = false;
        }

        if chunk.text.ends_with(char::is_whitespace) {
            pending_space = true;
        }
    }

    lines.push(current);

    // Match `str::lines`: a trailing newline does not open another line.
    if lines.len() > 1 && lines.last().is_some_and(Vec::is_empty) {
        lines.pop();
    }

    lines
}

/// Fills line after line with as much of an over-long word as fits.
///
/// The tail is left on `current` rather than pushed, so whatever follows the
/// word can still share its last line.
fn split_across_lines(
    lines: &mut Vec<Line>,
    current: &mut Line,
    text: &str,
    style: usize,
    line_width: f32,
    measure: &dyn Fn(&str, usize) -> f32,
) {
    let mut start = 0;
    let mut width = 0.0_f32;

    for (offset, ch) in text.char_indices() {
        let char_width = measure(&text[offset..offset + ch.len_utf8()], style);
        // One character per line at minimum, or a character wider than the
        // line would loop forever without ever placing anything.
        if offset > start && width + char_width > line_width + 0.5 {
            push_piece(current, &text[start..offset], style);
            lines.push(std::mem::take(current));
            start = offset;
            width = 0.0;
        }
        width += char_width;
    }

    if start < text.len() {
        push_piece(current, &text[start..], style);
    }
}

/// Width of `line` once `text` is appended to it, measured the way the line
/// will be drawn: the joined run as one string, so kerning across the join
/// counts.
fn joined_width(
    line: &Line,
    text: &str,
    spaced: bool,
    style: usize,
    measure: &dyn Fn(&str, usize) -> f32,
) -> f32 {
    let separator = if spaced { " " } else { "" };
    match line.split_last() {
        // `push_piece` merges into a run of the same style, so that is the
        // string painting will shape.
        Some((last, rest)) if last.style == style => {
            line_width(rest, measure) + measure(&format!("{}{separator}{text}", last.text), style)
        }
        _ => line_width(line, measure) + measure(&format!("{separator}{text}"), style),
    }
}

/// Appends text to a line, merging into the previous run when the style matches
/// so a line does not fragment into one run per word.
fn push_piece(line: &mut Line, text: &str, style: usize) {
    match line.last_mut() {
        Some(last) if last.style == style => last.text.push_str(text),
        _ => line.push(Run {
            text: text.to_owned(),
            style,
        }),
    }
}

/// One run of text that must stay together, and whether a space precedes it.
struct Piece<'a> {
    text: &'a str,
    leading_space: bool,
}

/// Characters a line may break *after*, keeping the character on the first
/// line. This is the common subset of UAX #14 that shows up in interface copy:
/// `Order AR-4827` wraps as `AR-` / `4827` the way a browser does.
const BREAK_AFTER: [char; 4] = ['-', '\u{2013}', '\u{2014}', '/'];

/// Splits a line into the runs that greedy wrapping is allowed to separate.
fn break_pieces(line: &str, word_break: WordBreak) -> Vec<Piece<'_>> {
    let mut pieces = Vec::new();

    for (index, word) in line.split_whitespace().enumerate() {
        // `break-all` lets a break fall between any two characters, so every
        // character is its own piece and the wrapper does the rest.
        if word_break == WordBreak::BreakAll {
            for (offset, ch) in word.char_indices() {
                pieces.push(Piece {
                    text: &word[offset..offset + ch.len_utf8()],
                    leading_space: index > 0 && offset == 0,
                });
            }
            continue;
        }

        // `keep-all` holds a word together even at punctuation that would
        // otherwise be a break opportunity.
        if word_break == WordBreak::KeepAll {
            pieces.push(Piece {
                text: word,
                leading_space: index > 0,
            });
            continue;
        }

        let mut start = 0;
        // A break character only ends a run when something breakable precedes
        // it, so a leading `-` (a minus sign, say) stays with its number.
        let mut has_content = false;

        for (offset, ch) in word.char_indices() {
            if BREAK_AFTER.contains(&ch) {
                if has_content {
                    let end = offset + ch.len_utf8();
                    pieces.push(Piece {
                        text: &word[start..end],
                        leading_space: index > 0 && start == 0,
                    });
                    start = end;
                    has_content = false;
                }
            } else {
                has_content = true;
            }
        }

        if start < word.len() {
            pieces.push(Piece {
                text: &word[start..],
                leading_space: index > 0 && start == 0,
            });
        }
    }

    pieces
}

/// Widest rendered line, used as the intrinsic width of a text box.
pub(crate) fn max_line_width(lines: &[Line], measure: &dyn Fn(&str, usize) -> f32) -> f32 {
    lines
        .iter()
        .map(|line| line_width(line, measure))
        .fold(0.0_f32, f32::max)
}

/// Rendered width of one line, summed across its runs.
pub(crate) fn line_width(line: &[Run], measure: &dyn Fn(&str, usize) -> f32) -> f32 {
    line.iter()
        .map(|run| measure(&run.text, run.style))
        .sum::<f32>()
}

/// Clamps `lines` to `max_lines` and, when `truncate` is set, ellipsizes the
/// last surviving line so it fits `max_width`.
///
/// Clamping never changes the line count beyond `max_lines`, so layout can
/// predict the painted height with `lines.len().min(max_lines)`.
pub(crate) fn apply_line_limit_and_ellipsis(
    lines: &mut Vec<Line>,
    max_lines: Option<usize>,
    truncate: bool,
    max_width: f32,
    measure: &dyn Fn(&str, usize) -> f32,
) {
    let limit = max_lines.unwrap_or(usize::MAX).max(1);
    let overflowed = lines.len() > limit;
    if overflowed {
        lines.truncate(limit);
    }

    // Only the last line can carry an ellipsis. When lines were dropped it is
    // always added, even though the surviving line fits, because the reader
    // needs to know the text continues.
    if truncate || overflowed {
        if let Some(last_line) = lines.last_mut() {
            ellipsize_line(last_line, max_width, measure, overflowed);
        }
    }
}

/// Trims a line from the right until it fits, then appends an ellipsis in the
/// style of whichever run survives at the cut.
fn ellipsize_line(
    line: &mut Line,
    max_width: f32,
    measure: &dyn Fn(&str, usize) -> f32,
    force: bool,
) {
    if !force && line_width(line, measure) <= max_width {
        return;
    }

    let ellipsis = "...";
    let style = line.last().map_or(0, |run| run.style);
    if measure(ellipsis, style) > max_width {
        line.clear();
        return;
    }

    // Drop whole runs from the end until the ellipsis could fit after them.
    while let Some(last) = line.last() {
        let without_last: f32 = line_width(line, measure) - measure(&last.text, last.style);
        if without_last + measure(ellipsis, last.style) <= max_width {
            break;
        }
        line.pop();
    }

    let Some(last) = line.pop() else {
        line.push(Run {
            text: ellipsis.to_owned(),
            style,
        });
        return;
    };
    // Measured after the pop: this is the width the trimmed run starts from.
    let head_width = line_width(line, measure);

    let mut fitted = String::new();
    for ch in last.text.chars() {
        let candidate = format!("{fitted}{ch}{ellipsis}");
        if head_width + measure(&candidate, last.style) > max_width {
            break;
        }
        fitted.push(ch);
    }

    line.push(Run {
        text: format!("{fitted}{ellipsis}"),
        style: last.style,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One unit of width per character keeps the expectations readable.
    fn measure_run(value: &str, _style: usize) -> f32 {
        value.chars().count() as f32
    }

    fn lines_of(lines: &[Line]) -> Vec<String> {
        lines
            .iter()
            .map(|line| line.iter().map(|run| run.text.as_str()).collect())
            .collect()
    }

    /// Wraps a single unstyled string, the shape most of these cases need.
    fn wrap_lines(value: &str, max_width: Option<f32>) -> Vec<String> {
        lines_of(&wrap_runs(
            &plain(value),
            max_width,
            &measure_run,
            WrapOptions::default(),
        ))
    }

    fn plain(text: &str) -> Vec<Run> {
        vec![Run {
            text: text.to_owned(),
            style: 0,
        }]
    }

    fn wrapped_with(text: &str, max_width: f32, options: WrapOptions) -> Vec<String> {
        lines_of(&wrap_runs(
            &plain(text),
            Some(max_width),
            &measure_run,
            options,
        ))
    }

    #[test]
    fn nowrap_keeps_a_line_together_however_long_it_is() {
        let options = WrapOptions {
            wrap: false,
            ..WrapOptions::default()
        };
        assert_eq!(
            wrapped_with("one two three four", 5.0, options),
            vec!["one two three four"]
        );
    }

    #[test]
    fn nowrap_still_honours_a_declared_newline() {
        // Turning wrapping off stops the renderer choosing breaks; it does not
        // overrule a break the document wrote.
        let options = WrapOptions {
            wrap: false,
            ..WrapOptions::default()
        };
        assert_eq!(
            wrapped_with("one two\nthree", 5.0, options),
            vec!["one two", "three"]
        );
    }

    #[test]
    fn white_space_and_text_wrap_can_each_turn_wrapping_off() {
        assert!(!WrapOptions::new(Some("nowrap"), None, None, 0.0).wrap);
        assert!(!WrapOptions::new(Some("pre"), None, None, 0.0).wrap);
        assert!(!WrapOptions::new(None, Some("nowrap"), None, 0.0).wrap);
        assert!(WrapOptions::new(Some("pre-wrap"), Some("balance"), None, 0.0).wrap);
        assert!(WrapOptions::new(None, None, None, 0.0).wrap);
    }

    #[test]
    fn break_all_breaks_between_any_two_characters() {
        let options = WrapOptions {
            word_break: WordBreak::BreakAll,
            ..WrapOptions::default()
        };
        assert_eq!(
            wrapped_with("abcdefgh", 3.0, options),
            vec!["abc", "def", "gh"]
        );
    }

    #[test]
    fn keep_all_holds_a_word_together_at_punctuation() {
        let hyphenated = "AR-4827";
        assert_eq!(
            wrapped_with(hyphenated, 4.0, WrapOptions::default()),
            vec!["AR-", "4827"],
            "normal breaks after the hyphen"
        );

        let options = WrapOptions {
            word_break: WordBreak::KeepAll,
            ..WrapOptions::default()
        };
        assert_eq!(
            wrapped_with(hyphenated, 4.0, options),
            vec!["AR-4827"],
            "keep-all does not"
        );
    }

    #[test]
    fn break_word_splits_only_a_word_that_will_not_fit_alone() {
        let options = WrapOptions {
            word_break: WordBreak::BreakWord,
            ..WrapOptions::default()
        };

        assert_eq!(
            wrapped_with("hi enormouslylong", 6.0, options),
            vec!["hi", "enormo", "uslylo", "ng"],
            "the short word is left whole and the long one is split"
        );
    }

    #[test]
    fn break_word_splits_a_long_word_that_is_alone_on_the_line() {
        // The first thing on a line never "overflows" off it — it is already
        // there. A fixture caught this: a paragraph that is one long word got
        // no break at all.
        let options = WrapOptions {
            word_break: WordBreak::BreakWord,
            ..WrapOptions::default()
        };

        assert_eq!(
            wrapped_with("enormouslylongword", 6.0, options),
            vec!["enormo", "uslylo", "ngword"]
        );
    }

    #[test]
    fn a_word_that_fits_is_never_split_by_break_word() {
        let options = WrapOptions {
            word_break: WordBreak::BreakWord,
            ..WrapOptions::default()
        };
        assert_eq!(
            wrapped_with("aaa bbb ccc", 7.0, options),
            vec!["aaa bbb", "ccc"]
        );
    }

    #[test]
    fn an_indent_takes_room_from_the_first_line_only() {
        let options = WrapOptions {
            indent: 4.0,
            ..WrapOptions::default()
        };
        // 10 wide, less a 4 indent, leaves 6 for the first line.
        assert_eq!(
            wrapped_with("aaa bbb ccc", 10.0, options),
            vec!["aaa", "bbb ccc"]
        );
    }

    #[test]
    fn wraps_greedily_to_width() {
        let lines = wrap_lines("aaa bbb ccc", Some(7.0));
        assert_eq!(lines, vec!["aaa bbb", "ccc"]);
    }

    #[test]
    fn hard_breaks_split_lines_even_without_a_width() {
        let lines = wrap_lines("first\nsecond", None);
        assert_eq!(lines, vec!["first", "second"]);
    }

    #[test]
    fn hard_breaks_and_wrapping_combine() {
        let lines = wrap_lines("aaa bbb\nccc ddd", Some(3.0));
        assert_eq!(lines, vec!["aaa", "bbb", "ccc", "ddd"]);
    }

    #[test]
    fn breaks_after_a_hyphen_inside_a_word() {
        // "Order AR-4827" at this width: "Order AR-" fits, "4827" does not.
        let lines = wrap_lines("Order AR-4827", Some(9.0));
        assert_eq!(lines, vec!["Order AR-", "4827"]);
    }

    #[test]
    fn a_hyphen_break_does_not_reintroduce_a_space() {
        let lines = wrap_lines("AR-4827", Some(3.0));
        assert_eq!(lines, vec!["AR-", "4827"]);
    }

    #[test]
    fn a_leading_hyphen_stays_with_its_word() {
        // A minus sign is not a break opportunity.
        let lines = wrap_lines("total -31.50", Some(6.0));
        assert_eq!(lines, vec!["total", "-31.50"]);
    }

    #[test]
    fn hyphenated_words_still_join_when_there_is_room() {
        let lines = wrap_lines("Order AR-4827", Some(40.0));
        assert_eq!(lines, vec!["Order AR-4827"]);
    }

    #[test]
    fn blank_value_is_one_empty_line() {
        assert_eq!(wrap_lines("", Some(10.0)), vec![String::new()]);
    }

    #[test]
    fn a_word_longer_than_the_width_keeps_its_own_line() {
        let lines = wrap_lines("tiny enormouswordhere", Some(6.0));
        assert_eq!(lines, vec!["tiny", "enormouswordhere"]);
    }

    #[test]
    fn line_limit_ellipsizes_the_last_surviving_line() {
        let mut lines = vec![plain("first"), plain("second"), plain("third")];
        apply_line_limit_and_ellipsis(&mut lines, Some(2), true, 5.0, &measure_run);

        assert_eq!(lines_of(&lines), vec!["first", "se..."]);
    }

    #[test]
    fn overflowing_lines_are_ellipsized_even_without_truncate() {
        let mut lines = vec![plain("first"), plain("second")];
        apply_line_limit_and_ellipsis(&mut lines, Some(1), false, 4.0, &measure_run);

        assert_eq!(lines_of(&lines), vec!["f..."]);
    }

    #[test]
    fn a_clamped_line_is_ellipsized_even_when_it_fits() {
        // Lines were dropped, so the surviving line has to show that the text
        // continues even though it fits the box on its own.
        let mut lines = vec![plain("first"), plain("fits"), plain("dropped")];
        apply_line_limit_and_ellipsis(&mut lines, Some(2), true, 10.0, &measure_run);

        assert_eq!(lines_of(&lines), vec!["first", "fits..."]);
    }

    #[test]
    fn an_ellipsis_can_land_across_styled_runs() {
        let mut lines = vec![vec![
            Run {
                text: "label ".to_owned(),
                style: 0,
            },
            Run {
                text: "and a long tail".to_owned(),
                style: 1,
            },
        ]];
        apply_line_limit_and_ellipsis(&mut lines, Some(1), true, 12.0, &measure_run);

        assert_eq!(lines_of(&lines), vec!["label and..."]);
        // The ellipsis keeps the style of the run it was cut from.
        assert_eq!(lines[0].last().unwrap().style, 1);
    }

    #[test]
    fn text_that_already_fits_is_left_alone() {
        let mut lines = vec![plain("fits")];
        apply_line_limit_and_ellipsis(&mut lines, Some(1), true, 10.0, &measure_run);

        assert_eq!(lines_of(&lines), vec!["fits"]);
    }

    /// A shaped measure: ten units a character, less a little for every
    /// neighbouring pair, the way kerning tightens a string. A whole line is
    /// then narrower than its words measured apart and added up.
    fn measure_kerned(value: &str, _style: usize) -> f32 {
        let chars = value.chars().count() as f32;
        chars * 10.0 - (chars - 1.0).max(0.0) * 0.3
    }

    #[test]
    fn text_wrapped_at_its_own_width_stays_on_one_line() {
        // Layout sizes a hugging box to the unwrapped line, and painting then
        // wraps into that box. If the two disagree by more than the slack, the
        // last piece moves to a line the box has no room for and is never
        // drawn: "Deploy feature/cache-headers" painted as "Deploy
        // feature/cache-".
        for text in [
            "Deploy feature/cache-headers",
            "Atlas API / feature/cache-headers",
            "TUESDAY, 27 AUGUST",
            "RECENT DESTINATIONS",
        ] {
            let runs = plain(text);
            let options = WrapOptions::default();
            let unwrapped = wrap_runs(&runs, None, &measure_kerned, options);
            let width = max_line_width(&unwrapped, &measure_kerned);

            let wrapped = wrap_runs(&runs, Some(width), &measure_kerned, options);
            assert_eq!(lines_of(&wrapped), vec![text], "at {width}");
        }
    }

    #[test]
    fn styled_runs_wrapped_at_their_own_width_stay_on_one_line() {
        let runs = vec![
            Run {
                text: "Total due ".to_owned(),
                style: 0,
            },
            Run {
                text: "AED 254.40 incl. service-charge".to_owned(),
                style: 1,
            },
        ];
        let options = WrapOptions::default();
        let unwrapped = wrap_runs(&runs, None, &measure_kerned, options);
        let width = max_line_width(&unwrapped, &measure_kerned);

        let wrapped = wrap_runs(&runs, Some(width), &measure_kerned, options);
        assert_eq!(wrapped.len(), 1, "{:?} at {width}", lines_of(&wrapped));
    }

    #[test]
    fn a_kerned_line_still_breaks_when_it_does_not_fit() {
        // Measuring the line whole must not make everything fit: one unit
        // short of the full width, the last word still has to move down.
        let text = "Deploy feature/cache-headers";
        let full = measure_kerned(text, 0);
        let lines = lines_of(&wrap_runs(
            &plain(text),
            Some(full - 1.0),
            &measure_kerned,
            WrapOptions::default(),
        ));
        assert_eq!(lines, vec!["Deploy feature/cache-", "headers"]);
    }
}
