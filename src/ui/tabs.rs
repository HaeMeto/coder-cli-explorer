//! Open file tabs.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::model::Model;

pub fn render(frame: &mut Frame, area: Rect, model: &Model) {
    let mut spans: Vec<Span> = Vec::new();
    if model.tabs.is_empty() {
        spans.push(Span::styled(
            " No file open ",
            Style::new().fg(model.theme.fg_dim).bg(model.theme.tab_inactive_bg),
        ));
    }
    for (i, tab) in model.tabs.iter().enumerate() {
        let active = Some(i) == model.active_tab;
        let (fg, bg) = if active {
            (model.theme.fg, model.theme.tab_active_bg)
        } else {
            (model.theme.fg_dim, model.theme.tab_inactive_bg)
        };
        let dirty = if tab.buffer.dirty { "●" } else { " " };
        let mut style = Style::new().fg(fg).bg(bg);
        if active {
            style = style.add_modifier(Modifier::BOLD);
        }
        spans.push(Span::styled(
            format!(" {} {} ", tab.title(), dirty),
            style,
        ));
        // Close button (clicking it closes the tab).
        spans.push(Span::styled(
            "✕ ",
            Style::new().fg(model.theme.fg_dim).bg(bg),
        ));
        spans.push(Span::styled("│", Style::new().fg(model.theme.border).bg(bg)));
    }
    let p = Paragraph::new(Line::from(spans)).style(Style::new().bg(model.theme.tab_inactive_bg));
    frame.render_widget(p, area);
}

/// Target of a click on the tab bar.
pub enum TabHit {
    /// Tab body — activate.
    Select(usize),
    /// Close button (✕) — close the tab.
    Close(usize),
}

/// Returns which tab / close button was clicked based on the mouse x.
pub fn tab_at(model: &Model, area: Rect, x: u16) -> Option<TabHit> {
    let mut cursor = area.x;
    for (i, tab) in model.tabs.iter().enumerate() {
        // " {title} {dirty} ✕ │"
        let w_name = tab.title().chars().count() as u16;
        let total = w_name + 6 + 1; // body (name+6) + separator "│"
        if x >= cursor && x < cursor + total {
            // ✕ position: " " + name + " " + dirty + " " = name+4 offset.
            // ✕ and the trailing space form the close region (wide target for the mouse).
            let close_x = cursor + w_name + 4;
            if x == close_x || x == close_x + 1 {
                return Some(TabHit::Close(i));
            }
            return Some(TabHit::Select(i));
        }
        cursor += total;
    }
    None
}
