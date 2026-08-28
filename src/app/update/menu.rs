//! File-tree context menu: opening, keyboard/mouse handling, item dispatch.

use super::*;

/// Opens the context menu for the tree row at `idx`, anchored at the click.
pub(super) fn open_file_menu(model: &mut Model, idx: usize, x: u16, y: u16) -> Vec<Cmd> {
    model.focus = Focus::Sidebar;
    model.sidebar.files.selected = idx;
    // Anchor one cell right/below the click, like a desktop context menu.
    model.context_menu = Some(ContextMenu::new(idx, x + 1, y + 1));
    Vec::new()
}

/// Handles keyboard input while the menu is open. It captures every key.
pub(super) fn menu_key(model: &mut Model, key: KeyEvent) -> Vec<Cmd> {
    let Some(m) = model.context_menu.as_mut() else {
        return Vec::new();
    };
    let last = MenuItem::ALL.len() - 1;
    match key.code {
        KeyCode::Up => {
            m.selected = if m.selected == 0 { last } else { m.selected - 1 };
            Vec::new()
        }
        KeyCode::Down => {
            m.selected = if m.selected == last { 0 } else { m.selected + 1 };
            Vec::new()
        }
        KeyCode::Home => {
            m.selected = 0;
            Vec::new()
        }
        KeyCode::End => {
            m.selected = last;
            Vec::new()
        }
        KeyCode::Enter => {
            let item = MenuItem::ALL[m.selected];
            run_item(model, item)
        }
        KeyCode::Esc => {
            model.context_menu = None;
            Vec::new()
        }
        _ => Vec::new(),
    }
}

/// Handles mouse input while the menu is open: a click on an item runs it, a
/// click anywhere else just closes the menu.
pub(super) fn menu_mouse(model: &mut Model, m: MouseEvent) -> Vec<Cmd> {
    let click = matches!(
        m.kind,
        MouseEventKind::Down(MouseButton::Left) | MouseEventKind::Down(MouseButton::Right)
    );
    if !click {
        return Vec::new();
    }
    let term = full_rect(model);
    let hit = model
        .context_menu
        .as_ref()
        .and_then(|menu| ui::context_menu::hit(menu, term, m.column, m.row));
    match hit {
        Some(item) => run_item(model, item),
        None => {
            model.context_menu = None;
            Vec::new()
        }
    }
}

/// Closes the menu and opens the dialog for the chosen item.
fn run_item(model: &mut Model, item: MenuItem) -> Vec<Cmd> {
    let Some(menu) = model.context_menu.take() else {
        return Vec::new();
    };
    let row = menu.row;
    match item {
        MenuItem::NewFile => new_entry_dialog(model, row, false),
        MenuItem::NewFolder => new_entry_dialog(model, row, true),
        MenuItem::Rename => rename_dialog(model, row),
        MenuItem::Delete => delete_dialog(model, row),
    }
}
