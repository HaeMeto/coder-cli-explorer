//! Embedded terminal sizing.

use super::*;

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
