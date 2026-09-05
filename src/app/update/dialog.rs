//! Modal dialog input handling.

use super::*;

/// Handles keyboard input while a dialog is open. Everything the dialog itself
/// doesn't recognize falls through to `overlay_fallback` (Ctrl+Q/Ctrl+S, panel
/// switches, ...) — reading `d.kind`/`d.selected` through short-lived reborrows
/// rather than holding one across the fallback call, since that needs `model`
/// free.
pub(super) fn dialog_key(model: &mut Model, key: KeyEvent) -> Vec<Cmd> {
    let Some(kind) = model.dialog.as_ref().map(|d| d.kind) else {
        return Vec::new();
    };
    match kind {
        DialogKind::Info => match key.code {
            KeyCode::Enter | KeyCode::Esc => dialog_confirm(model),
            _ => overlay_fallback(model, key),
        },
        DialogKind::Ask => match key.code {
            KeyCode::Left | KeyCode::Right | KeyCode::Tab => {
                if let Some(d) = model.dialog.as_mut() {
                    d.selected ^= 1;
                }
                Vec::new()
            }
            KeyCode::Char('e') | KeyCode::Char('y') => dialog_confirm(model),
            KeyCode::Char('h') | KeyCode::Char('n') | KeyCode::Esc => dialog_cancel(model),
            KeyCode::Enter => {
                let selected = model.dialog.as_ref().map(|d| d.selected).unwrap_or(0);
                if selected == 0 {
                    dialog_confirm(model)
                } else {
                    dialog_cancel(model)
                }
            }
            _ => overlay_fallback(model, key),
        },
        DialogKind::Input => {
            // The input widget consumes typing and caret motion; only the keys it
            // ignores (Enter/Esc/global shortcuts) drive the dialog.
            let Some(d) = model.dialog.as_mut() else {
                return Vec::new();
            };
            let outcome = d.input.handle_key(key, false);
            match outcome {
                InputOutcome::Ignored => match key.code {
                    KeyCode::Enter => dialog_confirm(model),
                    KeyCode::Esc => dialog_cancel(model),
                    _ => overlay_fallback(model, key),
                },
                _ => Vec::new(),
            }
        }
        DialogKind::AskSave => match key.code {
            KeyCode::Left | KeyCode::BackTab => {
                cycle_selected(model, -1);
                Vec::new()
            }
            KeyCode::Right | KeyCode::Tab => {
                cycle_selected(model, 1);
                Vec::new()
            }
            KeyCode::Char('s') => dialog_button(model, kind, 0),
            KeyCode::Char('d') | KeyCode::Char('n') => dialog_button(model, kind, 1),
            KeyCode::Char('c') | KeyCode::Esc => dialog_button(model, kind, 2),
            KeyCode::Enter => {
                let selected = model.dialog.as_ref().map(|d| d.selected).unwrap_or(0);
                dialog_button(model, kind, selected)
            }
            _ => overlay_fallback(model, key),
        },
    }
}

/// Moves `AskSave`'s selection by `delta`, wrapping across its 3 buttons.
fn cycle_selected(model: &mut Model, delta: isize) {
    if let Some(d) = model.dialog.as_mut() {
        d.selected = (d.selected as isize + delta).rem_euclid(3) as usize;
    }
}

/// Activates dialog button `idx` (0/1/2, see `ui::dialog::hit`), shared by
/// keyboard and mouse. For `AskSave`, index 2 ("Cancel") just closes the
/// dialog; 0/1 select and confirm (branching on `Dialog.selected` inside
/// `dialog_confirm`). Every other kind keeps its plain confirm/cancel split.
fn dialog_button(model: &mut Model, kind: DialogKind, idx: usize) -> Vec<Cmd> {
    if kind == DialogKind::AskSave {
        if idx == 2 {
            return dialog_cancel(model);
        }
        if let Some(d) = model.dialog.as_mut() {
            d.selected = idx;
        }
        return dialog_confirm(model);
    }
    if idx == 0 {
        dialog_confirm(model)
    } else {
        dialog_cancel(model)
    }
}

/// Pastes into the dialog's text field, if it has one (the `Input` kind only —
/// `Ask`/`Info` dialogs have nothing to paste into).
pub(super) fn dialog_paste(model: &mut Model, text: &str) -> Vec<Cmd> {
    let Some(d) = model.dialog.as_mut() else {
        return Vec::new();
    };
    if d.kind == DialogKind::Input {
        d.input.insert_paste(text, false);
    }
    Vec::new()
}

/// Handles mouse clicks while a dialog is open (buttons).
pub(super) fn dialog_mouse(model: &mut Model, m: MouseEvent) -> Vec<Cmd> {
    if let MouseEventKind::Down(MouseButton::Left) = m.kind {
        let term = full_rect(model);
        let hit = model.dialog.as_ref().and_then(|d| ui::dialog::hit(d, term, m.column, m.row));
        if let Some(idx) = hit {
            let kind = model.dialog.as_ref().map(|d| d.kind).unwrap_or(DialogKind::Ask);
            return dialog_button(model, kind, idx);
        }
    }
    Vec::new()
}

/// Confirms the dialog: runs the action and closes it.
fn dialog_confirm(model: &mut Model) -> Vec<Cmd> {
    let Some(d) = model.dialog.take() else {
        return Vec::new();
    };
    let selected = d.selected;
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
DialogAction::OpenWorkspace => {
 let root = PathBuf::from(d.input.content().trim());
 model.open_folder(root.clone());
 vec![Cmd::ScanDir(root), Cmd::LoadGitStatus]
}
        // `selected` is 0 ("Save") or 1 ("Don't Save") — index 2 ("Cancel")
        // never reaches here, `dialog_button` closes the dialog for it directly.
        DialogAction::QuitPrompt => {
            if selected == 0 {
                save_all_and_quit(model)
            } else {
                discard_and_quit(model)
            }
        }
        DialogAction::SaveAs(tab) => save_as(model, tab, d.input.content()),
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
    use super::dialog_key;
    use crate::app::model::{Dialog, DialogAction, Model};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn key(c: char, ctrl: bool) -> KeyEvent {
        let m = if ctrl { KeyModifiers::CONTROL } else { KeyModifiers::NONE };
        KeyEvent::new(KeyCode::Char(c), m)
    }

    #[test]
    fn global_shortcut_still_fires_while_a_dialog_is_open() {
        // Regression test: a dialog (New File/Rename/Delete confirm/...) must not
        // swallow Ctrl+Q — it should fall through to the global keybinding just
        // like the quickbar already does.
        let mut model = Model::new(std::env::temp_dir());
        model.dialog = Some(Dialog::ask(
            "Delete?".to_string(),
            "Are you sure?".to_string(),
            DialogAction::None,
        ));
        dialog_key(&mut model, key('q', true));
        assert!(model.should_quit, "Ctrl+Q must quit even with a dialog open");
    }

    #[test]
    fn dialogs_own_keys_are_not_overridden_by_the_fallback() {
        // 'y' (confirm) must still be handled by the Ask dialog itself, not
        // treated as an unbound key that falls through and does nothing.
        let mut model = Model::new(std::env::temp_dir());
        model.dialog = Some(Dialog::ask(
            "Delete?".to_string(),
            "Are you sure?".to_string(),
            DialogAction::None,
        ));
        dialog_key(&mut model, key('y', false));
        assert!(model.dialog.is_none(), "'y' should confirm and close the dialog");
    }

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

    /// `name` must be unique per test: these run concurrently in the same
    /// process, so two tests sharing a directory could race each other's
    /// fixture setup/cleanup (create/write/remove on the very same path).
    fn dirty_model_with_one_tab(name: &str) -> (Model, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("coder-ask-save-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("f.txt");
        std::fs::write(&path, "old").unwrap();
        let mut model = Model::new(dir);
        model.tabs.push(crate::app::model::Tab::new(crate::core::buffer::Buffer::new(
            Some(path.clone()),
            "old",
        )));
        model.tabs[0].buffer.insert_str("new");
        (model, path)
    }

    #[test]
    fn quit_with_dirty_tabs_opens_ask_save_instead_of_quitting() {
        use crate::app::update::action::apply_action;
        use crate::core::keymap::Action;
        let (mut model, _path) = dirty_model_with_one_tab("opens");
        apply_action(&mut model, Action::Quit);
        assert!(!model.should_quit, "must ask before quitting with unsaved changes");
        assert!(matches!(model.dialog.as_ref().map(|d| d.kind), Some(crate::app::model::DialogKind::AskSave)));
    }

    #[test]
    fn ask_save_enter_saves_and_quits_by_default() {
        use crate::app::update::action::apply_action;
        use crate::core::keymap::Action;
        let (mut model, path) = dirty_model_with_one_tab("save");
        apply_action(&mut model, Action::Quit);
        // "Save" is the default selection (index 0).
        dialog_key(&mut model, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(model.should_quit);
        assert!(!model.tabs[0].buffer.dirty);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn ask_save_dont_save_quits_without_writing() {
        use crate::app::update::action::apply_action;
        use crate::core::keymap::Action;
        let (mut model, path) = dirty_model_with_one_tab("dont_save");
        apply_action(&mut model, Action::Quit);
        dialog_key(&mut model, key('d', false));
        assert!(model.should_quit);
        assert!(model.tabs[0].buffer.dirty, "not written back, but nothing is lost either");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "old", "the real file is untouched");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn ask_save_cancel_just_closes_the_dialog() {
        use crate::app::update::action::apply_action;
        use crate::core::keymap::Action;
        let (mut model, path) = dirty_model_with_one_tab("cancel");
        apply_action(&mut model, Action::Quit);
        dialog_key(&mut model, key('c', false));
        assert!(model.dialog.is_none());
        assert!(!model.should_quit, "Cancel aborts the quit entirely");
        let _ = std::fs::remove_file(&path);
    }
}
