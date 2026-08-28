//! File-tree context menu (right-click). Floats over everything below the
//! dialog; while open it captures all keyboard/mouse input.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph};

use crate::app::model::{ContextMenu, MenuItem, Model};

/// Inner width: the longest "label + gap + shortcut" pair.
fn inner_width() -> u16 {
    MenuItem::ALL
        .iter()
        .map(|i| (i.label().chars().count() + i.shortcut().chars().count() + 4) as u16)
        .max()
        .unwrap_or(20)
}

/// The menu rectangle: anchored at the click, shifted to stay on screen.
pub fn menu_rect(m: &ContextMenu, term: Rect) -> Rect {
    let width = (inner_width() + 2).min(term.width);
    let height = (MenuItem::ALL.len() as u16 + 2).min(term.height);
    // Flip/shift back when the menu would run off the right or bottom edge.
    let x = m.x.min(term.width.saturating_sub(width));
    let y = m.y.min(term.height.saturating_sub(height));
    Rect { x, y, width, height }
}

pub fn render(frame: &mut Frame, model: &Model) {
    let Some(m) = model.context_menu.as_ref() else {
        return;
    };
    let th = &model.theme;
    let area = menu_rect(m, frame.area());

    frame.render_widget(Clear, area);
    let block = Block::bordered()
        .border_style(Style::new().fg(th.accent))
        .style(Style::new().bg(th.bg_alt).fg(th.fg));
    frame.render_widget(block, area);

    let inner = Rect {
        x: area.x + 1,
        y: area.y + 1,
        width: area.width.saturating_sub(2),
        height: area.height.saturating_sub(2),
    };
    let w = inner.width as usize;
    let lines: Vec<Line> = MenuItem::ALL
        .iter()
        .enumerate()
        .map(|(i, item)| {
            let selected = i == m.selected;
            let (fg, bg) = if selected {
                (th.statusbar_fg, th.accent)
            } else {
                (th.fg, th.bg_alt)
            };
            let label = item.label();
            let shortcut = item.shortcut();
            // Label left, shortcut right-aligned within the inner width.
            let gap = w.saturating_sub(label.chars().count() + shortcut.chars().count() + 2);
            Line::from(vec![
                Span::styled(format!(" {label}"), Style::new().fg(fg).bg(bg)),
                Span::styled(" ".repeat(gap), Style::new().bg(bg)),
                Span::styled(
                    format!("{shortcut} "),
                    Style::new()
                        .fg(if selected { fg } else { th.fg_dim })
                        .bg(bg)
                        .add_modifier(Modifier::DIM),
                ),
            ])
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);
}

/// What a click landed on: `Some(item)` inside the list, `None` elsewhere
/// (the caller closes the menu).
pub fn hit(m: &ContextMenu, term: Rect, x: u16, y: u16) -> Option<MenuItem> {
    let area = menu_rect(m, term);
    let inside = x >= area.x && x < area.x + area.width && y >= area.y && y < area.y + area.height;
    if !inside || y == area.y || y == area.y + area.height - 1 {
        return None; // outside, or on the border rows
    }
    let idx = (y - area.y - 1) as usize;
    MenuItem::ALL.get(idx).copied()
}

#[cfg(test)]
mod tests {
    use super::{hit, menu_rect};
    use crate::app::model::{ContextMenu, MenuItem};
    use ratatui::layout::Rect;

    const TERM: Rect = Rect { x: 0, y: 0, width: 80, height: 24 };

    #[test]
    fn menu_stays_on_screen_when_opened_at_the_edge() {
        let m = ContextMenu::new(0, 78, 23);
        let r = menu_rect(&m, TERM);
        assert!(r.x + r.width <= TERM.width);
        assert!(r.y + r.height <= TERM.height);
    }

    #[test]
    fn clicks_map_to_items_and_miss_on_the_border() {
        let m = ContextMenu::new(0, 10, 5);
        let r = menu_rect(&m, TERM);
        // First body row is one below the top border.
        assert_eq!(hit(&m, TERM, r.x + 1, r.y + 1), Some(MenuItem::NewFile));
        assert_eq!(hit(&m, TERM, r.x + 1, r.y + 4), Some(MenuItem::Delete));
        assert_eq!(hit(&m, TERM, r.x + 1, r.y), None); // top border
        assert_eq!(hit(&m, TERM, r.x + 1, r.y + r.height - 1), None); // bottom border
        assert_eq!(hit(&m, TERM, r.x + r.width + 2, r.y + 1), None); // outside
    }
}
