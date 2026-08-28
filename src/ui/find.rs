//! In-editor find / replace widget: floats over the top-right of the editor.
//!
//! Layout (right-aligned inside the editor area):
//! ```text
//! [ query...................  1/3  ‹ › ✕ ]
//! [ replace................ Replace  Replace All ]   (replace_mode only)
//! ```
//! `layout()` computes the geometry once; `render()` draws it and `hit()` maps a
//! mouse click to a target. Both share `layout()` so visuals and hit-testing agree.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};

use crate::app::model::{FindField, Focus, Model};
use crate::ui::text_input::TextInput;

/// Target of a mouse click inside the find widget.
pub enum FindHit {
    QueryField,
    ReplaceField,
    Prev,
    Next,
    Close,
    ReplaceOne,
    ReplaceAll,
}

/// Geometry of the find widget (absolute coordinates).
pub struct FindLayout {
    pub area: Rect,
    pub find_y: u16,
    pub replace_y: Option<u16>,
    /// Query input cell region.
    pub query: Rect,
    /// Count text region (e.g. "1/3").
    pub count: Rect,
    pub prev_x: u16,
    pub next_x: u16,
    pub close_x: u16,
    /// Replace input cell region (replace mode).
    pub replace: Rect,
    /// "Replace" button column range [start, end).
    pub replace_btn: (u16, u16),
    /// "Replace All" button column range [start, end).
    pub replace_all_btn: (u16, u16),
}

const TARGET_WIDTH: u16 = 46;

/// Computes the widget layout, or `None` when closed / no room.
pub fn layout(model: &Model, editor: Rect) -> Option<FindLayout> {
    if !model.find.open || editor.height == 0 {
        return None;
    }
    // Outer box, with a 1-cell padding on every side; content lives inside.
    let outer_w = TARGET_WIDTH.min(editor.width.saturating_sub(1));
    if outer_w < 26 {
        return None;
    }
    let cw = outer_w - 2; // inner content width
    // Content rows: find row [+ spacer + replace]; plus a padding row top and bottom.
    let content_rows = if model.find.replace_mode { 3 } else { 1 };
    let area = Rect {
        x: editor.x + editor.width - outer_w,
        y: editor.y,
        width: outer_w,
        height: content_rows + 2,
    };
    let ix = area.x + 1; // inner left edge
    let find_y = area.y + 1; // inner top edge

    // Find-row tail (flush right within content): "count ‹ › ✕".
    let cnt = model.find.count_label().chars().count() as u16;
    let close_x = ix + cw - 1;
    let next_x = ix + cw - 3;
    let prev_x = ix + cw - 5;
    let count_end = cw - 6; // exclusive content column
    let count_start = count_end.saturating_sub(cnt);
    let count = Rect {
        x: ix + count_start,
        y: find_y,
        width: cnt,
        height: 1,
    };
    let query = Rect {
        x: ix,
        y: find_y,
        width: count_start.saturating_sub(1).max(1),
        height: 1,
    };

    // Replace-row tail (flush right): [ Replace ]  [ Replace All ], 2-col gap.
    let ral_w = 13; // " Replace All "
    let rep_w = 9; // " Replace "
    let gap = 2;
    let ral_start = cw.saturating_sub(ral_w);
    let rep_end = ral_start.saturating_sub(gap);
    let rep_start = rep_end.saturating_sub(rep_w);
    let replace_row_y = find_y + 2; // find row, blank spacer, then replace row
    let replace = Rect {
        x: ix,
        y: replace_row_y,
        width: rep_start.saturating_sub(1).max(1),
        height: 1,
    };
    let replace_y = if model.find.replace_mode {
        Some(replace_row_y)
    } else {
        None
    };

    Some(FindLayout {
        area,
        find_y,
        replace_y,
        query,
        count,
        prev_x,
        next_x,
        close_x,
        replace,
        replace_btn: (ix + rep_start, ix + rep_end),
        replace_all_btn: (ix + ral_start, ix + cw),
    })
}

/// Maps a mouse click to a widget target, or `None` when the click is outside.
pub fn hit(model: &Model, editor: Rect, x: u16, y: u16) -> Option<FindHit> {
    let l = layout(model, editor)?;
    if x < l.area.x || x >= l.area.x + l.area.width || y < l.area.y || y >= l.area.y + l.area.height
    {
        return None;
    }
    if y == l.find_y {
        if x == l.close_x {
            return Some(FindHit::Close);
        }
        if x == l.next_x {
            return Some(FindHit::Next);
        }
        if x == l.prev_x {
            return Some(FindHit::Prev);
        }
        return Some(FindHit::QueryField);
    }
    if Some(y) == l.replace_y {
        let (rs, re) = l.replace_btn;
        let (as_, ae) = l.replace_all_btn;
        if x >= rs && x < re {
            return Some(FindHit::ReplaceOne);
        }
        if x >= as_ && x < ae {
            return Some(FindHit::ReplaceAll);
        }
        return Some(FindHit::ReplaceField);
    }
    Some(FindHit::QueryField)
}

/// Renders the widget on top of the editor.
pub fn render(frame: &mut Frame, editor: Rect, model: &Model) {
    let Some(l) = layout(model, editor) else {
        return;
    };
    let th = &model.theme;
    let focused = model.focus == Focus::Find;
    let ascii = model.ascii_icons;

    // Widget background. Clear first to wipe the editor glyphs underneath —
    // a styled Paragraph only recolors cells, it doesn't blank their symbols,
    // so without Clear the text behind shows through the empty parts.
    frame.render_widget(Clear, l.area);
    frame.render_widget(
        Paragraph::new("").style(Style::new().bg(th.bg_alt)),
        l.area,
    );

    // --- Find row ---
    let q_focused = focused && model.find.field == FindField::Query;
    frame.render_widget(
        TextInput::new(&model.find.query, th)
            .placeholder("Find...")
            .focused(q_focused),
        l.query,
    );

    let count_style = if model.find.matches.is_empty() && !model.find.query.is_empty() {
        Style::new().fg(th.git_deleted).bg(th.bg_alt)
    } else {
        Style::new().fg(th.fg_dim).bg(th.bg_alt)
    };
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(model.find.count_label(), count_style)))
            .style(Style::new().bg(th.bg_alt)),
        l.count,
    );

    let mut btn = |ch: &'static str, x: u16| {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                ch,
                Style::new().fg(th.fg).bg(th.bg_alt),
            )))
            .style(Style::new().bg(th.bg_alt)),
            Rect { x, y: l.find_y, width: 1, height: 1 },
        );
    };
    btn(if ascii { "<" } else { "‹" }, l.prev_x);
    btn(if ascii { ">" } else { "›" }, l.next_x);
    btn(if ascii { "x" } else { "✕" }, l.close_x);

    // --- Replace row ---
    if let Some(ry) = l.replace_y {
        let r_focused = focused && model.find.field == FindField::Replace;
        frame.render_widget(
            TextInput::new(&model.find.replace, th)
                .placeholder("Replace...")
                .focused(r_focused),
            l.replace,
        );

        let can = !model.find.matches.is_empty();
        let (fg, bg) = if can {
            (th.statusbar_fg, th.accent)
        } else {
            (th.fg_dim, th.tab_inactive_bg)
        };
        let (rs, re) = l.replace_btn;
        let (as_, ae) = l.replace_all_btn;
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(" Replace ", Style::new().fg(fg).bg(bg))))
                .style(Style::new().bg(th.bg_alt)),
            Rect { x: rs, y: ry, width: re - rs, height: 1 },
        );
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                " Replace All ",
                Style::new().fg(fg).bg(bg),
            )))
            .style(Style::new().bg(th.bg_alt)),
            Rect { x: as_, y: ry, width: ae - as_, height: 1 },
        );
    }
}
