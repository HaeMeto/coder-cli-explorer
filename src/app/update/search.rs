//! Search-panel run/replace commands.

use super::*;

/// Re-runs the workspace search with the current query/options (after a
/// checkbox toggle). No-op when the query is empty.
pub(super) fn rerun_search(model: &mut Model) -> Vec<Cmd> {
    let s = &model.sidebar.search;
    if s.query.is_empty() {
        return Vec::new();
    }
    vec![Cmd::RunSearch {
        query: s.query.content().to_string(),
        use_regex: s.use_regex,
        match_case: s.match_case,
        search_hidden: s.search_hidden,
    }]
}

/// Search panel "Replace All": replaces across all files under the workspace.
pub(super) fn search_replace_all(model: &mut Model) -> Vec<Cmd> {
    let s = &model.sidebar.search;
    if s.query.is_empty() {
        return Vec::new();
    }
    let (query, replace, use_regex, match_case, search_hidden) = (
        s.query.content().to_string(),
        s.replace.content().to_string(),
        s.use_regex,
        s.match_case,
        s.search_hidden,
    );
    model.notify("Replacing…".to_string());
    vec![Cmd::RunReplace {
        query,
        replace,
        use_regex,
        match_case,
        search_hidden,
    }]
}

/// Search panel "Replace": replaces only on the selected result's line.
pub(super) fn search_replace_one(model: &mut Model) -> Vec<Cmd> {
    let s = &model.sidebar.search;
    if s.query.is_empty() {
        return Vec::new();
    }
    let Some(m) = s.results.get(s.selected) else {
        return Vec::new();
    };
    let (path, line_no) = (m.path.clone(), m.line_no);
    let (query, replace, use_regex, match_case) = (
        s.query.content().to_string(),
        s.replace.content().to_string(),
        s.use_regex,
        s.match_case,
    );
    model.notify("Replacing…".to_string());
    vec![Cmd::RunReplaceLine {
        path,
        line_no,
        query,
        replace,
        use_regex,
        match_case,
    }]
}
