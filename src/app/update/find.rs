//! In-editor find / replace widget logic.

use super::*;

/// Opens (or refocuses) the find widget. `replace` also shows the replace row.
pub(super) fn open_find(model: &mut Model, replace: bool) -> Vec<Cmd> {
    if model.active_tab.is_none() {
        return Vec::new();
    }
    model.find.open = true;
    if replace {
        model.find.replace_mode = true;
    }
    model.find.field = FindField::Query;
    // Prefill the query from a single-line selection (VSCode behavior).
    if let Some(sel) = model.active_buffer().and_then(|b| b.selected_text())
        && !sel.is_empty()
        && !sel.contains('\n')
    {
        model.find.query.set_content(sel);
    }
    // Place the caret at the end of each field's current text.
    model.find.query.cursor_to_end();
    model.find.replace.cursor_to_end();
    model.focus = Focus::Find;
    recompute_find(model);
    Vec::new()
}

/// Closes the find widget and returns focus to the editor.
pub(super) fn close_find(model: &mut Model) {
    model.find.open = false;
    model.focus = Focus::Editor;
}

/// Recomputes match positions for the current query and selects the match at or
/// after the cursor. Called whenever the query or the buffer changes.
pub(super) fn recompute_find(model: &mut Model) {
    let matches = model
        .active_buffer()
        .map(|b| find_matches(&b.full_text(), model.find.query.content()))
        .unwrap_or_default();
    model.find.matches = matches;
    if model.find.matches.is_empty() {
        model.find.current = None;
        if let Some(buf) = model.active_buffer_mut() {
            buf.clear_selection();
        }
        return;
    }
    let cur = model.active_buffer().map(|b| b.cursor_char_index()).unwrap_or(0);
    let idx = model
        .find
        .matches
        .iter()
        .position(|(s, _)| *s >= cur)
        .unwrap_or(0);
    model.find.current = Some(idx);
    find_select_current(model);
}

/// Selects the current match in the buffer and scrolls it into view.
fn find_select_current(model: &mut Model) {
    let Some(i) = model.find.current else {
        return;
    };
    let Some(&(s, e)) = model.find.matches.get(i) else {
        return;
    };
    if let Some(buf) = model.active_buffer_mut() {
        buf.select_char_range(s, e);
    }
    // Center the found match in the viewport.
    center_cursor_in_view(model);
}

/// Moves to the next (delta=1) / previous (delta=-1) match, wrapping around.
pub(super) fn find_step(model: &mut Model, delta: isize) {
    let n = model.find.matches.len();
    if n == 0 {
        return;
    }
    let cur = model.find.current.unwrap_or(0) as isize;
    model.find.current = Some((cur + delta).rem_euclid(n as isize) as usize);
    find_select_current(model);
}

/// Replaces the current match with the replacement text, then advances.
pub(super) fn find_replace_one(model: &mut Model) -> Vec<Cmd> {
    if model.active_read_only() {
        return Vec::new(); // generated content (a commit patch) is never rewritten
    }
    let Some(i) = model.find.current else {
        return Vec::new();
    };
    let Some(&(s, e)) = model.find.matches.get(i) else {
        return Vec::new();
    };
    let rep = model.find.replace.content().to_string();
    if let Some(buf) = model.active_buffer_mut() {
        buf.select_char_range(s, e);
        buf.insert_str(&rep);
    }
    model.invalidate_highlight();
    // The cursor now sits just past the replacement; recompute selects the next match.
    recompute_find(model);
    Vec::new()
}

/// Replaces every match in the active buffer in a single undo step.
pub(super) fn find_replace_all(model: &mut Model) -> Vec<Cmd> {
    if model.active_read_only() {
        return Vec::new(); // generated content (a commit patch) is never rewritten
    }
    if model.find.query.is_empty() {
        return Vec::new();
    }
    let rep = model.find.replace.content().to_string();
    let Some((new_text, count)) = model
        .active_buffer()
        .map(|b| replace_all_text(&b.full_text(), model.find.query.content(), &rep))
    else {
        return Vec::new();
    };
    if count > 0 {
        if let Some(buf) = model.active_buffer_mut() {
            buf.replace_all(&new_text);
        }
        model.invalidate_highlight();
    }
    model.notify(format!("{count} replaced"));
    recompute_find(model);
    Vec::new()
}

/// Case-insensitive (ASCII) literal match positions as [start, end) char indices.
/// Matches are non-overlapping.
fn find_matches(text: &str, query: &str) -> Vec<(usize, usize)> {
    if query.is_empty() {
        return Vec::new();
    }
    let t: Vec<char> = text.chars().collect();
    let q: Vec<char> = query.chars().collect();
    let (tl, ql) = (t.len(), q.len());
    let mut out = Vec::new();
    if ql == 0 || ql > tl {
        return out;
    }
    let ci_eq = |a: char, b: char| a.eq_ignore_ascii_case(&b);
    let mut i = 0;
    while i + ql <= tl {
        if (0..ql).all(|k| ci_eq(t[i + k], q[k])) {
            out.push((i, i + ql));
            i += ql;
        } else {
            i += 1;
        }
    }
    out
}

/// Builds a new string with every match of `query` replaced by `rep`.
fn replace_all_text(text: &str, query: &str, rep: &str) -> (String, usize) {
    let matches = find_matches(text, query);
    if matches.is_empty() {
        return (text.to_string(), 0);
    }
    let t: Vec<char> = text.chars().collect();
    let mut out = String::new();
    let mut last = 0;
    for &(s, e) in &matches {
        out.extend(&t[last..s]);
        out.push_str(rep);
        last = e;
    }
    out.extend(&t[last..]);
    (out, matches.len())
}

#[cfg(test)]
mod find_tests {
    use super::{find_matches, replace_all_text};

    #[test]
    fn matches_are_case_insensitive_and_non_overlapping() {
        assert_eq!(find_matches("aXaXa", "x"), vec![(1, 2), (3, 4)]);
        assert_eq!(find_matches("aaaa", "aa"), vec![(0, 2), (2, 4)]);
        assert!(find_matches("abc", "").is_empty());
        assert!(find_matches("abc", "abcd").is_empty());
    }

    #[test]
    fn replace_all_rebuilds_text() {
        assert_eq!(replace_all_text("foo Foo", "foo", "bar"), ("bar bar".to_string(), 2));
        assert_eq!(replace_all_text("abc", "x", "y"), ("abc".to_string(), 0));
    }
}
