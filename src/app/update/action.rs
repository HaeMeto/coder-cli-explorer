//! apply_action(): maps a resolved keymap Action onto the Model.

use super::*;

/// Runs a file-tree action on the selected row. A no-op unless the Files panel
/// is the active one — the shortcuts belong to the tree, not the other panels.
fn on_selected_row(model: &mut Model, f: impl Fn(&mut Model, usize) -> Vec<Cmd>) -> Vec<Cmd> {
    if model.sidebar.active != Panel::Files {
        return Vec::new();
    }
    let idx = model.sidebar.files.selected;
    f(model, idx)
}

pub(super) fn apply_action(model: &mut Model, action: Action) -> Vec<Cmd> {
    match action {
        Action::Quit => {
            let dirty = model.tabs.iter().filter(|t| t.buffer.dirty).count();
            if dirty > 0 {
                let plural = if dirty == 1 { "file has" } else { "files have" };
                model.dialog = Some(Dialog::ask_save(
                    "Unsaved changes".to_string(),
                    format!("{dirty} {plural} unsaved changes. Save before quitting?"),
                    DialogAction::QuitPrompt,
                ));
            } else {
                model.should_quit = true;
            }
            Vec::new()
        }
 Action::Leader => {
 // Enter leader/unlock mode: the next locked command chord fires
 // directly instead of falling through to typing/motion.
 model.leader = true;
 Vec::new()
 }
        Action::ToggleSidebar => {
            model.layout.sidebar_open = !model.layout.sidebar_open;
            if !model.layout.sidebar_open
                && matches!(model.focus, Focus::Sidebar | Focus::SearchInput)
            {
                model.focus = Focus::Editor;
            } else if model.layout.sidebar_open {
                model.focus = if model.sidebar.active == Panel::Search {
                    Focus::SearchInput
                } else {
                    Focus::Sidebar
                };
            }
            Vec::new()
        }
        Action::ToggleTerminal => {
            model.layout.terminal_open = !model.layout.terminal_open;
            if model.layout.terminal_open {
                model.focus = Focus::Terminal;
                sync_terminal_size(model);
                if model.terminal.session.is_none() && !model.terminal.spawn_requested {
                    model.terminal.spawn_requested = true;
                    return vec![Cmd::SpawnPty {
                        rows: model.terminal.rows,
                        cols: model.terminal.cols,
                    }];
                }
            } else if model.focus == Focus::Terminal {
                model.focus = Focus::Editor;
            }
            Vec::new()
        }
        Action::SelectPanel(p) => select_panel(model, p),
        Action::ShowShortcuts => open_keybindings(model),
        Action::Save => {
            // Read-only tabs (binary / unreadable notices, commit patches) must
            // never be written back.
            if model.active_notice().is_some() || model.active_read_only() {
                return Vec::new();
            }
            // Cheap whitespace formatting runs synchronously first.
            apply_format_on_save(model);
            let Some(path) = model.active_buffer().and_then(|b| b.path.clone()) else {
                // No backing file yet (an untitled scratch buffer): ask where
                // to save it instead of silently doing nothing.
                let Some(i) = model.active_tab else {
                    return Vec::new();
                };
                model.dialog = Some(Dialog::input(
                    "Save As".to_string(),
                    "Path (relative to the workspace root, or absolute):".to_string(),
                    String::new(),
                    DialogAction::SaveAs(i),
                ));
                return Vec::new();
            };
            // When format-on-save is on and the language has a formatter (LSP or
            // tool), format asynchronously and defer the write until edits apply.
            if model.sidebar.settings.format_on_save {
                let fmt = super::lsp::request_format(model, true);
                if !fmt.is_empty() {
                    return fmt;
                }
            }
            let contents = model.active_buffer().map(|b| b.full_text()).unwrap_or_default();
            // Saving the config file re-applies it live (theme, settings, language
            // tooling) so hand-edits take effect without a restart, and re-probes
            // the (possibly changed) tool binaries.
            if crate::services::config::config_path().as_deref() == Some(path.as_path()) {
                let cfg = crate::services::config::parse(&contents);
                model.apply_config(&cfg);
                return vec![
                    Cmd::WriteFile { path, contents },
                    Cmd::CheckTools(model.extensions.tool_commands()),
                ];
            }
            // Saving the keybindings file re-applies the shortcuts live.
            if crate::services::keybindings::keybindings_path().as_deref() == Some(path.as_path()) {
                model.keybindings = crate::services::keybindings::parse(&contents);
            }
            vec![Cmd::WriteFile { path, contents }]
        }
        Action::CloseTab => close_active_tab(model),
        Action::NextTab => {
            cycle_tab(model, 1);
            Vec::new()
        }
        Action::PrevTab => {
            // Shift+Tab dedents a multi-line selection; otherwise it switches tabs.
            if model.focus == Focus::Editor
                && model.active_buffer().is_some_and(|b| b.selection_is_multiline())
            {
                mutate(model, |b| b.dedent_selection())
            } else if in_git_panel(model) {
                // In the Git panel Shift+Tab walks the zones backwards, mirroring Tab.
                git_cycle_zone(model, -1)
            } else {
                cycle_tab(model, -1);
                Vec::new()
            }
        }

        // ----- Editor -----
        Action::Insert(c) => {
            let cmds = mutate(model, |b| b.insert_char(c));
            // Auto-trigger completions while typing an identifier or after '.',
            // debounced ~400ms so a fast typist does not hit the server per key.
            if c.is_alphanumeric() || c == '_' || c == '.' {
                model.schedule_autocomplete();
            }
            cmds
        }
        Action::Newline => mutate(model, |b| b.insert_newline()),
        // Tab indents a multi-line selection; otherwise it inserts a soft tab.
        Action::InsertTab => {
            if model.focus == Focus::Editor
                && model.active_buffer().is_some_and(|b| b.selection_is_multiline())
            {
                mutate(model, |b| b.indent_selection())
            } else {
                mutate(model, |b| b.insert_str("    "))
            }
        }
        Action::Backspace => {
            let cmds = mutate(model, |b| b.backspace());
            // Keep an open popup fresh as the prefix shrinks (debounced).
            if model.completion.is_some() {
                model.schedule_autocomplete();
            }
            cmds
        }
        Action::Delete => mutate(model, |b| b.delete_forward()),
        Action::TriggerCompletion => super::lsp::request_completion(model),
        Action::Format => super::lsp::request_format(model, false),
        Action::SelectAll => edit(model, |b| b.select_all()),
        Action::Undo => mutate(model, |b| b.undo()),
        Action::Redo => mutate(model, |b| b.redo()),
        Action::Move(motion, extend) => {
            let (h, _) = editor_viewport(model);
            edit(model, |b| apply_motion(b, motion, extend, h))
        }
        Action::MoveLineUp => mutate(model, |b| b.move_lines(-1)),
        Action::MoveLineDown => mutate(model, |b| b.move_lines(1)),
        Action::Copy => {
            if let Some(buf) = model.active_buffer()
                && let Some(sel) = buf.selected_text() {
                    model.internal_clipboard = sel.clone();
                    let toast = model.show_toast("Copied to clipboard");
                    return vec![Cmd::SetClipboard(sel), toast];
                }
            Vec::new()
        }
        // Cut deletes, so on a read-only tab it degrades to a plain copy.
        Action::Cut if model.active_read_only() => apply_action(model, Action::Copy),
        Action::Cut => {
            if let Some(buf) = model.active_buffer_mut()
                && let Some(sel) = buf.selected_text() {
                    buf.delete_selection();
                    model.internal_clipboard = sel.clone();
                    ensure_cursor_visible(model);
                    let mut cmds = vec![Cmd::SetClipboard(sel)];
                    cmds.extend(super::lsp::notify_change(model));
                    cmds.push(model.show_toast("Cut to clipboard"));
                    return cmds;
                }
            Vec::new()
        }
 Action::Paste => {
 let text = read_clipboard(model);
 paste_into_editor(model, &text)
 }

        // ----- Sidebar navigation -----
        // In the Git panel the arrows drive the change list, so they stay put
        // while a button holds the focus (the mouse wheel still scrolls it).
        Action::NavUp | Action::NavDown
            if in_git_panel(model) && model.sidebar.git.zone != GitZone::Files =>
        {
            Vec::new()
        }
        Action::NavUp => {
            nav(model, -1);
            post_nav_persist(model)
        }
        Action::NavDown => {
            nav(model, 1);
            post_nav_persist(model)
        }
        // In the Git panel Enter presses whichever button holds the focus; on the
        // change list (and every other panel) it opens the selected row.
        Action::Activate if in_git_panel(model) => git_zone_activate(model),
        Action::Activate => activate_selection(model),

        // ----- Git panel (Source Control) -----
        Action::GitCycleZone(delta) => git_cycle_zone(model, delta),
        Action::GitToggleStage => git_toggle_stage(model),
        Action::GitRevertEntry => git_revert_entry(model),

        // ----- File tree entry management (Files panel only) -----
        Action::NewFile => on_selected_row(model, |m, i| new_entry_dialog(m, i, false)),
        Action::NewFolder => on_selected_row(model, |m, i| new_entry_dialog(m, i, true)),
        Action::NewUntitledFile => new_untitled_tab(model),
        Action::RenameEntry => on_selected_row(model, rename_dialog),
        Action::DeleteEntry => on_selected_row(model, delete_dialog),

        // ----- Search (typing handled by the focused input widget) -----
        Action::SearchToggleField => {
            let s = &mut model.sidebar.search;
            // Tab into the replace field only when it is actually shown — with
            // replace mode off there is nothing to switch to, so this is a
            // no-op rather than routing focus to a field that isn't drawn.
            if s.replace_mode {
                s.field = match s.field {
                    SearchField::Query => SearchField::Replace,
                    SearchField::Replace => SearchField::Query,
                };
            }
            Vec::new()
        }
        Action::SearchToggleRegex => {
            model.sidebar.search.use_regex = !model.sidebar.search.use_regex;
            rerun_search(model)
        }
        Action::SearchSubmit => {
            let s = &model.sidebar.search;
            let query = s.query.content().to_string();
            let (use_regex, match_case, search_hidden) =
                (s.use_regex, s.match_case, s.search_hidden);
            if query.is_empty() {
                return Vec::new();
            }
            match s.field {
                SearchField::Query => {
                    model.focus = Focus::Sidebar;
                    vec![Cmd::RunSearch {
                        query,
                        use_regex,
                        match_case,
                        search_hidden,
                    }]
                }
                // Enter in the Replace field -> replace across all files.
                SearchField::Replace => {
                    let replace = s.replace.content().to_string();
                    model.notify("Replacing...".to_string());
                    vec![Cmd::RunReplace {
                        query,
                        replace,
                        use_regex,
                        match_case,
                        search_hidden,
                    }]
                }
            }
        }

        // ----- In-editor find / replace (typing handled by the input widget) -----
        Action::OpenFind => open_find(model, false),
        Action::OpenFindReplace => open_find(model, true),
 Action::OpenQuickbar => open_quickbar(model),
        Action::FindNext => {
            find_step(model, 1);
            Vec::new()
        }
        Action::FindPrev => {
            find_step(model, -1);
            Vec::new()
        }
        Action::FindToggleField => {
            if model.find.replace_mode {
                model.find.field = match model.find.field {
                    FindField::Query => FindField::Replace,
                    FindField::Replace => FindField::Query,
                };
            }
            Vec::new()
        }
        Action::PtyInput(bytes) => {
            if let Some(session) = model.terminal.session.as_mut() {
                session.write(&bytes);
                // Typing snaps the view back to the live bottom.
                model.terminal.scroll_to(0);
            }
            Vec::new()
        }
        Action::Escape => {
            match model.focus {
                Focus::Find => close_find(model),
                Focus::Editor => {
                    if let Some(buf) = model.active_buffer_mut() {
                        buf.clear_selection();
                    }
                }
                Focus::Sidebar if model.sidebar.active == Panel::Search => {
                    model.focus = Focus::SearchInput;
                }
                Focus::SearchInput => model.focus = Focus::Editor,
                // Esc leaves the commit box for the change list, the panel's
                // resting zone.
                Focus::GitCommit => set_git_zone(model, GitZone::Files),
                Focus::Terminal => {
                    model.terminal.selection = None;
                }
                _ => {}
            }
            Vec::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::model::SearchField;

    #[test]
    fn search_tab_does_not_focus_the_hidden_replace_field() {
        // With replace mode off (the default), the replace row isn't drawn at
        // all — Tab must stay on Query rather than routing focus to a field
        // the user can't see.
        let mut model = Model::new(std::env::temp_dir());
        assert!(!model.sidebar.search.replace_mode);
        apply_action(&mut model, Action::SearchToggleField);
        assert_eq!(model.sidebar.search.field, SearchField::Query);
    }

    #[test]
    fn search_tab_cycles_fields_once_replace_mode_is_on() {
        let mut model = Model::new(std::env::temp_dir());
        model.sidebar.search.replace_mode = true;
        apply_action(&mut model, Action::SearchToggleField);
        assert_eq!(model.sidebar.search.field, SearchField::Replace);
        apply_action(&mut model, Action::SearchToggleField);
        assert_eq!(model.sidebar.search.field, SearchField::Query);
    }
}
