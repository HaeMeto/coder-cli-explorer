//! Theme picker panel.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::model::{Focus, Model};

use super::{list_scroll, panel_area};

pub(super) fn render(frame: &mut Frame, area: Rect, model: &Model) {
    let t = &model.sidebar.themes;
    let height = area.height as usize;
    let offset = list_scroll(t.selected, t.names.len(), height);

    let mut lines: Vec<Line> = Vec::new();
    for (i, name) in t.names.iter().enumerate().skip(offset).take(height) {
        let selected = i == t.selected;
        let marker = if i == t.selected { "● " } else { "  " };
        let name_style = if selected {
            Style::new().fg(model.theme.fg)
        } else {
            Style::new().fg(model.theme.fg_dim)
        };
        let line_style = if selected && model.focus == Focus::Sidebar {
            Style::new().bg(model.theme.selection)
        } else {
            Style::new().bg(model.theme.bg_alt)
        };
        lines.push(
            Line::from(vec![
                Span::styled(marker, Style::new().fg(model.theme.accent)),
                Span::styled(name.clone(), name_style),
            ])
            .style(line_style),
        );
    }
    let p = Paragraph::new(lines).style(Style::new().bg(model.theme.bg_alt));
    frame.render_widget(p, area);
}

/// Returns the row index in the theme list based on the mouse y.
pub fn theme_row_at(model: &Model, area: Rect, y: u16) -> Option<usize> {
    let body = panel_area(area);
    if y < body.y || y >= body.y + body.height {
        return None;
    }
    let len = model.sidebar.themes.names.len();
    let offset = list_scroll(model.sidebar.themes.selected, len, body.height as usize);
    let idx = offset + (y - body.y) as usize;
    if idx < len { Some(idx) } else { None }
}
