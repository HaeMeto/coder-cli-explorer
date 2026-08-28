//! Modal dialog input handling.

use super::*;

/// Handles keyboard input while a dialog is open.
pub(super) fn dialog_key(model: &mut Model, key: KeyEvent) -> Vec<Cmd> {
    let Some(d) = model.dialog.as_mut() else {
        return Vec::new();
    };
    match d.kind {
        DialogKind::Info => match key.code {
            KeyCode::Enter | KeyCode::Esc => dialog_confirm(model),
            _ => Vec::new(),
        },
        DialogKind::Ask => match key.code {
            KeyCode::Left | KeyCode::Right | KeyCode::Tab => {
                d.selected ^= 1;
                Vec::new()
            }
            KeyCode::Char('e') | KeyCode::Char('y') => dialog_confirm(model),
            KeyCode::Char('h') | KeyCode::Char('n') | KeyCode::Esc => dialog_cancel(model),
            KeyCode::Enter => {
                if d.selected == 0 {
                    dialog_confirm(model)
                } else {
                    dialog_cancel(model)
                }
            }
            _ => Vec::new(),
        },
        DialogKind::Input => {
            // The input widget consumes typing and caret motion; only the keys it
            // ignores (Enter/Esc) drive the dialog.
            match d.input.handle_key(key, false) {
                InputOutcome::Ignored => match key.code {
                    KeyCode::Enter => dialog_confirm(model),
                    KeyCode::Esc => dialog_cancel(model),
                    _ => Vec::new(),
                },
                _ => Vec::new(),
            }
        }
    }
}

/// Handles mouse clicks while a dialog is open (buttons).
pub(super) fn dialog_mouse(model: &mut Model, m: MouseEvent) -> Vec<Cmd> {
    if let MouseEventKind::Down(MouseButton::Left) = m.kind {
        let term = full_rect(model);
        if let Some(d) = model.dialog.as_ref() {
            match ui::dialog::hit(d, term, m.column, m.row) {
                Some(true) => return dialog_confirm(model),
                Some(false) => return dialog_cancel(model),
                None => {}
            }
        }
    }
    Vec::new()
}

/// Confirms the dialog: runs the action and closes it.
fn dialog_confirm(model: &mut Model) -> Vec<Cmd> {
    let Some(d) = model.dialog.take() else {
        return Vec::new();
    };
    match d.action {
        DialogAction::GitRevert(rel) => vec![Cmd::GitRevert(rel)],
        DialogAction::NewFile(dir) => create_in(model, dir, d.input.content(), false),
        DialogAction::NewFolder(dir) => create_in(model, dir, d.input.content(), true),
        DialogAction::Rename(path) => rename_to(model, path, d.input.content()),
        DialogAction::Delete(path) => vec![Cmd::DeletePath(path)],
        DialogAction::CloseTab(i, _path) => {
            model.dialog = None;
            close_tab(model, i)
        }
        DialogAction::ResetKeybindings => reset_keybindings(model),
        DialogAction::ResetConfig => reset_config(model),
        DialogAction::None => Vec::new(),
    }
}

/// Validates the typed name and returns the rename Cmd. The entry stays in its
/// own directory, so path separators are rejected here too.
fn rename_to(model: &mut Model, path: PathBuf, name: &str) -> Vec<Cmd> {
    let Some(name) = valid_name(model, name) else {
        return Vec::new();
    };
    let Some(parent) = path.parent() else {
        return Vec::new();
    };
    let to = parent.join(name);
    if to == path {
        return Vec::new(); // unchanged
    }
    vec![Cmd::RenamePath { from: path, to }]
}

/// Accepts a name only if it can name an entry inside one directory and nothing
/// else: no path separators, no `.`/`..` traversal. `None` = rejected.
fn sanitize_name(name: &str) -> Option<String> {
    let name = name.trim();
    if name.is_empty() || name == "." || name == ".." {
        return None;
    }
    if name.contains('/') || name.contains('\\') {
        return None;
    }
    Some(name.to_string())
}

/// `sanitize_name`, reporting a rejected non-empty name on the status bar.
fn valid_name(model: &mut Model, name: &str) -> Option<String> {
    let result = sanitize_name(name);
    if result.is_none() && !name.trim().is_empty() {
        model.notify(format!("Error: invalid name '{}'", name.trim()));
    }
    result
}

/// Validates the typed name and returns the create Cmd.
fn create_in(model: &mut Model, dir: PathBuf, name: &str, is_dir: bool) -> Vec<Cmd> {
    let Some(name) = valid_name(model, name) else {
        return Vec::new();
    };
    // The new entry must be visible once it exists.
    model.sidebar.files.expand(&dir);
    vec![Cmd::CreatePath {
        path: dir.join(&name),
        is_dir,
    }]
}

/// Cancels the dialog (closes it without running the action).
fn dialog_cancel(model: &mut Model) -> Vec<Cmd> {
    model.dialog = None;
    Vec::new()
}

#[cfg(test)]
mod dialog_tests {
    use super::sanitize_name;

    #[test]
    fn names_confined_to_one_directory_are_accepted() {
        assert_eq!(sanitize_name("  main.rs "), Some("main.rs".to_string()));
        assert_eq!(sanitize_name(".gitignore"), Some(".gitignore".to_string()));
    }

    #[test]
    fn separators_and_traversal_are_rejected() {
        for bad in ["", "   ", ".", "..", "a/b", "../etc/passwd", "a\\b"] {
            assert!(sanitize_name(bad).is_none(), "{bad} should be rejected");
        }
    }
}
