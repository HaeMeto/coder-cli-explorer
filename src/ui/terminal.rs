//! Draws the embedded PTY terminal from the vt100 screen into ratatui cells.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, Borders};

use crate::app::model::{Focus, Model};

pub fn render(frame: &mut Frame, area: Rect, model: &Model) {
    let focused = model.focus == Focus::Terminal;
    let border_color = if focused {
        model.theme.accent
    } else {
        model.theme.border
    };
    let block = Block::new()
        .borders(Borders::TOP)
        .border_style(Style::new().fg(border_color))
        .title(" TERMINAL ")
        .title_style(Style::new().fg(model.theme.fg_dim))
        .style(Style::new().bg(Color::Rgb(20, 20, 20)));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    // vt100's visible grid already reflects the scrollback view position
    // (set via parser.set_scrollback in the update layer), so cell()/rows()/
    // cursor_position() all operate on what should be shown right now.
    let screen = model.terminal.parser.screen();
    let (rows, cols) = screen.size();
    let selection = model.terminal.selection;

    // Reserve the rightmost inner column for the scrollbar. The PTY grid width
    // is kept one column narrower (see sync_terminal_size) so nothing is hidden.
    let has_bar = inner.width > 1;
    let text_w = if has_bar { inner.width - 1 } else { inner.width };

    let buf = frame.buffer_mut();

    for r in 0..inner.height.min(rows) {
        for c in 0..text_w.min(cols) {
            let x = inner.x + c;
            let y = inner.y + r;
            let Some(cell) = screen.cell(r, c) else { continue };
            let Some(out) = buf.cell_mut((x, y)) else { continue };
            let contents = cell.contents();
            if contents.is_empty() {
                out.set_char(' ');
            } else {
                out.set_symbol(&contents);
            }
            let mut style = Style::new()
                .fg(conv_color(cell.fgcolor(), model.theme.fg))
                .bg(conv_color(cell.bgcolor(), Color::Rgb(20, 20, 20)));
            if cell.bold() {
                style = style.add_modifier(Modifier::BOLD);
            }
            if cell.italic() {
                style = style.add_modifier(Modifier::ITALIC);
            }
            if cell.underline() {
                style = style.add_modifier(Modifier::UNDERLINED);
            }
            if cell.inverse() {
                style = style.add_modifier(Modifier::REVERSED);
            }
            if selection_contains(&selection, r, c) {
                style = style.bg(model.theme.selection);
            }
            out.set_style(style);
        }
    }

    if has_bar {
        render_scrollbar(buf, inner, model);
    }

    // Cursor — only while following the live bottom; when scrolled back into
    // history it would land on the wrong row.
    if focused && model.terminal.scroll_offset == 0 && !screen.hide_cursor() {
        let (cr, cc) = screen.cursor_position();
        let x = inner.x + cc;
        let y = inner.y + cr;
        if x < inner.x + text_w && y < inner.y + inner.height {
            frame.set_cursor_position((x, y));
        }
    }
}

/// Draws the scrollback scrollbar in the rightmost inner column. The thumb
/// covers the visible window's proportion of the whole scrollback + screen.
fn render_scrollbar(buf: &mut ratatui::buffer::Buffer, inner: Rect, model: &Model) {
    let th = &model.theme;
    let h = inner.height as usize;
    if h == 0 {
        return;
    }
    let x = inner.x + inner.width - 1;
    let rows = model.terminal.rows as usize;
    let total = model.terminal.scrollback_lines + rows;
    let offset = model.terminal.scroll_offset;

    // How many history rows sit above the top of the viewport.
    let above = model.terminal.scrollback_lines.saturating_sub(offset);
    let (thumb_start, thumb_end) = if total > rows && total > 0 {
        let ts = above * h / total;
        let tl = (rows * h / total).max(1);
        (ts, (ts + tl).min(h))
    } else {
        (0, h)
    };

    for y in 0..h {
        let in_thumb = y >= thumb_start && y < thumb_end;
        let bg = if in_thumb { th.fg_dim } else { th.bg_alt };
        if let Some(out) = buf.cell_mut((x, inner.y + y as u16)) {
            out.set_char(' ');
            out.set_style(Style::new().bg(bg));
        }
    }
}

/// Returns true if the given visible-grid (row, col) falls within the selection.
fn selection_contains(
    selection: &Option<(u16, u16, u16, u16)>,
    row: u16,
    col: u16,
) -> bool {
    let Some((r1, c1, r2, c2)) = *selection else {
        return false;
    };
    // Linear (logical) selection matching contents_between: fully-covered rows
    // between the endpoints, partial first/last rows.
    let ((sr, sc), (er, ec)) = if (r1, c1) <= (r2, c2) {
        ((r1, c1), (r2, c2))
    } else {
        ((r2, c2), (r1, c1))
    };
    if row < sr || row > er {
        return false;
    }
    if sr == er {
        return col >= sc && col <= ec;
    }
    if row == sr {
        return col >= sc;
    }
    if row == er {
        return col <= ec;
    }
    true
}

fn conv_color(c: vt100::Color, default: Color) -> Color {
    match c {
        vt100::Color::Default => default,
        vt100::Color::Idx(i) => Color::Indexed(i),
        vt100::Color::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
}
