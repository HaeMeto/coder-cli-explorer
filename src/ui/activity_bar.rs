//! Left icon strip: Files / Search / Git / Extensions.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};

use crate::app::model::{Model, Panel};

pub fn render(frame: &mut Frame, area: Rect, model: &Model) {
    let block = Block::new().style(Style::new().bg(model.theme.activity_bg));
    frame.render_widget(block, area);

    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(""));
    for panel in Panel::ALL {
        let active = model.sidebar.active == panel && model.layout.sidebar_open;
        let style = if active {
            Style::new()
                .fg(model.theme.fg)
                .bg(model.theme.activity_bg)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::new().fg(model.theme.fg_dim).bg(model.theme.activity_bg)
        };
        let marker = if active { "▎" } else { " " };
        let icon = model.panel_icon(panel);
        lines.push(Line::from(vec![
            Span::styled(marker, Style::new().fg(model.theme.accent)),
            Span::styled(format!("{icon} "), style),
        ]));
        lines.push(Line::from(""));
    }

    let p = Paragraph::new(lines).style(Style::new().bg(model.theme.activity_bg));
    frame.render_widget(p, area);
}

/// Returns which panel was clicked based on the mouse y coordinate.
pub fn panel_at(area: Rect, y: u16) -> Option<Panel> {
    // render: 1 blank row, then each panel takes 2 rows (icon + blank).
    if y < area.y + 1 {
        return None;
    }
    let idx = ((y - area.y - 1) / 2) as usize;
    Panel::ALL.get(idx).copied()
}
