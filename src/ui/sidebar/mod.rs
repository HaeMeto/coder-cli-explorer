//! Sidebar panels: file tree, search, git, extensions.
//!
//! Each panel lives in its own submodule; this module holds the shared layout
//! helpers and the top-level render dispatch.

mod extensions;
mod files;
mod git;
mod search;
mod settings;
mod themes;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};

use crate::app::model::{Model, Panel};

pub use files::{FileHit, file_hit, file_row_at, files_header_hit};
pub use git::{GitHit, git_hit};
pub use search::{SearchHit, search_hit};
pub use settings::{SETTINGS_ITEMS, SettingsItem, settings_row_at};
pub use themes::theme_row_at;

/// Insets a rect by 1 cell on every side (the sidebar's inner padding). Render
/// and all hit-testing pass the sidebar area through this so they stay aligned.
pub(super) fn content_rect(area: Rect) -> Rect {
    Rect {
        x: area.x + 1,
        y: area.y + 1,
        width: area.width.saturating_sub(2),
        height: area.height.saturating_sub(2),
    }
}

/// Rows reserved at the top of the inner sidebar area, above the panel body:
/// the title row (0) and the root/header-actions row (1). Panels without header
/// actions (all but Files) simply leave row 1 blank.
const HEADER_ROWS: u16 = 2;

/// The region where a panel's body is drawn: the sidebar inset by 1 on every
/// side (`content_rect`), minus `HEADER_ROWS` at the top. **This is the single
/// source of truth for panel-body placement** — both `render` and every panel's
/// hit-test derive their coordinates from it, so changing `HEADER_ROWS` (or the
/// inset) moves the visuals and the click targets together, never leaving one
/// stale.
fn panel_area(sidebar: Rect) -> Rect {
    let inner = content_rect(sidebar);
    Rect {
        y: inner.y + HEADER_ROWS,
        height: inner.height.saturating_sub(HEADER_ROWS),
        ..inner
    }
}

/// List scroll offset — keeps the selected item visible.
pub fn list_scroll(selected: usize, len: usize, height: usize) -> usize {
    if height == 0 || len <= height {
        return 0;
    }
    if selected < height {
        0
    } else {
        (selected + 1).saturating_sub(height).min(len - height)
    }
}

pub fn render(frame: &mut Frame, area: Rect, model: &Model) {
    let block = Block::new().style(Style::new().bg(model.theme.bg_alt));
    frame.render_widget(block, area);

    // Title row at the top of the inset area.
    let inner = content_rect(area);
    let title = Paragraph::new(Line::from(Span::styled(
        format!("{} {} ", model.sidebar.active.icon(), model.sidebar.active.title()),
        Style::new().fg(model.theme.fg_dim).add_modifier(Modifier::BOLD),
    )))
    .style(Style::new().bg(model.theme.bg_alt));
    frame.render_widget(title, Rect { height: 1, ..inner });

    // The Files panel gets root "new file"/"new folder" buttons at the right
    // edge of its title row (create directly in the workspace root).
    if model.sidebar.active == Panel::Files {
        files::render_header_actions(frame, area, model);
    }

    // Every panel (and its hit-test) works off the same body region.
    let body = panel_area(area);
    match model.sidebar.active {
        Panel::Files => files::render(frame, body, model),
        Panel::Search => search::render(frame, body, model),
        Panel::Git => git::render(frame, body, model),
        Panel::Extensions => extensions::render(frame, body, model),
        Panel::Themes => themes::render(frame, body, model),
        Panel::Settings => settings::render(frame, body, model),
    }
}
