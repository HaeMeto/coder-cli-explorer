//! Panel selection, tab lifecycle, and opening files (normal + diff tabs).

use super::*;

/// Selects a sidebar panel from the keyboard (a panel shortcut).
///
/// The shortcut always *goes to* the panel: it opens the sidebar, switches to
/// `p`, and moves focus there — so it works while the editor, terminal or find
/// widget holds focus. Only when the panel is already open **and** focused does
/// a second press collapse the sidebar again.
pub(super) fn select_panel(model: &mut Model, p: Panel) -> Vec<Cmd> {
    let focused_here = matches!(
        model.focus,
        Focus::Sidebar | Focus::SearchInput | Focus::GitCommit
    );
    open_panel(model, p, focused_here)
}

/// Selects a sidebar panel from a click on its activity-bar icon. Clicking the
/// already-active icon collapses the sidebar regardless of where focus sits,
/// which is what the mouse is expected to do.
pub(super) fn toggle_panel(model: &mut Model, p: Panel) -> Vec<Cmd> {
    open_panel(model, p, true)
}

/// Shared body: `collapse_if_active` decides whether re-selecting the panel that
/// is already open closes the sidebar or just focuses it.
fn open_panel(model: &mut Model, p: Panel, collapse_if_active: bool) -> Vec<Cmd> {
    if model.layout.sidebar_open && model.sidebar.active == p && collapse_if_active {
        model.layout.sidebar_open = false;
        if matches!(
            model.focus,
            Focus::Sidebar | Focus::SearchInput | Focus::GitCommit
        ) {
            model.focus = Focus::Editor;
        }
        return Vec::new();
    }

    model.sidebar.active = p;
    model.layout.sidebar_open = true;
    // Search lands in its query input so a search can be typed immediately; the
    // other panels focus their list.
    model.focus = if p == Panel::Search {
        Focus::SearchInput
    } else {
        Focus::Sidebar
    };
    if p == Panel::Search {
        model.sidebar.search.field = SearchField::Query;
        model.sidebar.search.query.cursor_to_end();
    }
    if p == Panel::Git {
        // Focus is `Focus::Sidebar` here, so the zone has to agree: start on the
        // change list, from where Tab reaches the commit box and the buttons.
        model.sidebar.git.zone = crate::app::model::GitZone::Files;
    }
    match p {
        Panel::Git => vec![Cmd::LoadGitStatus],
        Panel::Files if model.sidebar.files.children.is_none() => {
            vec![Cmd::ScanDir(model.root.clone())]
        }
        // Re-probe tool availability each time the panel opens (a binary may have
        // been installed since startup).
        Panel::Extensions => vec![Cmd::CheckTools(model.extensions.tool_commands())],
        _ => Vec::new(),
    }
}

pub(super) fn close_active_tab(model: &mut Model) -> Vec<Cmd> {
    if let Some(i) = model.active_tab {
        if model.tabs[i].buffer.dirty
            && let Some(ref path) = model.tabs[i].buffer.path
        {
            let display = path.display().to_string();
            model.focus = Focus::Editor;
            model.dialog = Some(Dialog::ask(
                "Close tab".to_string(),
                format!("Changes in '{display}' will be lost. Close anyway?"),
                DialogAction::CloseTab(i, display),
            ));
            return Vec::new();
        }
        close_tab(model, i)
    } else {
        Vec::new()
    }
}

pub(super) fn close_tab_with_dirty_check(model: &mut Model, i: usize) -> Vec<Cmd> {
    if i >= model.tabs.len() {
        return Vec::new();
    }
    if model.tabs[i].buffer.dirty
        && let Some(ref path) = model.tabs[i].buffer.path
    {
        let display = path.display().to_string();
        model.focus = Focus::Editor;
        model.dialog = Some(Dialog::ask(
            "Close tab".to_string(),
            format!("Changes in '{display}' will be lost. Close anyway?"),
            DialogAction::CloseTab(i, display),
        ));
        return Vec::new();
    }
    close_tab(model, i)
}

/// Closes the tab at the given index and fixes up the active tab. Returns a
/// `didClose` for the language server when the last tab of the file is closed.
pub(super) fn close_tab(model: &mut Model, i: usize) -> Vec<Cmd> {
    if i >= model.tabs.len() {
        return Vec::new();
    }
    let path = model.tabs[i].buffer.path.clone();
    model.tabs.remove(i);
    match model.active_tab {
        Some(a) if a == i => {
            if model.tabs.is_empty() {
                model.active_tab = None;
            } else {
                model.active_tab = Some(a.min(model.tabs.len() - 1));
            }
        }
        // If the closed tab is before the active one, the index shifts.
        Some(a) if a > i => model.active_tab = Some(a - 1),
        _ => {}
    }
    model.invalidate_highlight();
    // Only notify the server once no tab holds the file anymore.
    match path {
        Some(p) if !model.tabs.iter().any(|t| t.buffer.path.as_deref() == Some(p.as_path())) => {
            super::lsp::did_close(model, &p)
        }
        _ => Vec::new(),
    }
}

pub(super) fn cycle_tab(model: &mut Model, delta: isize) {
    if model.tabs.is_empty() {
        return;
    }
    let n = model.tabs.len() as isize;
    let cur = model.active_tab.unwrap_or(0) as isize;
    let next = (cur + delta).rem_euclid(n) as usize;
    model.active_tab = Some(next);
    model.focus = Focus::Editor;
}

pub(super) fn open_path(model: &mut Model, path: PathBuf) -> Vec<Cmd> {
    open_path_at(model, path, 0)
}

/// Opens the user config file as an editor tab, creating it (with the current
/// defaults) first if it doesn't exist yet so there is always something to edit.
pub(super) fn open_config(model: &mut Model) -> Vec<Cmd> {
    let Some(path) = crate::services::config::config_path() else {
        model.notify("No config path available".to_string());
        return Vec::new();
    };
    if !path.exists() {
        crate::services::config::save(&model.config_snapshot());
    }
    open_path(model, path)
}

/// Opens the keybindings file as an editor tab, seeding it with the current
/// shortcuts first if it doesn't exist yet. Bound to Alt+7 ("Shortcuts").
pub(super) fn open_keybindings(model: &mut Model) -> Vec<Cmd> {
    let Some(path) = crate::services::keybindings::keybindings_path() else {
        model.notify("No config path available".to_string());
        return Vec::new();
    };
    if !path.exists() {
        crate::services::keybindings::save(&model.keybindings);
    }
    open_path(model, path)
}

/// Runs the Settings panel's selected row: reset actions open a confirmation
/// dialog; the edit rows open the file in the editor. Shared by keyboard Enter
/// and mouse click.
pub(super) fn activate_settings(model: &mut Model) -> Vec<Cmd> {
    let Some(&item) = crate::ui::sidebar::SETTINGS_ITEMS.get(model.sidebar.settings_selected) else {
        return Vec::new();
    };
    use crate::ui::sidebar::SettingsItem;
    match item {
        SettingsItem::ResetKeybindings => {
            model.dialog = Some(Dialog::ask(
                "Reset keybindings".to_string(),
                "All shortcuts will be restored to their defaults, discarding your \
                 customizations. Are you sure?"
                    .to_string(),
                DialogAction::ResetKeybindings,
            ));
            Vec::new()
        }
        SettingsItem::ResetConfig => {
            model.dialog = Some(Dialog::ask(
                "Reset config".to_string(),
                "Theme, editor settings and language tooling will be restored to their \
                 defaults, discarding your customizations. Are you sure?"
                    .to_string(),
                DialogAction::ResetConfig,
            ));
            Vec::new()
        }
        SettingsItem::AsciiIcons => {
            model.ascii_icons = !model.ascii_icons;
            persist_config(model)
        }
        SettingsItem::EditKeybindings => open_keybindings(model),
        SettingsItem::EditConfig => open_config(model),
    }
}

/// Restores the built-in keybindings, overwriting `keybindings.toml`, and syncs
/// the open tab (if any) even if it has unsaved edits — reset discards them.
pub(super) fn reset_keybindings(model: &mut Model) -> Vec<Cmd> {
    model.keybindings = crate::services::keybindings::Keybindings::default();
    let Some(path) = crate::services::keybindings::keybindings_path() else {
        model.notify("No config path available".to_string());
        return Vec::new();
    };
    let contents = crate::services::keybindings::to_toml(&model.keybindings);
    force_replace_open_buffer(model, &path, &contents);
    model.notify("Keybindings reset to defaults".to_string());
    vec![Cmd::WriteFile { path, contents }]
}

/// Restores the seeded config (theme, editor settings, starter languages),
/// re-applies it live, overwrites `config.toml`, and syncs the open tab.
pub(super) fn reset_config(model: &mut Model) -> Vec<Cmd> {
    let cfg = crate::services::config::seed();
    model.apply_config(&cfg);
    let Some(path) = crate::services::config::config_path() else {
        model.notify("No config path available".to_string());
        return Vec::new();
    };
    let contents = crate::services::config::to_toml(&cfg);
    force_replace_open_buffer(model, &path, &contents);
    model.notify("Config reset to defaults".to_string());
    vec![
        Cmd::WriteFile { path, contents },
        Cmd::CheckTools(model.extensions.tool_commands()),
    ]
}

/// Replaces the buffer of any open tab for `path` with `text`, unconditionally
/// (unlike `apply_reload`, which preserves unsaved edits). Marks it saved so the
/// subsequent watcher event is a no-op.
fn force_replace_open_buffer(model: &mut Model, path: &std::path::Path, text: &str) {
    let target = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    for i in 0..model.tabs.len() {
        if !buf_path_eq(&model.tabs[i].buffer.path, &target) {
            continue;
        }
        let keep_path = model.tabs[i].buffer.path.clone();
        model.tabs[i].buffer = Buffer::new(keep_path, text);
        model.tabs[i].buffer.mark_saved();
        if model.active_tab == Some(i) {
            model.invalidate_highlight();
        }
    }
}

/// Compares an open buffer's path to a canonicalized disk path.
fn buf_path_eq(p: &Option<PathBuf>, target: &std::path::Path) -> bool {
    match p {
        Some(p) => p.as_path() == target || p.canonicalize().ok().as_deref() == Some(target),
        None => false,
    }
}

/// A watched file changed on disk: reload it only if it is open and has no
/// unsaved edits (a dirty buffer keeps its live text — see `apply_reload`).
pub(super) fn reload_if_clean(model: &mut Model, path: PathBuf) -> Vec<Cmd> {
    let target = path.canonicalize().unwrap_or(path);
    let has_clean = model
        .tabs
        .iter()
        .any(|t| !t.buffer.dirty && buf_path_eq(&t.buffer.path, &target));
    if has_clean {
        vec![Cmd::ReloadFile(target)]
    } else {
        Vec::new()
    }
}

/// Applies fresh on-disk content to every clean tab of a file, preserving the
/// cursor and scroll position. Dirty tabs and no-op reloads (e.g. our own save)
/// are skipped so unsaved work is never clobbered.
pub(super) fn apply_reload(model: &mut Model, path: PathBuf, text: String) -> Vec<Cmd> {
    let target = path.canonicalize().unwrap_or(path);
    let mut reloaded = false;
    for i in 0..model.tabs.len() {
        if !buf_path_eq(&model.tabs[i].buffer.path, &target) {
            continue;
        }
        let tab = &mut model.tabs[i];
        if tab.buffer.dirty || tab.buffer.full_text() == text {
            continue; // live edits present, or nothing actually changed
        }
        let cur = tab.buffer.cursor;
        let (sy, sx) = (tab.buffer.scroll_y, tab.buffer.scroll_x);
        let keep_path = tab.buffer.path.clone();
        tab.buffer = Buffer::new(keep_path, &text);
        // Restore cursor / scroll, clamped to the (possibly shorter) new content.
        let last = tab.buffer.line_count().saturating_sub(1);
        let line = cur.line.min(last);
        let col = cur.col.min(tab.buffer.line_len(line));
        tab.buffer.cursor = Cursor { line, col };
        tab.buffer.scroll_y = sy.min(last);
        tab.buffer.scroll_x = sx;
        tab.buffer.mark_saved();
        reloaded = true;
        if model.active_tab == Some(i) {
            model.invalidate_highlight();
        }
    }
    if reloaded {
        model.notify(format!("Reloaded from disk: {}", target.display()));
        vec![Cmd::LoadHeadText(target)]
    } else {
        Vec::new()
    }
}

/// Opens a file as a diff-mode tab (from the Git panel): reuses an existing diff
/// tab for the path, otherwise loads a fresh one flagged via `pending_diff`.
pub(super) fn open_diff(model: &mut Model, path: PathBuf) -> Vec<Cmd> {
    if let Some(i) = model.diff_tab_index_for(&path) {
        model.active_tab = Some(i);
        model.focus = Focus::Editor;
        ensure_cursor_visible(model);
        return Vec::new();
    }
    model.pending_diff = Some(path.clone());
    vec![Cmd::ReadFile(path)]
}

/// Opens the patch of a history commit as a read-only "<hash> diff" tab: focuses
/// the tab if it is already open, otherwise asks git for the patch (the tab is
/// created when `Msg::CommitDiffLoaded` arrives).
pub(super) fn open_commit_diff(model: &mut Model, hash: String) -> Vec<Cmd> {
    if let Some(i) = model.commit_diff_tab_index(&hash) {
        model.active_tab = Some(i);
        model.focus = Focus::Editor;
        ensure_cursor_visible(model);
        return Vec::new();
    }
    vec![Cmd::LoadCommitDiff(hash)]
}

/// Creates (or refreshes) the read-only diff tab holding a commit's changes.
pub(super) fn show_commit_diff(
    model: &mut Model,
    hash: &str,
    diff: &crate::services::git::CommitDiff,
) {
    let tab = Tab::commit_diff(hash, diff);
    let i = match model.commit_diff_tab_index(hash) {
        Some(i) => {
            model.tabs[i] = tab;
            i
        }
        None => {
            model.tabs.push(tab);
            model.tabs.len() - 1
        }
    };
    model.active_tab = Some(i);
    model.focus = Focus::Editor;
    // The green/red backgrounds come from the parent-vs-commit diff: it has to be
    // computed for the new tab before the first render.
    model.invalidate_highlight();
    model.mark_git_dirty();
    ensure_cursor_visible(model);
}

pub(super) fn open_path_at(model: &mut Model, path: PathBuf, line: usize) -> Vec<Cmd> {
    if let Some(i) = model.tab_index_for(&path) {
        model.active_tab = Some(i);
        model.focus = Focus::Editor;
        if line > 0 {
            model.tabs[i].buffer.goto_line(line);
            // Center the match (opened from a search result / goto).
            center_cursor_in_view(model);
        } else {
            ensure_cursor_visible(model);
        }
        Vec::new()
    } else {
        if line > 0 {
            model.pending_goto = Some((path.clone(), line));
        }
        vec![Cmd::ReadFile(path)]
    }
}

#[cfg(test)]
mod settings_tests {
    use super::*;
    use crate::ui::sidebar::SettingsItem;

    #[test]
    fn ascii_icons_toggle_flips_and_persists() {
        let mut model = Model::new(std::env::temp_dir());
        assert!(!model.ascii_icons, "default is Nerd Font icons");
        let idx = crate::ui::sidebar::SETTINGS_ITEMS
            .iter()
            .position(|i| *i == SettingsItem::AsciiIcons)
            .unwrap();
        model.sidebar.settings_selected = idx;

        let cmds = activate_settings(&mut model);
        assert!(model.ascii_icons, "first activation turns it on");
        assert!(cmds.iter().any(|c| matches!(c, Cmd::SaveConfig(_))));

        let cmds = activate_settings(&mut model);
        assert!(!model.ascii_icons, "second activation turns it back off");
        assert!(cmds.iter().any(|c| matches!(c, Cmd::SaveConfig(_))));
    }
}
