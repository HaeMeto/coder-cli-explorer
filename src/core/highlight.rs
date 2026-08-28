//! syntect-based syntax highlighting; results are cached per buffer version.

use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use ratatui::style::Color;
use syntect::highlighting::{
    Color as SynColor, HighlightIterator, HighlightState, Highlighter as SynHighlighter,
    Style as SynStyle, ThemeSet,
};
use syntect::parsing::{
    ParseState, ScopeStack, SyntaxDefinition, SyntaxReference, SyntaxSet,
};
use syntect::util::LinesWithEndings;

use crate::core::theme::Theme;

static SYNTAX_SET: OnceLock<SyntaxSet> = OnceLock::new();
static THEME_SET: OnceLock<ThemeSet> = OnceLock::new();

/// Sublime-syntax definitions compiled into the binary for languages syntect
/// doesn't bundle (e.g. TOML), so highlighting works without external files.
static EMBEDDED_SYNTAXES: &[&str] =
    &[include_str!("../../assets/syntaxes/TOML.sublime-syntax")];

fn syntax_set() -> &'static SyntaxSet {
    SYNTAX_SET.get_or_init(|| {
        // `_newlines` because `highlight_line` is fed lines that keep their `\n`.
        let mut builder = SyntaxSet::load_defaults_newlines().into_builder();
        for src in EMBEDDED_SYNTAXES {
            if let Ok(def) = SyntaxDefinition::load_from_str(src, true, None) {
                builder.add(def);
            }
        }
        // Extra user/asset syntaxes (best-effort, silently skipped if absent).
        for dir in syntax_dirs() {
            let _ = builder.add_from_folder(&dir, true);
        }
        builder.build()
    })
}

/// Folders searched for extra `.sublime-syntax` files: user config, env override, repo assets.
fn syntax_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(home) = std::env::var("HOME") {
        dirs.push(PathBuf::from(home).join(".config/coder/syntaxes"));
    }
    if let Ok(d) = std::env::var("CODER_SYNTAXES_DIR") {
        dirs.push(PathBuf::from(d));
    }
    dirs.push(PathBuf::from("assets/syntaxes"));
    dirs
}

fn theme_set() -> &'static ThemeSet {
    THEME_SET.get_or_init(|| {
        let mut ts = ThemeSet::load_defaults();
        // Popular full `.tmTheme` files compiled into the binary. These are the
        // real Sublime Text themes (complete scope coverage), not approximations,
        // so every listed theme is genuinely syntect-compatible.
        for (name, src) in EMBEDDED_THEMES {
            if let Ok(mut theme) = ThemeSet::load_from_reader(&mut Cursor::new(src.as_bytes())) {
                // Key the picker entry off our chosen display name, not the file's
                // internal one (e.g. Gruvbox ships as "gruvbox (Dark) (Medium)").
                theme.name = Some((*name).to_string());
                ts.themes.insert((*name).to_string(), theme);
            }
        }
        // Extra user `.tmTheme` files in config folders (silently skipped if absent).
        for dir in theme_dirs() {
            let _ = ts.add_from_folder(&dir);
        }
        ts
    })
}

/// Folders searched for .tmTheme files: user config, env override, repo assets.
fn theme_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(home) = std::env::var("HOME") {
        dirs.push(PathBuf::from(home).join(".config/coder/themes"));
    }
    if let Ok(d) = std::env::var("CODER_THEMES_DIR") {
        dirs.push(PathBuf::from(d));
    }
    dirs.push(PathBuf::from("assets/themes"));
    dirs
}

/// Popular full Sublime Text `.tmTheme` files compiled into the binary, as
/// `(display name, XML source)`. Unlike a hand-rolled palette these carry the
/// theme's complete scope rules, so they highlight every token kind the real
/// Sublime/bat versions do. Only genuinely syntect-loadable themes belong here
/// — the picker must never list a theme that can't render (see `theme_set`).
///
/// `.tmTheme` (TextMate/Sublime XML plist) is the only theme format syntect
/// understands; the newer `.sublime-color-scheme` JSON is not supported, so
/// these are sourced from projects that still ship the XML form (bat's set).
static EMBEDDED_THEMES: &[(&str, &str)] = &[
    ("Dracula", include_str!("../../assets/themes/Dracula.tmTheme")),
    ("Nord", include_str!("../../assets/themes/Nord.tmTheme")),
    ("Monokai Extended", include_str!("../../assets/themes/Monokai Extended.tmTheme")),
    ("One Dark", include_str!("../../assets/themes/One Dark.tmTheme")),
    ("Gruvbox Dark", include_str!("../../assets/themes/Gruvbox Dark.tmTheme")),
    ("Gruvbox Light", include_str!("../../assets/themes/Gruvbox Light.tmTheme")),
    ("Catppuccin Mocha", include_str!("../../assets/themes/Catppuccin Mocha.tmTheme")),
    ("Catppuccin Latte", include_str!("../../assets/themes/Catppuccin Latte.tmTheme")),
];

/// Default syntect theme used at application startup.
pub const DEFAULT_THEME: &str = "base16-eighties.dark";

/// Forces the syntax and theme sets to build now. They are otherwise built
/// lazily on the first highlight — deserializing the bundled syntax dump costs
/// ~500ms and would stall the first file open. Call from a background thread at
/// startup; `OnceLock` makes the race with the first real use harmless.
pub fn warm() {
    syntax_set();
    theme_set();
}

/// Names of the loaded syntect themes (alphabetical; BTreeMap order).
pub fn theme_names() -> Vec<String> {
    theme_set().themes.keys().cloned().collect()
}

/// Derives the UI palette from a syntect theme's `settings` field.
/// Fields without a counterpart (such as git colors) come from `Theme::default()`.
pub fn theme_for(name: &str) -> Theme {
    let def = Theme::default();
    let Some(t) = theme_set().themes.get(name) else {
        return def;
    };
    let s = &t.settings;
    let bg = s.background.map(conv).unwrap_or(def.bg);
    let fg = s.foreground.map(conv).unwrap_or(def.fg);
    let dark = luma(bg) < 128.0;
    let accent = s.caret.or(s.find_highlight).map(conv).unwrap_or(def.accent);
    // Status bar: a touch darker than the accent on dark themes, a touch lighter on light ones.
    let statusbar_bg = shade(accent, if dark { 0.8 } else { 1.2 });
    Theme {
        bg,
        bg_alt: shade(bg, if dark { 1.25 } else { 0.94 }),
        fg,
        fg_dim: mix(fg, bg, 0.45),
        accent,
        selection: s.selection.map(conv).unwrap_or(def.selection),
        activity_bg: shade(bg, if dark { 1.45 } else { 0.90 }),
        statusbar_bg,
        statusbar_fg: if luma(statusbar_bg) < 128.0 {
            Color::Rgb(255, 255, 255)
        } else {
            Color::Rgb(0, 0, 0)
        },
        tab_active_bg: bg,
        tab_inactive_bg: shade(bg, if dark { 1.25 } else { 0.94 }),
        border: shade(bg, if dark { 1.9 } else { 0.82 }),
        line_number: s.gutter_foreground.map(conv).unwrap_or(def.line_number),
        git_added: def.git_added,
        git_modified: def.git_modified,
        git_deleted: def.git_deleted,
        git_untracked: def.git_untracked,
        // Subtle change backgrounds: mostly the editor bg with a hint of the git color.
        diff_add_bg: mix(def.git_added, bg, 0.82),
        diff_del_bg: mix(def.git_deleted, bg, 0.82),
        // The theme's own find-highlight color when it defines one, else yellow.
        find_match: s.find_highlight.map(conv).unwrap_or(def.find_match),
        find_current: def.find_current,
    }
}

fn conv(c: SynColor) -> Color {
    Color::Rgb(c.r, c.g, c.b)
}

fn rgb_parts(c: Color) -> (u8, u8, u8) {
    if let Color::Rgb(r, g, b) = c {
        (r, g, b)
    } else {
        (128, 128, 128)
    }
}

fn luma(c: Color) -> f32 {
    let (r, g, b) = rgb_parts(c);
    0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32
}

/// Lightens (>1) or darkens (<1) the color by the given factor.
fn shade(c: Color, f: f32) -> Color {
    let (r, g, b) = rgb_parts(c);
    let ap = |v: u8| (v as f32 * f).clamp(0.0, 255.0) as u8;
    Color::Rgb(ap(r), ap(g), ap(b))
}

/// Mixes color `a` toward `b` by the ratio `t`.
fn mix(a: Color, b: Color, t: f32) -> Color {
    let (ar, ag, ab) = rgb_parts(a);
    let (br, bg, bb) = rgb_parts(b);
    let m = |x: u8, y: u8| (x as f32 * (1.0 - t) + y as f32 * t) as u8;
    Color::Rgb(m(ar, br), m(ag, bg), m(ab, bb))
}

/// The highlighted pieces of a line: (color, text).
pub type HlLine = Vec<(Color, String)>;

pub struct Highlighter {
    /// Syntax name for the file this highlighter belongs to.
    syntax_name: String,
    theme_name: String,
    /// Which buffer version the cache was produced for.
    cached_version: Option<u64>,
    /// Highlighted lines computed so far — a *prefix* of the file (only lines up
    /// to the viewport bottom are highlighted; the rest fill in on scroll).
    cache: Vec<HlLine>,
    /// syntect parse + highlight state *after* each cached line. `states[i]` is
    /// the state that begins line `i + 1`, so re-highlighting can resume from any
    /// line without re-parsing the whole file from the top.
    states: Vec<(ParseState, HighlightState)>,
}

impl Highlighter {
    pub fn for_path(path: Option<&Path>) -> Self {
        let ss = syntax_set();
        let syntax = path
            .and_then(|p| p.extension().and_then(|e| e.to_str()))
            .and_then(|ext| ss.find_syntax_by_extension(ext))
            .or_else(|| {
                path.and_then(|p| p.file_name().and_then(|n| n.to_str()))
                    .and_then(|name| ss.find_syntax_by_token(name))
            })
            .unwrap_or_else(|| ss.find_syntax_plain_text());
        Highlighter {
            syntax_name: syntax.name.clone(),
            theme_name: DEFAULT_THEME.to_string(),
            cached_version: None,
            cache: Vec::new(),
            states: Vec::new(),
        }
    }

    /// Changes the syntax theme and invalidates the cache
    /// (the next `highlight()` recomputes with the new theme).
    pub fn set_theme(&mut self, name: &str) {
        if self.theme_name != name {
            self.theme_name = name.to_string();
            self.reset_cache();
        }
    }

    /// Drops the cached highlight so the next `highlight()` recomputes. Needed
    /// when the buffer's content is replaced without advancing its version
    /// (e.g. a disk reload rebuilds the buffer back to version 0).
    pub fn invalidate(&mut self) {
        self.reset_cache();
    }

    /// Clears every cached line and the parse-state snapshots.
    fn reset_cache(&mut self) {
        self.cached_version = None;
        self.cache.clear();
        self.states.clear();
    }

    /// The highlighted lines computed so far (a prefix of the file). The renderer
    /// borrows this directly instead of cloning it each frame.
    pub fn cached(&self) -> &[HlLine] {
        &self.cache
    }

    fn syntax(&self) -> &'static SyntaxReference {
        let ss = syntax_set();
        ss.find_syntax_by_name(&self.syntax_name)
            .unwrap_or_else(|| ss.find_syntax_plain_text())
    }

    /// Highlights `text` incrementally, up to `needed` lines from the top.
    ///
    /// Only the lines the editor is about to draw need coloring, so cached lines
    /// above the change are reused and lines below `needed` are left for a later
    /// call (they fill in on scroll). Within that window there are two regimes:
    ///
    /// - **Single-line edit** (`!wide`): only `dirty_from`'s text changed, so this
    ///   re-highlights from there and stops the instant a line's parse+highlight
    ///   state matches the cached one — every line below then colors identically.
    ///   Typing a character usually reconverges on the first line, so one line is
    ///   redone instead of the whole viewport tail. This is what keeps typing in a
    ///   big Markdown file smooth (syntect parses each md line ~5-15× slower than a
    ///   config line, so re-parsing 50 tail lines per keystroke is the felt lag).
    /// - **Wide edit / scroll / first paint**: the tail is rebuilt or extended
    ///   line by line from the resumed state down to `needed`.
    ///
    /// - `version`: buffer version; a change means text below `dirty_from` is stale.
    /// - `dirty_from`: lowest line touched (`usize::MAX` when only the viewport moved).
    /// - `wide`: the edit crossed a line boundary, so line indices may have shifted
    ///   and the convergence shortcut is unsafe.
    /// - `needed`: how many lines from the top the caller intends to render.
    pub fn highlight(
        &mut self,
        text: &str,
        version: u64,
        dirty_from: usize,
        wide: bool,
        needed: usize,
    ) -> &[HlLine] {
        let version_changed = self.cached_version != Some(version);
        self.cached_version = Some(version);

        let ss = syntax_set();
        let syntax = self.syntax();
        let themes = &theme_set().themes;
        // Fall back to the default theme if the configured name is unknown (e.g.
        // a stale name in a hand-edited config) — indexing a missing key panics.
        let theme = themes
            .get(&self.theme_name)
            .unwrap_or_else(|| &themes[DEFAULT_THEME]);
        let highlighter = SynHighlighter::new(theme);

        // A multi-line edit may have shifted line numbers: cached entries at and
        // below the change no longer align, so drop them and rebuild the tail below.
        if version_changed && wide {
            let keep = dirty_from.min(self.cache.len());
            self.cache.truncate(keep);
            self.states.truncate(keep);
        }

        // Single-line edit: re-highlight from `dirty_from`, stop on state reconverge.
        if version_changed && !wide && dirty_from < self.cache.len() {
            let (mut ps, mut hs) = self.resume_state(dirty_from, syntax, &highlighter);
            let mut idx = dirty_from;
            for line in LinesWithEndings::from(text).skip(dirty_from) {
                let hl = highlight_line(&mut ps, &mut hs, line, ss, &highlighter);
                let state = (ps.clone(), hs.clone());
                if idx < self.cache.len() {
                    let converged = state == self.states[idx];
                    self.cache[idx] = hl;
                    self.states[idx] = state;
                    idx += 1;
                    if converged {
                        return &self.cache; // every line below is unchanged
                    }
                    if idx >= needed {
                        // Still diverging past the viewport — drop the rest and let
                        // it recompute on demand rather than chase convergence off
                        // screen (bounds a pathological edit to the visible region).
                        self.cache.truncate(idx);
                        self.states.truncate(idx);
                        return &self.cache;
                    }
                } else {
                    if idx >= needed {
                        break;
                    }
                    self.cache.push(hl);
                    self.states.push(state);
                    idx += 1;
                }
            }
            return &self.cache;
        }

        // Extend the cached prefix downward to `needed`: pure scroll, the tail of a
        // wide edit, or the first paint. Nothing to do if it already reaches there.
        if self.cache.len() >= needed {
            return &self.cache;
        }
        let start = self.cache.len();
        let (mut ps, mut hs) = self.resume_state(start, syntax, &highlighter);
        for line in LinesWithEndings::from(text).skip(start) {
            if self.cache.len() >= needed {
                break;
            }
            let hl = highlight_line(&mut ps, &mut hs, line, ss, &highlighter);
            self.cache.push(hl);
            self.states.push((ps.clone(), hs.clone()));
        }
        // The final empty line (when text ends in `\n`) is intentionally left
        // uncached: the renderer draws a missing line as plain text, which for an
        // empty line is identical to an empty highlight.
        &self.cache
    }

    /// The parse + highlight state entering `line` — the snapshot cached after the
    /// previous line, or a fresh start at the top of the file.
    fn resume_state(
        &self,
        line: usize,
        syntax: &SyntaxReference,
        highlighter: &SynHighlighter,
    ) -> (ParseState, HighlightState) {
        match line.checked_sub(1).and_then(|k| self.states.get(k)) {
            Some((ps, hs)) => (ps.clone(), hs.clone()),
            None => (
                ParseState::new(syntax),
                HighlightState::new(highlighter, ScopeStack::new()),
            ),
        }
    }
}

/// Longest line syntect will fully parse. Some syntaxes scan the whole line with
/// backtracking — Markdown's link/bracket rules are O(n²), so one 4000-char line
/// of `[` takes ~50ms, and re-run on every keystroke that is the editing freeze.
/// Past this we color only the head and render the tail as plain text; lines this
/// long are rare and usually minified/data where coloring adds little anyway.
const MAX_HL_LINE: usize = 512;

/// Highlights one line, advancing `ps`/`hs`, into a run of `(color, text)` pieces.
fn highlight_line(
    ps: &mut ParseState,
    hs: &mut HighlightState,
    line: &str,
    ss: &SyntaxSet,
    highlighter: &SynHighlighter,
) -> HlLine {
    // Hand syntect at most `MAX_HL_LINE` chars (split on a char boundary so both
    // halves stay valid UTF-8); the remainder is emitted uncolored. This bounds
    // per-line cost against O(n²) syntaxes.
    let (head, tail) = match line.char_indices().nth(MAX_HL_LINE) {
        Some((i, _)) => (&line[..i], &line[i..]),
        None => (line, ""),
    };
    let ops = ps.parse_line(head, ss).unwrap_or_default();
    let mut out: HlLine = Vec::new();
    for (style, piece) in HighlightIterator::new(hs, &ops, head, highlighter) {
        let piece = piece.trim_end_matches(['\n', '\r']);
        if piece.is_empty() {
            continue;
        }
        out.push((syn_to_color(style), piece.to_string()));
    }
    let tail = tail.trim_end_matches(['\n', '\r']);
    if !tail.is_empty() {
        out.push((syn_to_color(highlighter.get_default()), tail.to_string()));
    }
    out
}

fn syn_to_color(style: SynStyle) -> Color {
    Color::Rgb(style.foreground.r, style.foreground.g, style.foreground.b)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Per-keystroke re-highlight cost: a single-line edit mid-document with a
    /// viewport-bounded window. Run with `--ignored --nocapture` to read the
    /// numbers; asserts only that markdown stays cheap (convergence early-exit).
    #[test]
    #[ignore]
    fn bench_edit_latency() {
        use std::time::Instant;
        let run = |path: &str, unit: &str| -> f64 {
            let big: String = unit.repeat(2000);
            let mut hl = Highlighter::for_path(Some(Path::new(path)));
            let dirty = 1000;
            let needed = dirty + 50; // scroll_y + terminal height, viewport-bounded
            hl.highlight(&big, 0, 0, false, needed);
            let t = Instant::now();
            for v in 1..=200u64 {
                hl.highlight(&big, v, dirty, false, needed);
            }
            let per = t.elapsed().as_micros() as f64 / 200.0;
            eprintln!("{path}: {per:.0} us/edit");
            per
        };
        run("doc.md", "text with *emphasis* and `code` and [a](b)\n");
        run("doc.toml", "key = \"value\" # comment\n");

        // A single long line of '[': Markdown link scanning is O(n²), so without
        // the `MAX_HL_LINE` cap this climbs to tens of ms and freezes typing.
        for len in [500usize, 1000, 2000, 4000] {
            let line: String = "[".repeat(len) + "\n";
            let mut hl = Highlighter::for_path(Some(Path::new("doc.md")));
            let t = Instant::now();
            hl.highlight(&line, 0, 0, false, usize::MAX);
            eprintln!("doc.md '[' x{len}: {} us", t.elapsed().as_micros());
        }
    }

    #[test]
    fn long_line_is_capped_but_fully_covered() {
        // A line past `MAX_HL_LINE` must still render in full (head colored, tail
        // plain) — no truncation of the visible text, no panic.
        let len = MAX_HL_LINE + 500;
        let line: String = "[".repeat(len) + "\n";
        let mut hl = Highlighter::for_path(Some(Path::new("doc.md")));
        let out = hl.highlight(&line, 0, 0, false, usize::MAX);
        let covered: usize = out[0].iter().map(|(_, s)| s.chars().count()).sum();
        assert_eq!(covered, len, "every character is still emitted");
    }

    #[test]
    fn builtin_themes_listed() {
        let names = theme_names();
        // syntect defaults (7) + embedded full .tmTheme files.
        assert!(names.len() >= 7 + EMBEDDED_THEMES.len());
        for (name, _) in EMBEDDED_THEMES {
            assert!(names.iter().any(|n| n == name), "missing theme: {name}");
        }
        assert!(names.iter().any(|n| n == DEFAULT_THEME));
    }

    #[test]
    fn every_embedded_theme_loads() {
        // Guards the "no incompatible theme in the list" contract: each bundled
        // file must actually parse into a syntect theme, or it must not ship.
        let ts = theme_set();
        for (name, _) in EMBEDDED_THEMES {
            let theme = ts.themes.get(*name).unwrap_or_else(|| panic!("did not load: {name}"));
            assert!(theme.settings.background.is_some(), "no background: {name}");
        }
    }

    #[test]
    fn builtin_theme_derives_palette() {
        // The derived UI palette should differ from the default (settings are read).
        let t = theme_for("Dracula");
        assert_eq!(t.bg, Color::Rgb(0x28, 0x2a, 0x36));
    }

    #[test]
    fn toml_files_resolve_to_toml_syntax() {
        let hl = Highlighter::for_path(Some(Path::new("config.toml")));
        assert_eq!(hl.syntax_name, "TOML");
    }

    #[test]
    fn commit_diff_tabs_borrow_the_changed_files_syntax() {
        // A commit's diff tab has no file of its own: it carries a synthetic
        // `<hash>.<ext>` path so the code in it is colored like the file it
        // came from.
        let hl = Highlighter::for_path(Some(Path::new("2ea14b1.rs")));
        assert_eq!(hl.syntax_name, "Rust");
    }

    #[test]
    fn incremental_matches_full_highlight() {
        let text = "# heading\n\n```rust\nfn main() {}\n```\n\nsome *text* here\n";
        // Baseline: highlight everything in one shot.
        let mut full = Highlighter::for_path(Some(Path::new("doc.md")));
        let want = full.highlight(text, 0, 0, false, usize::MAX).to_vec();

        // Viewport-bounded: only the first 2 lines, then extend to the rest.
        // The extension must resume mid-file and reproduce the full result.
        let mut inc = Highlighter::for_path(Some(Path::new("doc.md")));
        let first = inc.highlight(text, 0, usize::MAX, false, 2).to_vec();
        assert_eq!(first.len(), 2, "only the requested lines are cached");
        assert_eq!(&first[..], &want[..2]);
        let all = inc.highlight(text, 0, usize::MAX, false, usize::MAX).to_vec();
        assert_eq!(all, want, "resumed highlight equals the full one");
    }

    #[test]
    fn edit_rehighlights_from_dirty_line() {
        let text = "line one\nline two\nline three\n";
        let mut hl = Highlighter::for_path(Some(Path::new("doc.md")));
        hl.highlight(text, 0, 0, false, usize::MAX);

        // Edit the second line; version bumps, dirty_from points at it.
        let edited = "line one\nline TWO changed\nline three\n";
        let got = hl.highlight(edited, 1, 1, false, usize::MAX).to_vec();

        // Must equal a from-scratch highlight of the edited text.
        let mut fresh = Highlighter::for_path(Some(Path::new("doc.md")));
        let want = fresh.highlight(edited, 0, 0, false, usize::MAX).to_vec();
        assert_eq!(got, want);
    }

    #[test]
    fn single_line_edit_that_shifts_state_repaints_tail() {
        // A single-line edit whose parse state does NOT reconverge must repaint
        // every following line, not stop at the edited one. Opening an unterminated
        // Rust block comment turns all lines below into comment scope.
        let text = "let a = 1;\nlet b = 2;\nlet c = 3;\n";
        let mut hl = Highlighter::for_path(Some(Path::new("code.rs")));
        hl.highlight(text, 0, 0, false, usize::MAX);

        // Replace line 0 with a comment opener (no newline added -> wide = false).
        let edited = "/* open comment\nlet b = 2;\nlet c = 3;\n";
        let got = hl.highlight(edited, 1, 0, false, usize::MAX).to_vec();

        let mut fresh = Highlighter::for_path(Some(Path::new("code.rs")));
        let want = fresh.highlight(edited, 0, 0, false, usize::MAX).to_vec();
        assert_eq!(got, want, "lines below a state-changing edit must repaint");
    }

    #[test]
    fn wide_edit_rebuilds_tail() {
        // Splitting a line (inserting a newline) shifts every line index below it;
        // the wide path must drop the misaligned tail and rebuild it correctly.
        let text = "alpha\nbeta\ngamma\n";
        let mut hl = Highlighter::for_path(Some(Path::new("doc.md")));
        hl.highlight(text, 0, 0, false, usize::MAX);

        // Insert a newline inside "beta" -> "be\nta"; wide = true, dirty_from = 1.
        let edited = "alpha\nbe\nta\ngamma\n";
        let got = hl.highlight(edited, 1, 1, true, usize::MAX).to_vec();

        let mut fresh = Highlighter::for_path(Some(Path::new("doc.md")));
        let want = fresh.highlight(edited, 0, 0, false, usize::MAX).to_vec();
        assert_eq!(got, want, "wide edit must rebuild the shifted tail");
    }

    #[test]
    fn toml_line_highlights_multiple_scopes() {
        let mut hl = Highlighter::for_path(Some(Path::new("config.toml")));
        // A key, a string and a comment should come out as distinct colors, not
        // one flat run of plain-text foreground.
        let lines = hl.highlight("theme = \"dark\" # note\n", 0, 0, false, usize::MAX);
        let colors: std::collections::HashSet<_> =
            lines[0].iter().map(|(c, _)| *c).collect();
        assert!(colors.len() >= 3, "expected varied coloring, got {colors:?}");
    }
}
