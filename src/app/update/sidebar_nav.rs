//! Sidebar list navigation, activation, and config persistence.

use super::*;

/// Moves the selection in the active sidebar list.
pub(super) fn nav(model: &mut Model, delta: isize) {
    match model.sidebar.active {
        Panel::Files => {
            let len = model.sidebar.files.visible_rows().len();
            model.sidebar.files.selected = move_index(model.sidebar.files.selected, delta, len);
        }
        Panel::Git => {
            let len = model.sidebar.git.nav_len();
            model.sidebar.git.selected = move_index(model.sidebar.git.selected, delta, len);
        }
        Panel::Search => {
            let len = model.sidebar.search.results.len();
            model.sidebar.search.selected = move_index(model.sidebar.search.selected, delta, len);
        }
        Panel::Themes => {
            let len = model.sidebar.themes.names.len();
            let sel = move_index(model.sidebar.themes.selected, delta, len);
            model.apply_theme(sel); // live theme change with the arrow keys
        }
        Panel::Settings => {
            let len = ui::sidebar::SETTINGS_ITEMS.len();
            model.sidebar.settings_selected =
                move_index(model.sidebar.settings_selected, delta, len);
        }
        // Extensions has no sidebar list to navigate.
        Panel::Extensions => {}
    }
}

fn move_index(cur: usize, delta: isize, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    let v = (cur as isize + delta).clamp(0, len as isize - 1);
    v as usize
}

/// Activates the selected item via Enter/Right or a click.
pub(super) fn activate_selection(model: &mut Model) -> Vec<Cmd> {
    match model.sidebar.active {
        Panel::Files => {
            let rows = model.sidebar.files.visible_rows();
            let Some(row) = rows.get(model.sidebar.files.selected) else {
                return Vec::new();
            };
            let path = row.path.clone();
            if row.is_dir {
                if model.sidebar.files.is_expanded(&path) {
                    model.sidebar.files.collapse(&path);
                    Vec::new()
                } else if model.sidebar.files.is_loaded(&path) {
                    model.sidebar.files.expand(&path);
                    Vec::new()
                } else {
                    vec![Cmd::ScanDir(path)]
                }
            } else {
                open_path(model, path)
            }
        }
        Panel::Git => {
            // Clicking a git entry opens the file as a diff-mode tab; changed lines
            // get a green/red background.
            let sel = model.sidebar.git.selected;
            if let Some((entry, _)) = model.sidebar.git.entry_at(sel) {
                let path = entry.path.clone();
                open_diff(model, path)
            } else if let Some(commit) = model.sidebar.git.commit_at(sel) {
                // A history row opens the commit's patch in a read-only tab.
                open_commit_diff(model, commit.hash.clone())
            } else {
                Vec::new()
            }
        }
        Panel::Search => {
            let m = model.sidebar.search.results.get(model.sidebar.search.selected);
            if let Some(m) = m {
                let path = m.path.clone();
                let line = m.line_no.saturating_sub(1);
                open_path_at(model, path, line)
            } else {
                Vec::new()
            }
        }
        Panel::Themes => {
            model.apply_theme(model.sidebar.themes.selected);
            persist_config(model)
        }
        Panel::Settings => activate_settings(model),
        // Extensions has no activatable rows.
        Panel::Extensions => Vec::new(),
    }
}

/// Opens the name-input dialog for "new file"/"new folder" on the tree row at
/// `idx`: inside it when the row is a directory, next to it when it is a file.
pub(super) fn new_entry_dialog(model: &mut Model, idx: usize, is_dir: bool) -> Vec<Cmd> {
    let rows = model.sidebar.files.visible_rows();
    let Some(row) = rows.get(idx) else {
        return Vec::new();
    };
    let (dir, in_name) = if row.is_dir {
        (row.path.clone(), row.name.clone())
    } else {
        let parent = row.path.parent().unwrap_or(&model.sidebar.files.root).to_path_buf();
        let name = dir_label(model, &parent);
        (parent, name)
    };
    model.focus = Focus::Sidebar;
    model.sidebar.files.selected = idx;
    let kind = if is_dir { "folder" } else { "file" };
    let (title, action) = if is_dir {
        ("New Folder", DialogAction::NewFolder(dir))
    } else {
        ("New File", DialogAction::NewFile(dir))
    };
    model.dialog = Some(Dialog::input(
        title.to_string(),
        format!("Name of the new {kind} in '{in_name}':"),
        String::new(),
        action,
    ));
    Vec::new()
}

/// Opens the "new file"/"new folder" dialog for the workspace root, from the
/// Files panel header buttons.
pub(super) fn new_root_entry_dialog(model: &mut Model, is_dir: bool) -> Vec<Cmd> {
    let dir = model.sidebar.files.root.clone();
    model.focus = Focus::Sidebar;
    let kind = if is_dir { "folder" } else { "file" };
    let (title, action) = if is_dir {
        ("New Folder", DialogAction::NewFolder(dir))
    } else {
        ("New File", DialogAction::NewFile(dir))
    };
    model.dialog = Some(Dialog::input(
        title.to_string(),
        format!("Name of the new {kind} in the workspace root:"),
        String::new(),
        action,
    ));
    Vec::new()
}

/// Opens the rename dialog for the tree row at `idx`, pre-filled with its name.
pub(super) fn rename_dialog(model: &mut Model, idx: usize) -> Vec<Cmd> {
    let rows = model.sidebar.files.visible_rows();
    let Some(row) = rows.get(idx) else {
        return Vec::new();
    };
    let (path, name) = (row.path.clone(), row.name.clone());
    model.focus = Focus::Sidebar;
    model.sidebar.files.selected = idx;
    model.dialog = Some(Dialog::input(
        "Rename".to_string(),
        format!("New name for '{name}':"),
        name,
        DialogAction::Rename(path),
    ));
    Vec::new()
}

/// Opens the delete confirmation for the tree row at `idx`.
pub(super) fn delete_dialog(model: &mut Model, idx: usize) -> Vec<Cmd> {
    let rows = model.sidebar.files.visible_rows();
    let Some(row) = rows.get(idx) else {
        return Vec::new();
    };
    let (path, name, is_dir) = (row.path.clone(), row.name.clone(), row.is_dir);
    model.focus = Focus::Sidebar;
    model.sidebar.files.selected = idx;
    let message = if is_dir {
        format!("Folder '{name}' and everything in it will be deleted. This cannot be undone. Are you sure?")
    } else {
        format!("File '{name}' will be deleted. This cannot be undone. Are you sure?")
    };
    model.dialog = Some(Dialog::ask(
        "Delete".to_string(),
        message,
        DialogAction::Delete(path),
    ));
    Vec::new()
}

/// The display name of a directory: its file name, or "workspace" for the root.
fn dir_label(model: &Model, dir: &std::path::Path) -> String {
    if dir == model.sidebar.files.root {
        return "workspace".to_string();
    }
    dir.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| dir.to_string_lossy().into_owned())
}

/// A Cmd that writes the current preferences (theme + settings) to disk.
pub(super) fn persist_config(model: &Model) -> Vec<Cmd> {
    vec![Cmd::SaveConfig(model.config_snapshot())]
}

/// Persists after a keyboard nav that changes the theme live (Themes panel only).
pub(super) fn post_nav_persist(model: &Model) -> Vec<Cmd> {
    if model.sidebar.active == Panel::Themes {
        persist_config(model)
    } else {
        Vec::new()
    }
}
