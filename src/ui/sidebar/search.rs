//! Search / replace panel.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::model::{Focus, Model, SearchField};
use crate::ui::text_input::TextInput;

use super::{list_scroll, panel_area};

/// Number of fixed rows above the result list in the search panel: query,
/// blank, replace, blank, replace buttons, blank, regex, match-case,
/// search-hidden, count.
const SEARCH_HEADER_ROWS: u16 = 10;

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
    // The two input rows are drawn by the shared TextInput widget as an overlay
    // (see below); reserve blank sunken rows for them here.
    let blank_input = || Line::from(Span::styled(" ".repeat(width), Style::new().bg(th.input_bg())));
    let mut lines: Vec<Line> = vec![
        // 0) Search input (overlaid).
        blank_input(),
        Span::from("").into(),
        // 2) Replace input (overlaid).
        blank_input(),
        Span::from("").into(),
        // 4) Replace / Replace All buttons.
        two_button_line(th, width, "Replace", has_query && has_results, "Replace All", has_query),
        Span::from("").into(),
        // 6) Regex checkbox (+ shortcut hint).
        Line::from(vec![
            Span::styled(
                format!(" {} RegExp", if s.use_regex { "[x]" } else { "[ ]" }),
                Style::new().fg(if s.use_regex { th.accent } else { th.fg_dim }),
            ),
        ]),
        // 7) Match Case checkbox.
        check(s.match_case, "Match Case"),
        // 8) Search Ignored & Hidden Files checkbox.
        check(s.search_hidden, "Search Ignored & Hidden"),
        // 9) Result count.
        Line::from(Span::styled(
            format!("- {} results: -", s.results.len()),
            Style::new().fg(th.fg_dim),
        )),
    ];

    // 6+) Results — two rows per hit: file path (dim + underlined), then the matched line.
    let list_h = (area.height as usize).saturating_sub(SEARCH_HEADER_ROWS as usize);
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

    // Overlay the query/replace inputs on their reserved rows (indices 0 and 2).
    let input_row = |dy: u16| Rect { x: area.x, y: area.y + dy, width: area.width, height: 1 };
    frame.render_widget(
        TextInput::new(&s.query, th)
            .placeholder("Find...")
            .focused(query_active)
            .pad(1),
        input_row(0),
    );
    frame.render_widget(
        TextInput::new(&s.replace, th)
            .placeholder("Replace...")
            .focused(replace_active)
            .pad(1),
        input_row(2),
    );
}

/// Target of a mouse click in the search panel.
pub enum SearchHit {
    QueryField,
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
    let base = body.y; // first body row = the query input
    // Body rows (see `render`): query(0), blank(1), replace(2), blank(3),
    // replace buttons(4), blank(5), regex(6), match-case(7), search-hidden(8),
    // count(9), then the result list (`SEARCH_HEADER_ROWS`).
    if y == base {
        return Some(SearchHit::QueryField);
    }
    if y == base + 2 {
        return Some(SearchHit::ReplaceField);
    }
    if y == base + 4 {
        return two_button_hit(body.x, body.width, x)
            .map(|right| if right { SearchHit::ReplaceAll } else { SearchHit::ReplaceOne });
    }
    if y == base + 6 {
        return Some(SearchHit::RegexToggle);
    }
    if y == base + 7 {
        return Some(SearchHit::MatchCaseToggle);
    }
    if y == base + 8 {
        return Some(SearchHit::SearchHiddenToggle);
    }
    let start = base + SEARCH_HEADER_ROWS;
    if y < start {
        return None;
    }
    // Two rows per result: file path then matched line.
    let list_h = body.height.saturating_sub(SEARCH_HEADER_ROWS) as usize;
    let per_page = (list_h / 2).max(1);
    let offset = list_scroll(s.selected, s.results.len(), per_page);
    let idx = offset + ((y - start) / 2) as usize;
    if idx < s.results.len() {
        Some(SearchHit::Result(idx))
    } else {
        None
    }
}
