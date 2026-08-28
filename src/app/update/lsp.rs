//! LSP-related update helpers: opening documents, debounced changes, and
//! turning server results (raw LSP positions) into Model state (char columns).

use std::path::Path;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::*;
use crate::app::model::{CompletionState, Diagnostic, PendingFormat};
use crate::services::lsp::{self, LspClientMsg, Severity};

/// Resolves the LSP language id for a tab's file, if it declares an `lsp` server.
fn tab_lsp_language(model: &Model, tab: usize) -> Option<String> {
    let path = model.tabs.get(tab)?.buffer.path.as_ref()?;
    let lang = model.extensions.language_for_path(path)?;
    lang.lsp.as_ref()?;
    Some(lang.id.clone())
}

/// Builds a `didOpen` for a tab whose server is running.
fn did_open_cmd(model: &Model, tab: usize) -> Option<Cmd> {
    let lang_id = tab_lsp_language(model, tab)?;
    let handle = model.lsp.sessions.get(&lang_id)?;
    let buf = &model.tabs[tab].buffer;
    let path = buf.path.as_ref()?;
    Some(Cmd::LspSend {
        to_server: handle.to_server.clone(),
        msg: LspClientMsg::DidOpen {
            uri: lsp::path_to_uri(path),
            language_id: lang_id,
            version: buf.version as i32,
            text: buf.full_text(),
        },
    })
}

/// Ensures a server is running for a just-opened tab and opens the document.
/// Idempotent: dedupes spawns via `lsp.starting`; a not-yet-initialized server
/// gets the `didOpen` later from `on_initialized` (avoids a double didOpen).
pub(super) fn open_tab(model: &mut Model, tab: usize) -> Vec<Cmd> {
    let Some(path) = model.tabs.get(tab).and_then(|t| t.buffer.path.clone()) else {
        return Vec::new();
    };
    let Some(lang) = model.extensions.language_for_path(&path) else {
        return Vec::new();
    };
    let Some(spec) = lang.lsp.clone() else {
        return Vec::new();
    };
    let lang_id = lang.id.clone();

    if model.lsp.initialized.contains(&lang_id) {
        return did_open_cmd(model, tab).into_iter().collect();
    }
    if model.lsp.sessions.contains_key(&lang_id) || model.lsp.starting.contains(&lang_id) {
        return Vec::new(); // spawn in flight; on_initialized will open it
    }
    model.lsp.starting.insert(lang_id.clone());
    vec![Cmd::LspEnsureStarted {
        language: lang_id,
        spec,
        root: model.root.clone(),
    }]
}

/// A server finished initializing: open every already-loaded tab of its language.
pub(super) fn on_initialized(model: &mut Model, language: &str) -> Vec<Cmd> {
    model.lsp.initialized.insert(language.to_string());
    let mut cmds = Vec::new();
    for i in 0..model.tabs.len() {
        if tab_lsp_language(model, i).as_deref() == Some(language) {
            cmds.extend(did_open_cmd(model, i));
        }
    }
    cmds
}

/// Marks a debounced `didChange` due ~1s from now (reset on every edit). The main
/// loop flushes it via `flush_didchange` once the deadline passes — no timer task.
pub(super) fn notify_change(model: &mut Model) -> Vec<Cmd> {
    // Only arm the timer when the active buffer actually has a running server.
    let Some(i) = model.active_tab else {
        return Vec::new();
    };
    let Some(lang_id) = tab_lsp_language(model, i) else {
        return Vec::new();
    };
    if !model.lsp.initialized.contains(&lang_id) || model.tabs[i].buffer.path.is_none() {
        return Vec::new();
    }
    model.schedule_didchange();
    Vec::new()
}

/// Sends the active buffer's current text to its language server as a `didChange`.
/// Called by the main loop when the debounced deadline set in `notify_change`
/// elapses.
pub(super) fn flush_didchange(model: &Model) -> Vec<Cmd> {
    let Some(i) = model.active_tab else {
        return Vec::new();
    };
    let Some(lang_id) = tab_lsp_language(model, i) else {
        return Vec::new();
    };
    let Some(handle) = model.lsp.sessions.get(&lang_id) else {
        return Vec::new();
    };
    let Some(path) = model.tabs[i].buffer.path.as_ref() else {
        return Vec::new();
    };
    vec![Cmd::LspSend {
        to_server: handle.to_server.clone(),
        msg: LspClientMsg::DidChange {
            uri: lsp::path_to_uri(path),
            version: model.tabs[i].buffer.version as i32,
            text: model.tabs[i].buffer.full_text(),
        },
    }]
}

/// Notifies the server that a file was saved.
pub(super) fn did_save(model: &Model, path: &Path) -> Vec<Cmd> {
    let Some(lang) = model.extensions.language_for_path(path) else {
        return Vec::new();
    };
    let Some(lang_id) = lang.lsp.as_ref().map(|_| lang.id.clone()) else {
        return Vec::new();
    };
    let Some(handle) = model.lsp.sessions.get(&lang_id) else {
        return Vec::new();
    };
    vec![Cmd::LspSend {
        to_server: handle.to_server.clone(),
        msg: LspClientMsg::DidSave {
            uri: lsp::path_to_uri(path),
        },
    }]
}

/// Notifies the server that the last tab for a file closed.
pub(super) fn did_close(model: &Model, path: &Path) -> Vec<Cmd> {
    let Some(lang) = model.extensions.language_for_path(path) else {
        return Vec::new();
    };
    let Some(lang_id) = lang.lsp.as_ref().map(|_| lang.id.clone()) else {
        return Vec::new();
    };
    let Some(handle) = model.lsp.sessions.get(&lang_id) else {
        return Vec::new();
    };
    vec![Cmd::LspSend {
        to_server: handle.to_server.clone(),
        msg: LspClientMsg::DidClose {
            uri: lsp::path_to_uri(path),
        },
    }]
}

/// Stores diagnostics for a file, converting raw LSP UTF-16 positions to buffer
/// char columns. Only kept for files that are open (we need the rope to convert).
pub(super) fn store_diagnostics(
    model: &mut Model,
    path: std::path::PathBuf,
    raw: Vec<lsp::RawDiagnostic>,
) {
    let Some(i) = model.all_tabs_for(&path).into_iter().next() else {
        return;
    };
    let buf = &model.tabs[i].buffer;
    let converted: Vec<Diagnostic> = raw
        .into_iter()
        .map(|d| {
            let col_start = buf.utf16_to_char_col(d.start_line, d.start_char);
            // Underline to the end of the start line for multi-line diagnostics.
            let col_end = if d.end_line == d.start_line {
                buf.utf16_to_char_col(d.start_line, d.end_char)
            } else {
                buf.line_len(d.start_line)
            };
            Diagnostic {
                line: d.start_line,
                col_start,
                col_end: col_end.max(col_start + 1),
                severity: d.severity,
                message: d.message,
            }
        })
        .collect();
    if converted.is_empty() {
        model.diagnostics.remove(&path);
    } else {
        model.diagnostics.insert(path, converted);
    }
}

/// Requests completions at the cursor. First flushes the current text so the
/// server completes against exactly what the user sees, then asks. Guarded by a
/// `(tab, version)` token so a late response can be discarded.
pub(super) fn request_completion(model: &Model) -> Vec<Cmd> {
    let Some(i) = model.active_tab else {
        return Vec::new();
    };
    let Some(lang_id) = tab_lsp_language(model, i) else {
        return Vec::new();
    };
    if !model.lsp.initialized.contains(&lang_id) {
        return Vec::new();
    }
    let Some(handle) = model.lsp.sessions.get(&lang_id) else {
        return Vec::new();
    };
    let buf = &model.tabs[i].buffer;
    let Some(path) = buf.path.as_ref() else {
        return Vec::new();
    };
    let uri = lsp::path_to_uri(path);
    let line = buf.cursor.line as u32;
    let character = buf.char_col_to_utf16(buf.cursor.line, buf.cursor.col);
    let version = buf.version;
    vec![
        Cmd::LspSend {
            to_server: handle.to_server.clone(),
            msg: LspClientMsg::DidChange {
                uri: uri.clone(),
                version: version as i32,
                text: buf.full_text(),
            },
        },
        Cmd::LspSend {
            to_server: handle.to_server.clone(),
            msg: LspClientMsg::Completion {
                uri,
                line,
                character,
                token: (i, version),
            },
        },
    ]
}

/// Start of the identifier prefix before the cursor (the range a completion
/// replaces on accept).
fn word_start(buf: &crate::core::buffer::Buffer) -> Cursor {
    let line = buf.cursor.line;
    let chars: Vec<char> = buf.line_text(line).chars().collect();
    let mut s = buf.cursor.col.min(chars.len());
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    while s > 0 && is_word(chars[s - 1]) {
        s -= 1;
    }
    Cursor { line, col: s }
}

/// How well an item matches the typed prefix — lower is better, `None` rejects.
/// Mirrors the usual editor tiers: exact-case prefix, case-insensitive prefix,
/// substring, then subsequence (so "erm" still finds "enable_raw_mode").
fn match_score(filter_text: &str, prefix: &str) -> Option<u8> {
    if prefix.is_empty() {
        return Some(0);
    }
    if filter_text.starts_with(prefix) {
        return Some(0);
    }
    let haystack = filter_text.to_lowercase();
    let needle = prefix.to_lowercase();
    if haystack.starts_with(&needle) {
        return Some(1);
    }
    if haystack.contains(&needle) {
        return Some(2);
    }
    // Subsequence: every needle char appears in order.
    let mut chars = haystack.chars();
    if needle.chars().all(|c| chars.any(|h| h == c)) {
        return Some(3);
    }
    None
}

/// Keeps only the items matching the typed prefix, best tier first. Servers
/// (rust-analyzer especially) return every candidate at the position and leave
/// the filtering to the client.
fn filter_items(items: Vec<lsp::CompletionItem>, prefix: &str) -> Vec<lsp::CompletionItem> {
    let mut scored: Vec<(u8, lsp::CompletionItem)> = items
        .into_iter()
        .filter_map(|it| match_score(&it.filter_text, prefix).map(|s| (s, it)))
        .collect();
    // Stable within a tier: fall back to the server's own ranking.
    scored.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.sort_text.cmp(&b.1.sort_text)));
    scored.into_iter().map(|(_, it)| it).collect()
}

/// The identifier text between `word_start` and the cursor.
fn typed_prefix(buf: &crate::core::buffer::Buffer) -> String {
    let anchor = word_start(buf);
    buf.line_text(buf.cursor.line)
        .chars()
        .skip(anchor.col)
        .take(buf.cursor.col.saturating_sub(anchor.col))
        .collect()
}

/// A completion response arrived: open the popup only if it still matches the
/// current tab and buffer version (otherwise the user moved on — discard it).
pub(super) fn completions_arrived(
    model: &mut Model,
    token: lsp::Token,
    items: Vec<lsp::CompletionItem>,
) -> Vec<Cmd> {
    let (tab, version) = token;
    if model.active_tab != Some(tab) || model.tabs.get(tab).map(|t| t.buffer.version) != Some(version)
    {
        return Vec::new(); // superseded by newer typing / a tab switch
    }
    let buf = &model.tabs[tab].buffer;
    let items = filter_items(items, &typed_prefix(buf));
    if items.is_empty() {
        model.completion = None;
        return Vec::new();
    }
    let anchor = word_start(buf);
    model.completion = Some(CompletionState {
        items,
        selected: 0,
        anchor,
        tab_index: tab,
    });
    Vec::new()
}

/// Handles a key while the completion popup is open. Returns `Some(cmds)` when it
/// consumes the key, or `None` to let normal editing handle it (which may refresh
/// the popup). Non-editing keys dismiss the popup and fall through.
pub(super) fn completion_key(model: &mut Model, key: KeyEvent) -> Option<Vec<Cmd>> {
    model.completion.as_ref()?;
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Up => {
            let c = model.completion.as_mut().unwrap();
            c.selected = c.selected.saturating_sub(1);
            Some(Vec::new())
        }
        KeyCode::Down => {
            let c = model.completion.as_mut().unwrap();
            if c.selected + 1 < c.items.len() {
                c.selected += 1;
            }
            Some(Vec::new())
        }
        KeyCode::Esc => {
            model.completion = None;
            model.cancel_autocomplete(); // don't let a pending debounce reopen it
            Some(Vec::new())
        }
        KeyCode::Enter | KeyCode::Tab => Some(accept_completion(model)),
        // Identifier chars / '.' and Backspace keep typing; the edit re-requests.
        KeyCode::Char(c) if !ctrl && (c.is_alphanumeric() || c == '_' || c == '.') => None,
        KeyCode::Backspace => None,
        // Anything else (arrows, etc.): dismiss and let the key act normally.
        _ => {
            model.completion = None;
            model.cancel_autocomplete();
            None
        }
    }
}

/// Inserts the selected completion, replacing the typed prefix.
fn accept_completion(model: &mut Model) -> Vec<Cmd> {
    let Some(comp) = model.completion.take() else {
        return Vec::new();
    };
    // Guard against a tab switch between request and accept.
    if model.active_tab != Some(comp.tab_index) {
        return Vec::new();
    }
    let Some(item) = comp.items.get(comp.selected).cloned() else {
        return Vec::new();
    };
    let before = model.active_buffer().map(|b| b.version);
    if let Some(buf) = model.active_buffer_mut() {
        // Select the prefix [anchor, cursor] so insert_str replaces it.
        buf.anchor = Some(comp.anchor);
        buf.insert_str(&item.insert_text);
        buf.clear_selection();
    }
    if before != model.active_buffer().map(|b| b.version) {
        notify_change(model)
    } else {
        Vec::new()
    }
}

/// Requests formatting of the active buffer: the LSP server if one is running,
/// else a standalone formatter tool. `save_after` writes the file once edits
/// apply (format-on-save). Returns empty when the language has no formatter, so
/// the caller can fall back to a plain save.
pub(super) fn request_format(model: &mut Model, save_after: bool) -> Vec<Cmd> {
    let Some(i) = model.active_tab else {
        return Vec::new();
    };
    let Some(path) = model.tabs[i].buffer.path.clone() else {
        return Vec::new();
    };
    let (has_lsp, lang_id, formatter) = match model.extensions.language_for_path(&path) {
        Some(l) => (l.lsp.is_some(), l.id.clone(), l.formatter.clone()),
        None => return Vec::new(),
    };
    let version = model.tabs[i].buffer.version;
    let token = (i, version);

    if has_lsp
        && model.lsp.initialized.contains(&lang_id)
        && let Some(handle) = model.lsp.sessions.get(&lang_id)
    {
        let to_server = handle.to_server.clone();
        model.pending_format = Some(PendingFormat { save_after });
        return vec![Cmd::LspSend {
            to_server,
            msg: LspClientMsg::Formatting {
                uri: lsp::path_to_uri(&path),
                token,
            },
        }];
    }
    if let Some(spec) = formatter {
        let text = model.tabs[i].buffer.full_text();
        // The tool path carries save_after through the Msg; clear any stale LSP one.
        model.pending_format = None;
        return vec![Cmd::RunFormatterTool {
            path,
            spec,
            text,
            token,
            save_after,
        }];
    }
    Vec::new()
}

/// Applies LSP `TextEdit`s to a buffer as one undo step. Edits are converted to
/// char ranges and applied from the end so earlier offsets stay valid.
fn apply_text_edits(buf: &mut Buffer, edits: &[lsp::RawTextEdit]) {
    let mut chars: Vec<char> = buf.full_text().chars().collect();
    let n = chars.len();
    let mut ranges: Vec<(usize, usize, &str)> = edits
        .iter()
        .map(|e| {
            let s = buf.lsp_pos_to_char(e.start_line, e.start_char).min(n);
            let en = buf.lsp_pos_to_char(e.end_line, e.end_char).min(n);
            (s.min(en), s.max(en), e.new_text.as_str())
        })
        .collect();
    ranges.sort_by_key(|r| std::cmp::Reverse(r.0));
    for (s, en, t) in ranges {
        chars.splice(s..en, t.chars());
    }
    let new_text: String = chars.into_iter().collect();
    buf.replace_all(&new_text);
}

/// Writes the (freshly formatted) file when a format-on-save is pending.
fn finish_format_save(model: &Model, tab: usize, save_after: bool) -> Vec<Cmd> {
    if !save_after {
        return Vec::new();
    }
    let Some(t) = model.tabs.get(tab) else {
        return Vec::new();
    };
    let Some(path) = t.buffer.path.clone() else {
        return Vec::new();
    };
    vec![Cmd::WriteFile {
        path,
        contents: t.buffer.full_text(),
    }]
}

/// LSP formatting edits arrived: apply them (unless the buffer moved on) and,
/// if this was a format-on-save, write the file.
pub(super) fn format_edits_arrived(
    model: &mut Model,
    token: lsp::Token,
    edits: Vec<lsp::RawTextEdit>,
) -> Vec<Cmd> {
    let (tab, version) = token;
    let save_after = model
        .pending_format
        .take()
        .map(|p| p.save_after)
        .unwrap_or(false);
    let fresh = model.tabs.get(tab).map(|t| t.buffer.version) == Some(version);
    if fresh && !edits.is_empty() {
        apply_text_edits(&mut model.tabs[tab].buffer, &edits);
        if model.active_tab == Some(tab) {
            model.invalidate_highlight();
        }
    }
    let mut cmds = if fresh && model.active_tab == Some(tab) {
        notify_change(model)
    } else {
        Vec::new()
    };
    cmds.extend(finish_format_save(model, tab, save_after));
    cmds
}

/// A standalone formatter returned new text: apply it (unless stale) and, for a
/// format-on-save, write the file.
pub(super) fn formatter_output(
    model: &mut Model,
    _path: std::path::PathBuf,
    text: String,
    token: lsp::Token,
    save_after: bool,
) -> Vec<Cmd> {
    model.pending_format = None;
    let (tab, version) = token;
    let fresh = model.tabs.get(tab).map(|t| t.buffer.version) == Some(version);
    if fresh && model.tabs[tab].buffer.full_text() != text {
        model.tabs[tab].buffer.replace_all(&text);
        if model.active_tab == Some(tab) {
            model.invalidate_highlight();
        }
    }
    let mut cmds = if fresh && model.active_tab == Some(tab) {
        notify_change(model)
    } else {
        Vec::new()
    };
    cmds.extend(finish_format_save(model, tab, save_after));
    cmds
}

/// Runs the standalone linter for a saved file, unless an LSP server already
/// supplies diagnostics for the language.
pub(super) fn run_linter(model: &Model, path: &Path) -> Vec<Cmd> {
    let Some(lang) = model.extensions.language_for_path(path) else {
        return Vec::new();
    };
    if lang.lsp.is_some() && model.lsp.initialized.contains(&lang.id) {
        return Vec::new();
    }
    let Some(spec) = lang.linter.clone() else {
        return Vec::new();
    };
    let Some(i) = model.all_tabs_for(path).into_iter().next() else {
        return Vec::new();
    };
    vec![Cmd::RunLinterTool {
        path: path.to_path_buf(),
        spec,
        text: model.tabs[i].buffer.full_text(),
    }]
}

/// Stores linter diagnostics (given in 0-based line/col) for an open file.
pub(super) fn linter_diagnostics(
    model: &mut Model,
    path: std::path::PathBuf,
    items: Vec<(usize, usize, String)>,
) -> Vec<Cmd> {
    let Some(i) = model.all_tabs_for(&path).into_iter().next() else {
        model.diagnostics.remove(&path);
        return Vec::new();
    };
    let buf = &model.tabs[i].buffer;
    let diags: Vec<Diagnostic> = items
        .into_iter()
        .map(|(line, col, message)| Diagnostic {
            line,
            col_start: col,
            col_end: buf.line_len(line).max(col + 1),
            severity: Severity::Warning,
            message,
        })
        .collect();
    if diags.is_empty() {
        model.diagnostics.remove(&path);
    } else {
        model.diagnostics.insert(path, diags);
    }
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::lsp::RawTextEdit;

    fn edit(sl: usize, sc: u32, el: usize, ec: u32, t: &str) -> RawTextEdit {
        RawTextEdit {
            start_line: sl,
            start_char: sc,
            end_line: el,
            end_char: ec,
            new_text: t.to_string(),
        }
    }

    #[test]
    fn applies_single_edit() {
        let mut b = Buffer::new(None, "ab");
        apply_text_edits(&mut b, &[edit(0, 0, 0, 2, "cd")]);
        assert_eq!(b.full_text(), "cd");
    }

    #[test]
    fn applies_multiple_edits_end_first() {
        // Two non-overlapping edits on one line; applied from the end so offsets hold.
        let mut b = Buffer::new(None, "let x=1");
        // Replace "=" (col5..6) with " = ", and "x" (col4..5) with "y".
        apply_text_edits(
            &mut b,
            &[edit(0, 5, 0, 6, " = "), edit(0, 4, 0, 5, "y")],
        );
        assert_eq!(b.full_text(), "let y = 1");
    }

    fn item(label: &str, sort: &str) -> lsp::CompletionItem {
        lsp::CompletionItem {
            label: label.to_string(),
            insert_text: label.to_string(),
            detail: None,
            filter_text: label.to_string(),
            sort_text: sort.to_string(),
        }
    }

    #[test]
    fn filters_unrelated_items_out() {
        // What rust-analyzer actually returns at an expression position.
        let items = vec![
            item("self::", "a"),
            item("crate::", "b"),
            item("EnableMouseCapture", "c"),
            item("enable_raw_mode", "d"),
        ];
        let kept = filter_items(items, "enab");
        assert_eq!(
            kept.iter().map(|i| &i.label[..]).collect::<Vec<_>>(),
            ["enable_raw_mode", "EnableMouseCapture"],
            "prefix matches first (case-insensitive), non-matches dropped"
        );
    }

    #[test]
    fn empty_prefix_keeps_everything() {
        // Right after a '.', there is no prefix — the server's list stands.
        let items = vec![item("len", "b"), item("iter", "a")];
        let kept = filter_items(items, "");
        assert_eq!(kept.len(), 2);
        assert_eq!(kept[0].label, "iter", "ties fall back to the server's sortText");
    }

    #[test]
    fn subsequence_matches_last() {
        let items = vec![item("enable_raw_mode", "a"), item("erase", "b")];
        let kept = filter_items(items, "erm");
        // "erase" has no 'm'; only the subsequence hit survives.
        assert_eq!(kept.iter().map(|i| &i.label[..]).collect::<Vec<_>>(), ["enable_raw_mode"]);
    }

    #[test]
    fn edit_range_respects_utf16() {
        // "a😀b": the emoji spans UTF-16 units 1..3; replacing it yields "aXb".
        let mut b = Buffer::new(None, "a😀b");
        apply_text_edits(&mut b, &[edit(0, 1, 0, 3, "X")]);
        assert_eq!(b.full_text(), "aXb");
    }
}

/// Removes a server and clears its languages' diagnostics.
pub(super) fn remove_server(model: &mut Model, language: &str) {
    model.lsp.sessions.remove(language);
    model.lsp.starting.remove(language);
    model.lsp.initialized.remove(language);
    // Drop diagnostics for files of this language.
    let paths: Vec<std::path::PathBuf> = model
        .diagnostics
        .keys()
        .filter(|p| {
            model
                .extensions
                .language_for_path(p)
                .map(|l| l.id == language)
                .unwrap_or(false)
        })
        .cloned()
        .collect();
    for p in paths {
        model.diagnostics.remove(&p);
    }
}
