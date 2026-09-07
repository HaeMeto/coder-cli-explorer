//! Search / replace panel.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::model::{Focus, Model, SearchField};
use crate::ui::text_input::TextInput;

use super::{list_scroll, panel_area};

/// One header row kind, top to bottom. Both `render` and `search_hit` walk the
/// same `header_rows` list for the current mode, so the two can never drift
/// apart the way two independently hand-counted row offsets could.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Row {
    Query,
    Blank,
    /// The "[ ] Replace" checkbox that shows/hides the rows below.
    ReplaceToggle,
    ReplaceInput,
    ReplaceButtons,
    Regex,
    MatchCase,
    SearchHidden,
    Count,
}

/// The header rows for the current mode. The replace input and its buttons
/// only appear once "Replace" is toggled on — otherwise the panel stays a
/// compact "just find" view instead of always showing replace UI up front.
fn header_rows(replace_mode: bool) -> Vec<Row> {
    let mut rows = vec![Row::Query, Row::Blank, Row::ReplaceToggle];
    if replace_mode {
        rows.extend([Row::Blank, Row::ReplaceInput, Row::Blank, Row::ReplaceButtons]);
    }
    rows.extend([Row::Blank, Row::Regex, Row::MatchCase, Row::SearchHidden, Row::Count]);
    rows
}

/// Builds a two-button row (left | gap | right) sized to `width`; each cell is
/// accent-colored when enabled, dim otherwise. Splits match `two_button_hit`.
fn two_button_line(
    th: &crate::core::theme::Theme,
    width: usize,
    left: &str,
    left_on: bool,
    right: &str,
    right_on: bool,
) -> Line<'static> {
    let lw = width.saturating_sub(1) / 2;
    let rw = width.saturating_sub(lw + 1);
    let cell = |label: &str, on: bool, w: usize| -> Span<'static> {
        let (fg, bg) = if on {
            (th.statusbar_fg, th.accent)
        } else {
            (th.fg_dim, th.tab_inactive_bg)
        };
        Span::styled(format!("{label:^w$}"), Style::new().fg(fg).bg(bg))
    };
    Line::from(vec![
        cell(left, left_on, lw),
        Span::styled(" ", Style::new().bg(th.bg_alt)),
        cell(right, right_on, rw),
    ])
}

/// Which half (if any) of a two-button row column `x` falls into: `Some(false)`
/// = left, `Some(true)` = right, `None` = the gap.
fn two_button_hit(area_x: u16, width: u16, x: u16) -> Option<bool> {
    let rel = x.checked_sub(area_x)?;
    let lw = width.saturating_sub(1) / 2;
    if rel < lw {
        Some(false)
    } else if rel == lw {
        None // gap
    } else {
        Some(true)
    }
}

pub(super) fn render(frame: &mut Frame, area: Rect, model: &Model) {
    let s = &model.sidebar.search;
    let th = &model.theme;
    let input_focused = model.focus == Focus::SearchInput;
    let query_active = input_focused && s.field == SearchField::Query;
    let replace_active = input_focused && s.field == SearchField::Replace;
    let width = area.width as usize;

    let has_results = !s.results.is_empty();
    let has_query = !s.query.is_empty();

    // A checkbox row: "[x] Label", accent when on, dim when off.
    let check = |on: bool, label: &str| -> Line<'static> {
        let box_ = if on { "[x]" } else { "[ ]" };
        Line::from(Span::styled(
            format!(" {box_} {label}"),
            Style::new().fg(if on { th.accent } else { th.fg_dim }),
        ))
    };
    // Input rows are drawn by the shared TextInput widget as an overlay (see
    // below); reserve a blank sunken row for them here.
    let blank_input = || Line::from(Span::styled(" ".repeat(width), Style::new().bg(th.input_bg())));

    let rows = header_rows(s.replace_mode);
    let mut lines: Vec<Line> = Vec::with_capacity(rows.len());
    // Rows of the query/replace inputs, so they can be overlaid afterward at
    // their actual (mode-dependent) position instead of a hardcoded index.
    let mut query_row: u16 = 0;
    let mut replace_row: Option<u16> = None;
    for (i, row) in rows.iter().enumerate() {
        lines.push(match row {
            Row::Query => {
                query_row = i as u16;
                blank_input()
            }
            Row::Blank => Span::from("").into(),
            Row::ReplaceToggle => check(s.replace_mode, "Replace"),
            Row::ReplaceInput => {
                replace_row = Some(i as u16);
                blank_input()
            }
            Row::ReplaceButtons => two_button_line(
                th,
                width,
                "Replace",
                has_query && has_results,
                "Replace All",
                has_query,
            ),
            Row::Regex => Line::from(Span::styled(
                format!(" {} RegExp", if s.use_regex { "[x]" } else { "[ ]" }),
                Style::new().fg(if s.use_regex { th.accent } else { th.fg_dim }),
            )),
            Row::MatchCase => check(s.match_case, "Match Case"),
            Row::SearchHidden => check(s.search_hidden, "Search Ignored & Hidden"),
            Row::Count => Line::from(Span::styled(
                format!("- {} results: -", s.results.len()),
                Style::new().fg(th.fg_dim),
            )),
        });
    }

    // Results — two rows per hit: file path (dim + underlined), then the matched line.
    let list_h = (area.height as usize).saturating_sub(rows.len());
    let per_page = (list_h / 2).max(1);
    let offset = list_scroll(s.selected, s.results.len(), per_page);
    for (i, m) in s.results.iter().enumerate().skip(offset).take(per_page) {
        let selected = i == s.selected;
        let bg = if selected { th.selected_bg() } else { th.bg_alt };
        lines.push(
            Line::from(Span::styled(
                format!(" - {}:{}", m.rel, m.line_no),
                Style::new().fg(th.fg_dim),
            ))
            .style(Style::new().bg(bg)),
        );
        lines.push(
            Line::from(Span::raw(m.line.trim().to_string())).style(Style::new().bg(bg).fg(th.fg)),
        );
    }
    let p = Paragraph::new(lines).style(Style::new().bg(th.bg_alt));
    frame.render_widget(p, area);

    // Overlay the query/replace inputs on their reserved rows.
    let input_row = |dy: u16| Rect { x: area.x, y: area.y + dy, width: area.width, height: 1 };
    frame.render_widget(
        TextInput::new(&s.query, th)
            .placeholder("Find...")
            .focused(query_active)
            .pad(1),
        input_row(query_row),
    );
    if let Some(replace_row) = replace_row {
        frame.render_widget(
            TextInput::new(&s.replace, th)
                .placeholder("Replace...")
                .focused(replace_active)
                .pad(1),
            input_row(replace_row),
        );
    }
}

/// Target of a mouse click in the search panel.
pub enum SearchHit {
    QueryField,
    ReplaceModeToggle,
    ReplaceField,
    RegexToggle,
    MatchCaseToggle,
    SearchHiddenToggle,
    ReplaceOne,
    ReplaceAll,
    Result(usize),
}

/// Maps a mouse (x, y) to a search-panel target. `area` is the full sidebar area.
pub fn search_hit(model: &Model, area: Rect, x: u16, y: u16) -> Option<SearchHit> {
    let body = panel_area(area);
    let s = &model.sidebar.search;
    let rows = header_rows(s.replace_mode);
    let rel = y.checked_sub(body.y)?;

    if let Some(row) = rows.get(rel as usize) {
        return match row {
            Row::Query => Some(SearchHit::QueryField),
            Row::ReplaceToggle => Some(SearchHit::ReplaceModeToggle),
            Row::ReplaceInput => Some(SearchHit::ReplaceField),
            Row::ReplaceButtons => two_button_hit(body.x, body.width, x)
                .map(|right| if right { SearchHit::ReplaceAll } else { SearchHit::ReplaceOne }),
            Row::Regex => Some(SearchHit::RegexToggle),
            Row::MatchCase => Some(SearchHit::MatchCaseToggle),
            Row::SearchHidden => Some(SearchHit::SearchHiddenToggle),
            Row::Blank | Row::Count => None,
        };
    }

    // Past the header: the result list, two rows per hit.
    let start = body.y + rows.len() as u16;
    if y < start {
        return None;
    }
    let list_h = body.height.saturating_sub(rows.len() as u16) as usize;
    let per_page = (list_h / 2).max(1);
    let offset = list_scroll(s.selected, s.results.len(), per_page);
    let idx = offset + ((y - start) / 2) as usize;
    if idx < s.results.len() {
        Some(SearchHit::Result(idx))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_is_compact_without_replace_mode() {
        let off = header_rows(false);
        let on = header_rows(true);
        assert!(!off.contains(&Row::ReplaceInput));
        assert!(!off.contains(&Row::ReplaceButtons));
        assert!(on.contains(&Row::ReplaceInput));
        assert!(on.contains(&Row::ReplaceButtons));
        assert!(on.len() > off.len(), "replace mode adds rows, never removes any");
    }

    #[test]
    fn every_row_kind_appears_exactly_once_per_mode() {
        // Regression guard: `render` and `search_hit` both derive their offsets
        // from this same list, so a duplicated/missing row would misalign one
        // against the other. Every kind except `Blank` is unique per row.
        for replace_mode in [false, true] {
            let rows = header_rows(replace_mode);
            for kind in [
                Row::Query,
                Row::ReplaceToggle,
                Row::Regex,
                Row::MatchCase,
                Row::SearchHidden,
                Row::Count,
            ] {
                assert_eq!(
                    rows.iter().filter(|r| **r == kind).count(),
                    1,
                    "mode={replace_mode}"
                );
            }
        }
    }
}
