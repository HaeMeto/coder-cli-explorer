//! Workspace text search (ignore + regex).

use std::path::{Path, PathBuf};

use ignore::WalkBuilder;
use regex::{NoExpand, Regex, RegexBuilder};

#[derive(Clone, Debug)]
pub struct SearchMatch {
    pub path: PathBuf,
    pub rel: String,
    pub line_no: usize,
    pub line: String,
}

/// Builds a Regex from `query`. If `use_regex` is false the pattern is escaped
/// (plain-text search). Case-insensitive unless `match_case` is true.
fn build_regex(query: &str, use_regex: bool, match_case: bool) -> Option<Regex> {
    let pattern = if use_regex {
        query.to_string()
    } else {
        regex::escape(query)
    };
    RegexBuilder::new(&pattern)
        .case_insensitive(!match_case)
        .build()
        .ok()
}

/// Builds the file walker. By default skips hidden (dot) files and .gitignore'd
/// paths; when `search_hidden` is true it descends into both.
fn walker(root: &Path, search_hidden: bool) -> ignore::Walk {
    WalkBuilder::new(root)
        .hidden(!search_hidden)
        .git_ignore(!search_hidden)
        .git_exclude(!search_hidden)
        .ignore(!search_hidden)
        .build()
}

/// Searches for `query` under `root` (plain/regex via `use_regex`).
/// The number of results is capped by `limit` (blocking; call inside spawn_blocking).
pub fn search(
    root: &Path,
    query: &str,
    use_regex: bool,
    match_case: bool,
    search_hidden: bool,
    limit: usize,
) -> Vec<SearchMatch> {
    let mut results = Vec::new();
    if query.is_empty() {
        return results;
    }
    let re = match build_regex(query, use_regex, match_case) {
        Some(re) => re,
        None => return results,
    };

    let walker = walker(root, search_hidden);

    for entry in walker.flatten() {
        if results.len() >= limit {
            break;
        }
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => continue, // skip binary files
        };
        let rel = path
            .strip_prefix(root)
            .unwrap_or(path)
            .to_string_lossy()
            .into_owned();
        for (i, line) in content.lines().enumerate() {
            if results.len() >= limit {
                break;
            }
            if re.is_match(line) {
                results.push(SearchMatch {
                    path: path.to_path_buf(),
                    rel: rel.clone(),
                    line_no: i + 1,
                    line: line.chars().take(200).collect(),
                });
            }
        }
    }
    results
}

/// Replaces `query` matches with `replace` on a single 1-based line of `path`
/// (the line a search result points at). Only that line is touched; matches on
/// other lines are left alone. Returns the number of replacements (0 if none).
/// Blocking; call in spawn_blocking.
pub fn replace_in_line(
    path: &Path,
    line_no: usize,
    query: &str,
    replace: &str,
    use_regex: bool,
    match_case: bool,
) -> usize {
    if query.is_empty() || line_no == 0 {
        return 0;
    }
    let re = match build_regex(query, use_regex, match_case) {
        Some(re) => re,
        None => return 0,
    };
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return 0,
    };
    // Keep line terminators so unrelated lines round-trip byte-for-byte.
    let segments: Vec<&str> = content.split_inclusive('\n').collect();
    let Some(seg) = segments.get(line_no - 1).copied() else {
        return 0;
    };
    // Split the terminator (\n or \r\n) off so the pattern sees only line text.
    let (body, term) = match seg.strip_suffix('\n') {
        Some(t) => match t.strip_suffix('\r') {
            Some(t) => (t, "\r\n"),
            None => (t, "\n"),
        },
        None => (seg, ""),
    };
    let count = re.find_iter(body).count();
    if count == 0 {
        return 0;
    }
    let replaced = if use_regex {
        re.replace_all(body, replace)
    } else {
        re.replace_all(body, NoExpand(replace))
    };
    let new_line = format!("{replaced}{term}");
    let mut new_content = String::with_capacity(content.len());
    for (i, s) in segments.iter().enumerate() {
        if i == line_no - 1 {
            new_content.push_str(&new_line);
        } else {
            new_content.push_str(s);
        }
    }
    if new_content != content && std::fs::write(path, new_content.as_bytes()).is_ok() {
        count
    } else {
        0
    }
}

/// Replaces `query` matches with `replace` in all files under `root`.
/// Returns the paths of the changed files and the total number of replacements
/// (blocking; call inside spawn_blocking).
///
/// If `use_regex` is true, group references like `$1` in `replace` are expanded;
/// if false, it is inserted as literal (unchanged) text.
pub fn replace_all(
    root: &Path,
    query: &str,
    replace: &str,
    use_regex: bool,
    match_case: bool,
    search_hidden: bool,
) -> (Vec<PathBuf>, usize) {
    let mut changed = Vec::new();
    let mut total = 0usize;
    if query.is_empty() {
        return (changed, total);
    }
    let re = match build_regex(query, use_regex, match_case) {
        Some(re) => re,
        None => return (changed, total),
    };

    let walker = walker(root, search_hidden);

    for entry in walker.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => continue, // skip binary files
        };
        let count = re.find_iter(&content).count();
        if count == 0 {
            continue;
        }
        let new = if use_regex {
            re.replace_all(&content, replace)
        } else {
            re.replace_all(&content, NoExpand(replace))
        };
        if new != content && std::fs::write(path, new.as_bytes()).is_ok() {
            total += count;
            changed.push(path.to_path_buf());
        }
    }
    (changed, total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Writes `content` to a unique temp file and returns its path.
    fn temp_file(content: &str) -> PathBuf {
        static N: AtomicUsize = AtomicUsize::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "coder-search-test-{}-{n}.txt",
            std::process::id()
        ));
        std::fs::write(&path, content).unwrap();
        path
    }

    #[test]
    fn replace_in_line_touches_only_that_line() {
        // "foo" appears on lines 1, 2 and 3; only line 2 must change.
        let path = temp_file("foo\nfoo bar foo\nfoo\n");
        let count = replace_in_line(&path, 2, "foo", "X", false, true);
        let out = std::fs::read_to_string(&path).unwrap();
        std::fs::remove_file(&path).ok();
        assert_eq!(count, 2); // both "foo" on line 2
        assert_eq!(out, "foo\nX bar X\nfoo\n");
    }

    #[test]
    fn replace_in_line_no_match_leaves_file() {
        let path = temp_file("alpha\nbeta\n");
        let count = replace_in_line(&path, 1, "zzz", "X", false, true);
        let out = std::fs::read_to_string(&path).unwrap();
        std::fs::remove_file(&path).ok();
        assert_eq!(count, 0);
        assert_eq!(out, "alpha\nbeta\n");
    }

    #[test]
    fn replace_in_line_preserves_crlf_terminator() {
        let path = temp_file("foo\r\nfoo\r\n");
        let count = replace_in_line(&path, 1, "foo", "bar", false, true);
        let out = std::fs::read_to_string(&path).unwrap();
        std::fs::remove_file(&path).ok();
        assert_eq!(count, 1);
        assert_eq!(out, "bar\r\nfoo\r\n");
    }
}
