//! Quickbar (command palette) overlay, centered on screen. Floats over every
//! other widget; while open it captures all keyboard/mouse input.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph};

use crate::app::model::{Model, QuickbarState};
use crate::ui::text_input::TextInput;

/// Maximum number of list rows drawn (the palette scrolls).
const MAX_ROWS: usize = 12;

/// Top padding before the list inside the popup (border + input row + gap).
const LIST_OFFSET: u16 = 3;

/// The centered rectangle for the quickbar.
pub fn area(term: Rect) -> Rect {
 let width = ((term.width as usize * 2 / 3).max(30)).min(term.width as usize) as u16;
 let height = (LIST_OFFSET + MAX_ROWS as u16).min(term.height);
 let x = term.x + term.width.saturating_sub(width) / 2;
 // Slightly above vertical center so it feels like a command palette.
 let y = term.y + (term.height.saturating_sub(height)) / 2 / 2;
 Rect { x, y, width, height }
}

pub fn render(frame: &mut Frame, model: &Model) {
 let Some(qb) = model.quickbar.as_ref() else {
 return;
 };
 let th = &model.theme;
 let term_rect = frame.area();
 let area = area(term_rect);

 frame.render_widget(Clear, area);
 let block = Block::bordered()
 .title(" Quickbar ")
 .border_style(Style::new().fg(th.accent))
 .style(Style::new().bg(th.bg_alt).fg(th.fg));
 frame.render_widget(block, area);

 let inner_x = area.x + 1;
 let inner_w = area.width.saturating_sub(2);

 // Query input row.
 let input_rect = Rect {
 x: inner_x,
 y: area.y + 1,
 width: inner_w,
 height: 1,
 };
 frame.render_widget(
 TextInput::new(&qb.input, th)
 .placeholder("Type to search files & commands...")
 .focused(true),
 input_rect,
 );

 // Filtered list.
 let start = area.y + LIST_OFFSET;
 let mut lines: Vec<Line> = Vec::new();
 for (i, item) in qb.items.iter().take(MAX_ROWS).enumerate() {
 let selected = i == qb.selected;
 let (fg, bg) = if selected {
 (th.statusbar_fg, th.accent)
 } else {
 (th.fg, th.bg_alt)
 };
 let marker = item.marker();
 let label = item.label();
 lines.push(Line::from(vec![
 Span::styled(
 format!(" {marker} "),
 Style::new().fg(if selected { fg } else { th.fg_dim }).bg(bg),
 ),
 Span::styled(format!("{label:<width$}", width = inner_w.saturating_sub(3) as usize), Style::new().fg(fg).bg(bg)),
 ]));
 }
 if lines.is_empty() {
 let msg = if qb.files_loaded { "No matches" } else { "Loading files..." };
 lines.push(Line::from(Span::styled(
 format!(" {msg}"),
 Style::new().fg(th.fg_dim).bg(th.bg_alt),
 )));
 }
 let list_rect = Rect {
 x: inner_x,
 y: start,
 width: inner_w,
 height: (lines.len() as u16).min(MAX_ROWS as u16),
 };
 frame.render_widget(Paragraph::new(lines).style(Style::new().bg(th.bg_alt)), list_rect);
}

/// Which list row a click landed on, or `None` if the click hit the border /
/// input row / outside the popup. Operates purely on the state so it is easy to
/// unit test.
pub fn hit_on(qb: &QuickbarState, term: Rect, x: u16, y: u16) -> Option<usize> {
 let area = area(term);
 let inside = x >= area.x && x < area.x + area.width && y >= area.y && y < area.y + area.height;
 if !inside || y < area.y + LIST_OFFSET {
 return None; // outside, border, or the input row
 }
 let row = (y - area.y - LIST_OFFSET) as usize;
 qb.items.get(row).map(|_| row)
}

/// The `Model` wrapper around [`hit_on`].
pub fn hit(model: &Model, term: Rect, x: u16, y: u16) -> Option<usize> {
 hit_on(model.quickbar.as_ref()?, term, x, y)
}

#[cfg(test)]
mod tests {
 use super::{area, hit_on};
 use crate::app::model::{Panel, QuickbarItem, QuickbarState};
 use ratatui::layout::Rect;

 const TERM: Rect = Rect { x: 0, y: 0, width: 80, height: 24 };

 fn qb() -> QuickbarState {
 let mut s = QuickbarState::new();
 s.items = vec![
 QuickbarItem::Panel(Panel::Files),
 QuickbarItem::NewFile,
 QuickbarItem::NewFolder,
 QuickbarItem::File { path: "a.rs".into(), rel: "a.rs".into() },
 ];
 s
 }

 #[test]
 fn popup_fits_inside_the_terminal() {
 let a = area(TERM);
 assert!(a.x + a.width <= TERM.width);
 assert!(a.y + a.height <= TERM.height);
 }

 #[test]
 fn clicks_map_to_rows_and_miss_on_border_input_and_outside() {
 let s = qb();
 let a = area(TERM);
 // First list row is LIST_OFFSET below the popup top.
 assert_eq!(hit_on(&s, TERM, a.x + 1, a.y + super::LIST_OFFSET), Some(0));
 assert_eq!(hit_on(&s, TERM, a.x + 1, a.y + super::LIST_OFFSET + 3), Some(3));
 // The input row / top border is not clickable.
 assert_eq!(hit_on(&s, TERM, a.x + 1, a.y), None);
 assert_eq!(hit_on(&s, TERM, a.x + 1, a.y + 1), None);
 // Outside the popup.
 assert_eq!(hit_on(&s, TERM, a.x + a.width + 5, a.y + super::LIST_OFFSET), None);
 }
}
