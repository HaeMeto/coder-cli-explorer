//! update(): takes a Msg, updates the Model, returns side-effect Cmds.
//!
//! Split into focused submodules by concern. This file holds the top-level
//! Msg dispatcher plus the shared imports every submodule pulls in via
//! `use super::*`.

use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;

use crate::app::cmd::Cmd;
use crate::app::model::{
    ContextMenu, Dialog, DialogAction, DialogKind, DragTarget, FindField, Focus, GitZone, MenuItem,
    Model, Panel, QuickbarItem, QuickbarState, SearchField, Tab,
};
use crate::app::msg::Msg;
use crate::core::buffer::{Buffer, Cursor};
use crate::core::keymap::{self, Action, Motion};
use crate::core::text_input::{InputOutcome, TextInputState};
use crate::ui;

mod action;
mod dialog;
mod editor;
mod find;
mod git;
mod lsp;
mod menu;
mod mouse;
mod quickbar;
mod search;
mod session;
mod sidebar_nav;
mod tabs;
mod terminal;

use action::apply_action;
use dialog::{dialog_key, dialog_mouse, dialog_paste};
use menu::{menu_key, menu_mouse, open_file_menu};
use quickbar::{files_listed, open_quickbar, quickbar_key, quickbar_mouse, quickbar_paste};
use editor::*;
use find::*;
use git::*;
use mouse::handle_mouse;
use search::*;
use sidebar_nav::*;
use tabs::*;
use terminal::{paste_into_terminal, sync_terminal_size};

/// Resolves `key` against the user's global shortcuts (Quit, Save, panel
/// switches, ...) and applies the action if there is one. Shared fallback for
/// every modal overlay (quickbar, dialog, context menu): a key the overlay
/// itself doesn't recognize falls through here instead of being silently
/// swallowed, so e.g. Ctrl+Q/Ctrl+S still work while one is open.
pub(super) fn overlay_fallback(model: &mut Model, key: KeyEvent) -> Vec<Cmd> {
    // Only the user-bound command table, never `keymap::resolve`'s hardcoded
    // typing/motion fallback — an overlay is open, so a plain letter must not
    // fall through into the editor buffer as text.
    match model.keybindings.resolve(key, Focus::Editor) {
        Some(action) => apply_action(model, action),
        None => Vec::new(),
    }
}

/// Fires debounced work whose deadline has elapsed. Called once per main-loop
/// iteration (not on a message) so autocomplete and `didChange` are throttled
/// without spawning a timer task per keystroke.
/// Loads and rebuilds the workspace session at startup (see `services::session`
/// and `update::session::restore`). Called once from `main::run`, before the
/// event loop starts.
pub fn restore_session(model: &mut Model) -> Vec<Cmd> {
    session::restore(model)
}

pub fn tick(model: &mut Model) -> Vec<Cmd> {
    let (autocomplete, didchange, session_save) = model.take_due_timers(std::time::Instant::now());
    let mut cmds = Vec::new();
    if autocomplete {
        // The completion request flushes the current text on its own, so a pending
        // didChange for the same edit is now redundant — drop it.
        model.cancel_didchange();
        cmds.extend(lsp::request_completion(model));
    }
    if didchange {
        cmds.extend(lsp::flush_didchange(model));
    }
    if session_save {
        cmds.push(Cmd::SaveSession {
            snapshot: model.session_snapshot(),
            seen: model.session_seen_generation.unwrap_or(0),
        });
    }
    cmds
}

pub fn update(model: &mut Model, msg: Msg) -> Vec<Cmd> {
    match msg {
        Msg::Key(key) => {
 // The quickbar (command palette) is the topmost overlay: it captures
 // every key while open.
 if model.quickbar.is_some() {
 return quickbar_key(model, key);
 }
            // If a modal dialog is open it captures all keyboard input.
            if model.dialog.is_some() {
                return dialog_key(model, key);
            }
            // The file-tree context menu captures input the same way.
            if model.context_menu.is_some() {
                return menu_key(model, key);
            }
            // The completion popup (editor sub-mode) gets first refusal on keys.
            if model.completion.is_some()
                && let Some(cmds) = lsp::completion_key(model, key)
            {
                return cmds;
            }
            // A focused text input handles its own editing/motion keys first and
            // reports back what it did; only keys it ignores fall through to the
            // keymap (Enter/Tab/Esc, Ctrl-shortcuts, match/result navigation).
            if let Some((input, multiline)) = focused_input(model) {
                match input.handle_key(key, multiline) {
                    InputOutcome::Ignored => {}
                    InputOutcome::Moved => return Vec::new(),
                    InputOutcome::Changed => {
                        // Live find: re-run matches when the query text changes.
                        if model.focus == Focus::Find && model.find.field == FindField::Query {
                            recompute_find(model);
                        }
                        return Vec::new();
                    }
                }
            }
            if let Some(action) = keymap::resolve(&model.keybindings, key, model.focus, model.leader) {
// Consume the leader latch once a command has fired (it may have just
// been used to unlock a locked command). The Leader key itself re-arms it.
if model.leader && !matches!(action, Action::Leader) {
model.leader = false;
}

                return apply_action(model, action);
            }
            Vec::new()
        }
        Msg::Mouse(m) => {
 if model.quickbar.is_some() {
 return quickbar_mouse(model, m);
 }
            if model.dialog.is_some() {
                return dialog_mouse(model, m);
            }
            if model.context_menu.is_some() {
                return menu_mouse(model, m);
            }
            handle_mouse(model, m)
        }
        // A terminal bracketed paste, routed the same way `Msg::Key` cascades
        // through overlays: whichever one currently owns input gets the text.
        // Falling through to nothing (e.g. `Focus::Sidebar`) is deliberate — it
        // is also what keeps a stray paste from firing single-letter shortcuts
        // (Git panel `a`/`r` stage/revert) one keystroke at a time, which is
        // what happened before bracketed paste existed.
        Msg::Paste(text) => {
            if model.quickbar.is_some() {
                return quickbar_paste(model, &text);
            }
            if model.dialog.is_some() {
                return dialog_paste(model, &text);
            }
            if model.context_menu.is_some() {
                return Vec::new();
            }
            if let Some((input, multiline)) = focused_input(model) {
                input.insert_paste(&text, multiline);
                if model.focus == Focus::Find && model.find.field == FindField::Query {
                    recompute_find(model);
                }
                return Vec::new();
            }
            if model.focus == Focus::Terminal {
                return paste_into_terminal(model, &text);
            }
            paste_into_editor(model, &text)
        }
        Msg::Resize(w, h) => {
            model.term_size = (w, h);
            sync_terminal_size(model);
            Vec::new()
        }
        Msg::Quit => {
            model.should_quit = true;
            Vec::new()
        }
        Msg::Highlighted {
            tab,
            version,
            base,
            lines,
        } => {
            // Only the active tab is highlighted; drop a result for a tab that has
            // since been switched away or closed.
            if model.active_tab == Some(tab) && tab < model.tabs.len() {
                model.set_display_hl(tab, version, base, lines);
            }
            Vec::new()
        }
        Msg::DirScanned { path, entries } => {
            model.sidebar.files.set_children(&path, entries);
            // A rescan after a delete can leave the selection past the last row.
            let len = model.sidebar.files.visible_rows().len();
            if model.sidebar.files.selected >= len {
                model.sidebar.files.selected = len.saturating_sub(1);
            }
            Vec::new()
        }
        Msg::FileLoaded { path, text } => {
            let buffer = Buffer::new(Some(path.clone()), &text);
            let mut tab = Tab::new(buffer);
            // A load requested from the Git panel becomes a diff-mode tab.
            if model.pending_diff.as_deref() == Some(path.as_path()) {
                tab.diff_mode = true;
                model.pending_diff = None;
                // Scroll to the first change once HEAD text arrives (marks need it).
                model.pending_diff_scroll = Some(path.clone());
            }

            // Session restore: cursor/scroll for every restored tab, and for a
            // dirty file that was checkpointed as a diff (large file), rebuild
            // its unsaved content by applying the stored hunks over the disk
            // text just read.
            let restore = model.pending_session_restore.remove(&path);
            if let Some(r) = &restore {
                if let Some(hunks) = &r.dirty_hunks {
                    match crate::services::session::apply_hunks(&text, hunks) {
                        Some(restored) => {
                            tab.buffer = Buffer::new(Some(path.clone()), &restored);
                            tab.buffer.dirty = true;
                        }
                        None => model.notify(format!(
                            "Could not restore unsaved changes for {}: the file changed too much",
                            path.display()
                        )),
                    }
                }
                let last = tab.buffer.line_count().saturating_sub(1);
                let line = r.line.min(last);
                let col = r.col.min(tab.buffer.line_len(line));
                tab.buffer.cursor = Cursor { line, col };
                tab.buffer.scroll_y = r.scroll_y.min(last);
                tab.buffer.scroll_x = r.scroll_x;
            }
            let is_restore = restore.is_some();

            model.tabs.push(tab);
            let idx = model.tabs.len() - 1;

            // While a session restore is choosing which tab should end up
            // focused, only the matching load may claim `active_tab` —
            // otherwise whichever file's async read happens to finish last
            // would win the focus race.
            let restoring_to_other = model
                .session_active_path
                .as_deref()
                .is_some_and(|p| p != path.as_path());
            if !restoring_to_other {
                model.active_tab = Some(idx);
                model.focus = Focus::Editor;
                if model.session_active_path.as_deref() == Some(path.as_path()) {
                    model.session_active_path = None;
                }
                if !is_restore {
                    // Apply a pending goto if there is one (from a search
                    // result): center it. Restored cursor/scroll (above) is
                    // already exact, so this only runs for a fresh open.
                    let goto = model.pending_goto.take().filter(|(gp, _)| *gp == path);
                    if let Some((_, line)) = goto {
                        if let Some(buf) = model.active_buffer_mut() {
                            buf.goto_line(line);
                        }
                        center_cursor_in_view(model);
                    } else {
                        ensure_cursor_visible(model);
                    }
                }
            }
            // Load the HEAD content for the change gutter, and open the document
            // with its language server (if any).
            let mut cmds = vec![Cmd::LoadHeadText(path)];
            cmds.extend(lsp::open_tab(model, idx));
            cmds
        }
        Msg::FileLoadFailed { path, error } => {
            // A session restore was waiting on this file (see `update::session`):
            // release the guard so it doesn't block every load after it from
            // ever claiming `active_tab` again.
            model.pending_session_restore.remove(&path);
            if model.session_active_path.as_deref() == Some(path.as_path()) {
                model.session_active_path = None;
            }
            // Reuse an already-open tab for this file, else open a read-only notice tab.
            if let Some(i) = model.tab_index_for(&path) {
                model.tabs[i].notice = Some(error);
                model.active_tab = Some(i);
            } else {
                model.tabs.push(Tab::notice(path, error));
                model.active_tab = Some(model.tabs.len() - 1);
            }
            model.focus = Focus::Editor;
            Vec::new()
        }
        Msg::CommitDiffLoaded { hash, diff } => {
            show_commit_diff(model, &hash, &diff);
            Vec::new()
        }
        Msg::HeadTextLoaded { path, text } => {
            // Update every open tab for this file (a normal tab and its diff tab).
            for i in model.all_tabs_for(&path) {
                model.tabs[i].head_text = text.clone();
                if model.active_tab == Some(i) {
                    model.invalidate_highlight();
                }
            }
            // First open of a diff tab: jump the cursor to the first changed line so
            // the diff is on screen without scrolling.
            if model.pending_diff_scroll.as_deref() == Some(path.as_path()) {
                model.pending_diff_scroll = None;
                model.refresh_git_marks();
                if let Some(first) = model.active_git_marks.keys().min().copied() {
                    // Leave ~10 lines of context above the first change so it sits
                    // a bit below the top edge rather than flush against it.
                    const DIFF_TOP_MARGIN: usize = 10;
                    if let Some(buf) = model.active_buffer_mut() {
                        buf.goto_line(first);
                        buf.scroll_y = first.saturating_sub(DIFF_TOP_MARGIN);
                        buf.scroll_x = 0;
                    }
                }
            }
            Vec::new()
        }
        Msg::DiskChanged(path) => {
            // A change inside `.git` (external `git commit`/stage/checkout, or the
            // embedded terminal) moves HEAD/index/refs. A change to a working-tree
            // file alters its status too. Refresh the git panel so the branch,
            // ahead/behind counts, buttons and the Changes list stay live — but
            // only while it is open, to avoid running `git status` on every keystroke.
            let in_git = path.components().any(|c| c.as_os_str() == ".git");
            // Live-refresh the file tree: if the changed path sits in a directory
            // the tree has loaded, rescan that directory so new/removed entries
            // appear without reopening the folder. `set_children` merges, so
            // expanded subdirs are preserved. Only watched (already-scanned) dirs
            // fire here, so nothing is walked eagerly.
            let mut cmds = Vec::new();
            if !in_git
                && let Some(parent) = path.parent()
                && (parent == model.sidebar.files.root || model.sidebar.files.is_loaded(parent))
            {
                cmds.push(Cmd::ScanDir(parent.to_path_buf()));
            }
            cmds.extend(reload_if_clean(model, path));
            if in_git || model.sidebar.active == Panel::Git {
                cmds.push(Cmd::LoadGitStatus);
            }
            cmds
        }
        Msg::FileReloaded { path, text } => apply_reload(model, path, text),
        Msg::PathRenamed { from, to } => {
            // Re-point open tabs: a renamed directory moves every file under it.
            let mut cmds = Vec::new();
            for tab in model.tabs.iter_mut() {
                let Some(old) = tab.buffer.path.clone() else {
                    continue;
                };
                let Ok(rest) = old.strip_prefix(&from) else {
                    continue;
                };
                let new = if rest.as_os_str().is_empty() {
                    to.clone()
                } else {
                    to.join(rest)
                };
                tab.buffer.path = Some(new.clone());
                cmds.push(Cmd::LoadHeadText(new));
            }
            model.notify(format!(
                "Renamed: {} -> {}",
                name_of(&from),
                name_of(&to)
            ));
            cmds.push(Cmd::LoadGitStatus);
            cmds
        }
        Msg::PathDeleted(path) => {
            // Close the tabs of deleted files (a whole subtree for a directory).
            let gone: Vec<usize> = model
                .tabs
                .iter()
                .enumerate()
                .filter(|(_, t)| {
                    t.buffer
                        .path
                        .as_ref()
                        .is_some_and(|p| p.starts_with(&path))
                })
                .map(|(i, _)| i)
                .collect();
            let mut cmds: Vec<Cmd> = Vec::new();
            for i in gone.into_iter().rev() {
                cmds.extend(close_tab(model, i));
            }
            model.notify(format!("Deleted '{}'", name_of(&path)));
            cmds.push(Cmd::LoadGitStatus);
            cmds
        }
        Msg::FileSaved { path } => {
            for i in model.all_tabs_for(&path) {
                model.tabs[i].buffer.mark_saved();
            }
            model.notify(format!("Saved: {}", path.display()));
            // Reveal the change gutter now: it was frozen while editing, so a save
            // is when the diff catches up to what is on disk.
            model.mark_git_dirty();
            // Refresh git status, notify the language server, and run a linter.
            let mut cmds = vec![Cmd::LoadGitStatus];
            cmds.extend(lsp::did_save(model, &path));
            cmds.extend(lsp::run_linter(model, &path));
            cmds
        }
        Msg::GitStatusLoaded {
            branch,
            staged,
            unstaged,
            is_repo,
            ahead,
            behind,
            has_upstream,
            has_remote,
            history,
        } => {
            // Close diff-mode tabs for files no longer in the change list (reverted /
            // committed). Their diff is gone, leaving a stale editor view otherwise.
            let stale: Vec<usize> = model
                .tabs
                .iter()
                .enumerate()
                // A commit's diff tab is read-only history, not a live change —
                // it must survive every status refresh.
                .filter(|(_, t)| t.diff_mode && !t.read_only)
                .filter_map(|(i, t)| {
                    let path = t.buffer.path.as_ref()?;
                    let in_list = staged
                        .iter()
                        .chain(unstaged.iter())
                        .any(|e| e.path == path.as_path());
                    if !in_list {
                        Some(i)
                    } else {
                        None
                    }
                })
                .collect();
            let mut cmds: Vec<Cmd> = Vec::new();
            for i in stale.into_iter().rev() {
                cmds.extend(close_tab(model, i));
            }
            let g = &mut model.sidebar.git;
            g.branch = branch;
            g.staged = staged;
            g.unstaged = unstaged;
            g.is_repo = is_repo;
            g.ahead = ahead;
            g.behind = behind;
            g.has_upstream = has_upstream;
            g.has_remote = has_remote;
            g.history = history;
            let len = g.nav_len();
            if g.selected >= len {
                g.selected = len.saturating_sub(1);
            }
            // Refresh the change gutter for open files (HEAD may have moved after a commit/revert).
            cmds.extend(
                model
                    .tabs
                    .iter()
                    // Generated tabs (a commit patch) have no file behind their
                    // synthetic path — there is no HEAD text to load.
                    .filter(|t| !t.read_only)
                    .filter_map(|t| t.buffer.path.clone())
                    .map(Cmd::LoadHeadText),
            );
            cmds
        }
        Msg::GitCommitUndone { message } => {
            model.sidebar.git.commit.set_content(message); // moves caret to end
            // Land in the commit box so the restored message can be edited.
            set_git_zone(model, GitZone::Message);
            Vec::new()
        }
        Msg::SearchResults { query, matches } => {
            if query == model.sidebar.search.query.content() {
                model.sidebar.search.results = matches;
                model.sidebar.search.selected = 0;
                model.notify(format!(
                    "{} results found",
                    model.sidebar.search.results.len()
                ));
            }
            Vec::new()
        }
 Msg::FilesListed { paths } => files_listed(model, paths),
        Msg::ReplaceDone { changed, count } => {
            // Reload buffers that are open and changed on disk.
            for path in &changed {
                if let Ok(text) = std::fs::read_to_string(path) {
                    for i in model.all_tabs_for(path) {
                        model.tabs[i].buffer = Buffer::new(Some(path.clone()), &text);
                    }
                }
            }
            model.invalidate_highlight();
            model.notify(format!("{} changes, {} files", count, changed.len()));
            // Refresh the results and git status.
            let mut cmds = vec![Cmd::LoadGitStatus];
            let s = &model.sidebar.search;
            if !s.query.is_empty() {
                cmds.push(Cmd::RunSearch {
                    query: s.query.content().to_string(),
                    use_regex: s.use_regex,
                    match_case: s.match_case,
                    search_hidden: s.search_hidden,
                });
            }
            cmds
        }
        Msg::PtyReady(session) => {
            model.terminal.session = Some(session);
            model.terminal.spawn_requested = false;
            sync_terminal_size(model);
            Vec::new()
        }
        Msg::PtyOutput(bytes) => {
            model.terminal.parser.process(&bytes);
            // Refresh scrollback bounds and re-anchor the view after vt100 has
            // appended any new rows.
            model.terminal.sync_scroll_bounds();
            Vec::new()
        }
        Msg::PtyExited => {
            model.terminal.session = None;
            model.terminal.spawn_requested = false;
            model.notify("Terminal closed".to_string());
            Vec::new()
        }
        Msg::LspSessionReady { language, handle } => {
            model.lsp.starting.remove(&language);
            model.lsp.sessions.insert(language, handle);
            Vec::new()
        }
        Msg::LspInitialized { language } => lsp::on_initialized(model, &language),
        Msg::LspDiagnostics { path, diagnostics } => {
            lsp::store_diagnostics(model, path, diagnostics);
            Vec::new()
        }
        Msg::LspCompletions { token, items } => lsp::completions_arrived(model, token, items),
        Msg::LspFormatEdits { token, edits } => lsp::format_edits_arrived(model, token, edits),
        Msg::LspExited { language } => {
            lsp::remove_server(model, &language);
            model.notify(format!("Language server '{language}' stopped"));
            Vec::new()
        }
        Msg::LspError { language, message } => {
            lsp::remove_server(model, &language);
            model.notify(format!("LSP ({language}): {message}"));
            Vec::new()
        }
        Msg::FormatterOutput {
            path,
            text,
            token,
            save_after,
        } => lsp::formatter_output(model, path, text, token, save_after),
        Msg::LinterDiagnostics { path, items } => lsp::linter_diagnostics(model, path, items),
        Msg::ToolsChecked(statuses) => {
            for (command, installed) in statuses {
                model.tool_available.insert(command, installed);
            }
            Vec::new()
        }
        Msg::Error(e) => {
            model.notify(format!("Error: {e}"));
            Vec::new()
        }
        Msg::Toast(s) => {
            model.show_toast(s);
            Vec::new()
        }
        Msg::ToastExpired => {
            // Only clear if actually expired: a newer toast raised in the meantime
            // has a later `shown_at` and must survive this stale timer.
            if model.toast.as_ref().is_some_and(|t| t.is_expired()) {
                model.toast = None;
            }
            Vec::new()
        }
        Msg::SessionSaved(outcome) => {
            use crate::services::session::SaveOutcome;
            match outcome {
                SaveOutcome::Saved(new_gen) => {
                    model.session_seen_generation = Some(new_gen);
                    Vec::new()
                }
                SaveOutcome::Conflict(disk_gen) => {
                    // Another coder instance wrote a newer checkpoint since we last
                    // checked: adopt its generation as our new baseline (so we don't
                    // re-report the same conflict every checkpoint) and skip this
                    // write rather than clobber it — see `services::session::save`'s
                    // multi-instance note.
                    model.session_seen_generation = Some(disk_gen);
                    model.notify(
                        "Session updated by another coder window; this checkpoint was skipped".to_string(),
                    );
                    Vec::new()
                }
                SaveOutcome::NoPath => Vec::new(),
            }
        }
    }
}

/// The file name of a path, for status messages.
fn name_of(path: &std::path::Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

/// The text input the current focus routes keys to, and whether it is multi-line.
/// `None` when focus is not on a text field.
fn focused_input(model: &mut Model) -> Option<(&mut TextInputState, bool)> {
    match model.focus {
        Focus::Find => {
            let f = match model.find.field {
                FindField::Query => &mut model.find.query,
                FindField::Replace => &mut model.find.replace,
            };
            Some((f, false))
        }
        Focus::SearchInput => {
            let s = match model.sidebar.search.field {
                SearchField::Query => &mut model.sidebar.search.query,
                SearchField::Replace => &mut model.sidebar.search.replace,
            };
            Some((s, false))
        }
        Focus::GitCommit => Some((&mut model.sidebar.git.commit, true)),
        _ => None,
    }
}
