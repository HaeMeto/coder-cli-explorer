//! Settings panel: quick links to the editable config files, an inline
//! toggle, and the reset-to-defaults actions (bottom), separated by a blank row.
//!
//! The gear no longer opens `config.toml` directly — it opens this panel.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::model::{Focus, Model};

use super::panel_area;

/// A clickable row in the Settings panel.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SettingsItem {
    EditKeybindings,
    EditConfig,
    /// Toggle: plain ASCII glyphs instead of Nerd Font / Codicon icons — the
    /// escape hatch for a terminal font that doesn't bundle those Private-Use
    /// codepoints (they render as tiny fallback/placeholder glyphs then).
    AsciiIcons,
    ResetKeybindings,
    ResetConfig,
}

/// The actionable rows, in selection order (Edit group, toggle, Reset group).
/// Selection (`settings_selected`) and mouse hit-testing index into this array.
pub const SETTINGS_ITEMS: [SettingsItem; 5] = [
    SettingsItem::EditKeybindings,
    SettingsItem::EditConfig,
    SettingsItem::AsciiIcons,
    SettingsItem::ResetKeybindings,
    SettingsItem::ResetConfig,
];

/// Visual layout, top to bottom: the two edit rows, the toggle, a blank
/// spacer, the two reset rows. `Some(i)` is an actionable row indexing
/// `SETTINGS_ITEMS`; `None` is the spacer. Render and hit-testing both walk
/// this so they stay aligned.
const LAYOUT: [Option<usize>; 6] = [Some(0), Some(1), Some(2), None, Some(3), Some(4)];

/// Row label. A `String` (not `&'static str`) because the toggle row's
/// checkbox state depends on the model.
fn label(item: SettingsItem, model: &Model) -> String {
    match item {
        SettingsItem::EditKeybindings => "Edit keybindings.toml".to_string(),
        SettingsItem::EditConfig => "Edit config.toml".to_string(),
        SettingsItem::AsciiIcons => format!(
            "[{}] ASCII icons (no Nerd Font needed)",
            if model.ascii_icons { "x" } else { " " }
        ),
        SettingsItem::ResetKeybindings => "Reset keybindings to defaults".to_string(),
        SettingsItem::ResetConfig => "Reset config to defaults".to_string(),
    }
}

fn is_reset(item: SettingsItem) -> bool {
    matches!(
        item,
        SettingsItem::ResetKeybindings | SettingsItem::ResetConfig
    )
}

pub(super) fn render(frame: &mut Frame, area: Rect, model: &Model) {
    let sel = model.sidebar.settings_selected;
    let ascii = model.ascii_icons;

    let mut lines: Vec<Line> = Vec::new();
    for cell in LAYOUT {
        let Some(i) = cell else {
            lines.push(Line::from("").style(Style::new().bg(model.theme.bg_alt)));
            continue;
        };
        let item = SETTINGS_ITEMS[i];
        let selected = i == sel;
        // Reset rows get a ⟲ marker; the toggle's own "[x]" is its indent;
        // plain edit rows are indented level with the text.
        let icon = if is_reset(item) {
            if ascii { "R " } else { "\u{27f2} " }
        } else {
            "  "
        };
        let text_style = if selected {
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
                Span::styled(icon, Style::new().fg(model.theme.accent)),
                Span::styled(label(item, model), text_style),
            ])
            .style(line_style),
        );
    }
    let p = Paragraph::new(lines).style(Style::new().bg(model.theme.bg_alt));
    frame.render_widget(p, area);
}

/// Returns the actionable settings item index under the mouse y, or `None`
/// (outside the body, or on the blank spacer row).
pub fn settings_row_at(area: Rect, y: u16) -> Option<usize> {
    let body = panel_area(area);
    if y < body.y || y >= body.y + body.height {
        return None;
    }
    let visual = (y - body.y) as usize;
    LAYOUT.get(visual).copied().flatten()
}
