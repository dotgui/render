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

/// Breaks styled runs into rendered lines, wrapping across style boundaries.
///
/// `measure` is given a string and the style index it belongs to. Text flows
/// continuously between runs, so a bold word mid-sentence does not force a
/// break, and a break can fall inside a run.
pub(crate) fn wrap_runs(
    runs: &[Run],
    max_width: Option<f32>,
    measure: &dyn Fn(&str, usize) -> f32,
) -> Vec<Line> {
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
    let mut current_width = 0.0_f32;
    // Whitespace at a run boundary belongs between the runs, not inside either,
    // so `<segment value="Total: "/><segment value="$12"/>` keeps its space.
    let mut pending_space = false;

    for chunk in chunks {
        if chunk.hard_break_before {
            lines.push(std::mem::take(&mut current));
            current_width = 0.0;
            pending_space = false;
        }

        let starts_with_space = chunk.text.starts_with(char::is_whitespace);
        for (index, piece) in break_pieces(chunk.text).into_iter().enumerate() {
            let separated = if index == 0 {
                starts_with_space || pending_space
            } else {
                piece.leading_space
            };

            let piece_width = measure(piece.text, chunk.style);
            let space_width = if current.is_empty() || !separated {
                0.0
            } else {
                measure(" ", chunk.style)
            };

            let overflows = max_width.is_some_and(|max_width| {
                !current.is_empty()
                    && current_width + space_width + piece_width > max_width.max(1.0) + 0.5
            });

            if overflows {
                // The separating space is dropped, as it would be at the start
                // of a line in HTML. A piece broken off mid-word never had one.
                lines.push(std::mem::take(&mut current));
                push_piece(&mut current, piece.text, chunk.style);
                current_width = piece_width;
            } else {
                if space_width > 0.0 {
                    push_piece(&mut current, " ", chunk.style);
                }
                push_piece(&mut current, piece.text, chunk.style);
                current_width += space_width + piece_width;
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
fn break_pieces(line: &str) -> Vec<Piece<'_>> {
    let mut pieces = Vec::new();

    for (index, word) in line.split_whitespace().enumerate() {
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
pub(crate) fn line_width(line: &Line, measure: &dyn Fn(&str, usize) -> f32) -> f32 {
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
        lines_of(&wrap_runs(&plain(value), max_width, &measure_run))
    }

    fn plain(text: &str) -> Vec<Run> {
        vec![Run {
            text: text.to_owned(),
            style: 0,
        }]
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
}
