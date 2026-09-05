//! Embedded terminal sizing.

use super::*;

/// Forwards a bracketed paste straight to the PTY as raw bytes, in one write
/// instead of one `Action::PtyInput` per character — the shell/program inside
/// doesn't care about auto-indent the way the code editor does, so no
/// reindenting is needed here, just delivering it atomically.
pub(super) fn paste_into_terminal(model: &mut Model, text: &str) -> Vec<Cmd> {
    if let Some(session) = model.terminal.session.as_mut() {
        session.write(text.as_bytes());
        model.terminal.scroll_to(0);
    }
    Vec::new()
}

/// Propagates the terminal area size to the vt100 parser and the PTY.
pub(super) fn sync_terminal_size(model: &mut Model) {
    if !model.layout.terminal_open {
        return;
    }
    let area = full_rect(model);
    let a = ui::compute_areas(model, area);
    // the terminal area includes the top border (1 row); the rightmost inner
    // column is reserved for the scrollbar.
    let rows = a.terminal.height.saturating_sub(1).max(1);
    let cols = a.terminal.width.saturating_sub(1).max(1);
    model.terminal.resize(rows, cols);
}
