//! Completion popup: floats at the cursor over the editor (modeled on `ui/find`).

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};

use crate::app::model::Model;
use crate::ui::editor;

/// Max popup dimensions.
const MAX_ROWS: usize = 8;
const MAX_WIDTH: u16 = 42;

pub fn render(frame: &mut Frame, area: Rect, gutter_w: u16, model: &Model) {
    let Some(comp) = &model.completion else {
        return;
    };
    if comp.items.is_empty() {
        return;
    }
    let Some((cx, cy)) = editor::cursor_screen_pos(model, area, gutter_w) else {
        return;
    };
    let th = &model.theme;

    let rows = comp.items.len().min(MAX_ROWS);
    let height = rows as u16;
    // Widest label+detail, clamped to the editor and MAX_WIDTH.
    let content_w = comp
        .items
        .iter()
        .map(|it| it.label.chars().count() + it.detail.as_ref().map(|d| d.chars().count() + 2).unwrap_or(0))
        .max()
        .unwrap_or(0) as u16
        + 2;
    let width = content_w.clamp(8, MAX_WIDTH.min(area.width.max(8)));

    // Prefer below the caret; flip above if it would overflow the editor bottom.
    let below = cy + 1;
    let y = if below + height <= area.y + area.height {
        below
    } else {
        cy.saturating_sub(height)
    };
    // Keep the box inside the editor horizontally.
    let x = cx.min(area.x + area.width.saturating_sub(width));
    let popup = Rect {
        x,
        y,
        width,
        height,
    };

    // Scroll the item window so the selected row is visible.
    let start = if comp.selected < rows {
        0
    } else {
        comp.selected - rows + 1
    };

    let mut lines: Vec<Line> = Vec::with_capacity(rows);
    for (offset, item) in comp.items.iter().skip(start).take(rows).enumerate() {
        let idx = start + offset;
        let selected = idx == comp.selected;
        let row_bg = if selected { th.selection } else { th.bg_alt };
        let label_style = Style::new().fg(th.fg).bg(row_bg);
        let label = format!(" {}", item.label);
        let mut used = label.chars().count();
        let mut spans = vec![Span::styled(label, label_style)];
        if let Some(detail) = &item.detail {
            let detail = format!("  {detail}");
            used += detail.chars().count();
            spans.push(Span::styled(detail, Style::new().fg(th.fg_dim).bg(row_bg)));
        }
        // Pad to the popup width so the row background covers it edge to edge.
        let pad = (width as usize).saturating_sub(used);
        if pad > 0 {
            spans.push(Span::styled(" ".repeat(pad), Style::new().bg(row_bg)));
        }
        lines.push(Line::from(spans).style(Style::new().bg(row_bg)));
    }

    // Clear first: a row shorter than the popup would otherwise keep the editor
    // text underneath it (Paragraph's style only re-colors, it doesn't blank).
    frame.render_widget(Clear, popup);
    frame.render_widget(Paragraph::new(lines).style(Style::new().bg(th.bg_alt)), popup);
}
