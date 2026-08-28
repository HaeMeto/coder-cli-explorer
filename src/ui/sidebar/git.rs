//! Git panel: change tree, commit box, fetch/pull/push actions.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::model::{Focus, GitStatus, GitZone, Model};
use crate::services::git::{GitEntry, GitState};
use crate::ui::text_input::TextInput;

use super::{list_scroll, panel_area};

/// Type of a scrollable content row in the Git panel.
#[derive(Clone)]
pub enum GitRowKind {
    StagedHeader,
    ChangesHeader,
    /// "Unstage All -" row (under the staged header).
    UnstageAll,
    /// "Stage All +" row (under the unstaged header).
    StageAll,
    /// Separator line between the action row and the file list.
    Separator,
    /// A directory node in the change tree (display only, not selectable).
    Dir {
        name: String,
        depth: usize,
    },
    /// A staged file: `idx` into `staged`, `depth` in the tree.
    Staged {
        idx: usize,
        depth: usize,
    },
    /// An unstaged file: `idx` into `unstaged`, `depth` in the tree.
    Unstaged {
        idx: usize,
        depth: usize,
    },
    /// Keyboard hint under the change list ("Stage/Unstage: a, Revert: r").
    Hint,
    /// The "HISTORY" heading above the previous-commits list.
    HistoryHeader,
    /// Keyboard hint under the HISTORY heading ("Show diff: Enter").
    HistoryHint,
    /// A previous commit: `idx` into `history`.
    Commit {
        idx: usize,
    },
    Info(&'static str),
}

/// Emits tree rows (directory headers + file rows) for one section's entries,
/// grouping by path like the file explorer. `make_row` builds the file row
/// (`Staged`/`Unstaged`) from an entry index + its tree depth.
fn tree_rows(
    entries: &[GitEntry],
    make_row: impl Fn(usize, usize) -> GitRowKind,
    rows: &mut Vec<GitRowKind>,
) {
    // Sort entry indices by path so shared directories are contiguous.
    let mut order: Vec<usize> = (0..entries.len()).collect();
    order.sort_by(|&a, &b| entries[a].rel.cmp(&entries[b].rel));

    let mut prev: Vec<&str> = Vec::new();
    for &i in &order {
        let parts: Vec<&str> = entries[i].rel.split('/').collect();
        let dirs = &parts[..parts.len() - 1];
        // Emit directory headers for components not shared with the previous row.
        let mut common = 0;
        while common < dirs.len() && common < prev.len() && dirs[common] == prev[common] {
            common += 1;
        }
        for (d, name) in dirs.iter().enumerate().skip(common) {
            rows.push(GitRowKind::Dir {
                name: name.to_string(),
                depth: d,
            });
        }
        rows.push(make_row(i, dirs.len()));
        prev = dirs.to_vec();
    }
}

/// Height of the commit message box (rows).
const COMMIT_INPUT_H: u16 = 4;

/// Display width of the leftmost file icon on each change row (glyph + space).
/// Clicking within these columns opens the plain file instead of the diff.
const FILE_ICON_W: usize = 2;

/// Labels for the commit-row buttons (Uncommit left-aligned, Commit right-aligned).
const UNCOMMIT_LABEL: &str = " Uncommit ";
const COMMIT_LABEL: &str = " Commit ";

/// Disabled-button colors, fixed regardless of the active theme.
const DISABLED_FG: Color = Color::Rgb(140, 140, 140);
const DISABLED_BG: Color = Color::Rgb(60, 60, 60);

/// Style of a panel button. The keyboard-focused one swaps its foreground and
/// background, which reads as a distinctly different colour in every theme and
/// on the disabled (grey) buttons alike — where a highlight colour of its own
/// would have to be picked per theme.
fn button_style(th: &crate::core::theme::Theme, enabled: bool, focused: bool) -> Style {
    let (fg, bg) = if enabled {
        (th.statusbar_fg, th.accent)
    } else {
        (DISABLED_FG, DISABLED_BG)
    };
    let (fg, bg) = if focused { (bg, fg) } else { (fg, bg) };
    let style = Style::new().fg(fg).bg(bg);
    if focused {
        style.add_modifier(Modifier::BOLD)
    } else {
        style
    }
}

/// The Git panel layout computed once; shared by render and mouse hit-testing.
pub struct GitLayout {
    pub rows: Vec<GitRowKind>,
    /// y of the branch row at the top (only meaningful when `branch_shown`).
    pub branch_y: u16,
    /// Whether the branch row is drawn.
    pub branch_shown: bool,
    /// y of the first scrollable change row (below the commit block).
    pub content_y: u16,
    /// Scrollable list height.
    pub list_h: u16,
    pub offset: usize,
    /// y of the fetch/pull/push button row (just above the commit box).
    pub actions_y: u16,
    /// Top y of the commit message input field (height `COMMIT_INPUT_H`).
    pub input_top: u16,
    /// y of the commit button row.
    pub button_y: u16,
    /// Whether the commit box is shown (whether a repo exists).
    pub has_box: bool,
}

/// Computes the Git panel layout. `area` is the panel **body** region
/// (`panel_area`), i.e. below the sidebar title + spacer.
///
/// Top→bottom: branch row, blank, fetch/pull/push row, commit input box, commit
/// button, blank, then the scrollable change list (which runs to the bottom).
pub fn git_layout(model: &Model, area: Rect) -> GitLayout {
    let g = &model.sidebar.git;
    let mut rows = Vec::new();
    if !g.is_repo {
        rows.push(GitRowKind::Info("No git repository"));
    } else {
        if !g.staged.is_empty() {
            rows.push(GitRowKind::StagedHeader);
            rows.push(GitRowKind::UnstageAll);
            rows.push(GitRowKind::Separator);
            tree_rows(
                &g.staged,
                |idx, depth| GitRowKind::Staged { idx, depth },
                &mut rows,
            );
        }
        if !g.unstaged.is_empty() {
            rows.push(GitRowKind::ChangesHeader);
            rows.push(GitRowKind::StageAll);
            rows.push(GitRowKind::Separator);
            tree_rows(
                &g.unstaged,
                |idx, depth| GitRowKind::Unstaged { idx, depth },
                &mut rows,
            );
        }
        if g.staged.is_empty() && g.unstaged.is_empty() {
            rows.push(GitRowKind::Info("No changes"));
        } else {
            // The row shortcuts, right under the last change and above the
            // HISTORY divider. Only worth showing when there is a row to act on.
            rows.push(GitRowKind::Hint);
        }
        // Previous commits below the changes, separated by a divider.
        if !g.history.is_empty() {
            rows.push(GitRowKind::Separator);
            rows.push(GitRowKind::HistoryHeader);
            rows.push(GitRowKind::HistoryHint);
            for idx in 0..g.history.len() {
                rows.push(GitRowKind::Commit { idx });
            }
        }
    }

    let has_box = g.is_repo;
    let top = area.y;
    let branch_shown = g.is_repo && g.branch.is_some();
    let branch_y = top;

    // Fixed top block (only with a repo): branch, blank, fetch/pull/push row,
    // input box, button, blank. The change list then runs to the bottom.
    let (content_y, input_top, button_y, actions_y, list_h) = if has_box {
        let actions_y = top + if branch_shown { 1 } else { 0 } + 1; // branch + blank
        let input_top = actions_y + 1; // right below the fetch/pull/push row
        let button_y = input_top + COMMIT_INPUT_H;
        let content_y = button_y + 2; // blank, then the change list
        let bottom = area.y + area.height;
        let list_h = bottom.saturating_sub(content_y);
        (content_y, input_top, button_y, actions_y, list_h)
    } else {
        (top, 0, 0, 0, area.height)
    };

    let sel_pos = selected_row_pos(g, &rows);
    let offset = list_scroll(sel_pos, rows.len(), list_h as usize);

    GitLayout {
        rows,
        branch_y,
        branch_shown,
        content_y,
        list_h,
        offset,
        actions_y,
        input_top,
        button_y,
        has_box,
    }
}

/// The three git action buttons, left to right.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum GitAction {
    Fetch,
    Pull,
    Push,
}

/// Cell widths of the three action buttons, leaving a 1-column gap between them.
fn action_segments(width: usize) -> (usize, usize, usize) {
    if width < 5 {
        return (width, 0, 0); // too narrow for gaps
    }
    let inner = width - 2; // two 1-col gaps
    let seg = inner / 3;
    (seg, seg, inner - 2 * seg)
}

/// Which action button covers column `x` within the sidebar `area` (gaps map to None).
fn action_at_col(area: Rect, x: u16) -> Option<GitAction> {
    let rel = x.checked_sub(area.x)? as usize;
    let (w0, w1, _) = action_segments(area.width as usize);
    if rel < w0 {
        Some(GitAction::Fetch)
    } else if rel < w0 + 1 {
        None // gap
    } else if rel < w0 + 1 + w1 {
        Some(GitAction::Pull)
    } else if rel < w0 + 2 + w1 {
        None // gap
    } else {
        Some(GitAction::Push)
    }
}

/// Position of the selected item (combined index) in the row list.
fn selected_row_pos(g: &GitStatus, rows: &[GitRowKind]) -> usize {
    // The combined index runs staged, then unstaged, then the history commits.
    if let Some(commit_idx) = g.selected.checked_sub(g.changes_len()) {
        for (pos, r) in rows.iter().enumerate() {
            if matches!(r, GitRowKind::Commit { idx } if *idx == commit_idx) {
                return pos;
            }
        }
        return 0;
    }
    let (want_staged, want_idx) = if g.selected < g.staged.len() {
        (true, g.selected)
    } else {
        (false, g.selected - g.staged.len())
    };
    for (pos, r) in rows.iter().enumerate() {
        match r {
            GitRowKind::Staged { idx, .. } if want_staged && *idx == want_idx => return pos,
            GitRowKind::Unstaged { idx, .. } if !want_staged && *idx == want_idx => return pos,
            _ => {}
        }
    }
    0
}

pub(super) fn render(frame: &mut Frame, area: Rect, model: &Model) {
    let l = git_layout(model, area);
    let width = area.width as usize;
    let th = &model.theme;

    // Branch row at the very top, with a Refresh button pinned to the right.
    if l.branch_shown {
        let refresh_icon = if model.ascii_icons { "[R]" } else { " ⟳ " };
        let name_w = width.saturating_sub(refresh_icon.chars().count());
        let name = model.sidebar.git.branch.clone().unwrap_or_default();
        let branch = Paragraph::new(Line::from(vec![
            Span::styled(format!("{name:<name_w$}"), Style::new().fg(th.fg)),
            Span::styled(refresh_icon.to_string(), Style::new().fg(th.accent)),
        ]))
        .style(Style::new().bg(th.bg_alt));
        frame.render_widget(
            branch,
            Rect {
                y: l.branch_y,
                height: 1,
                ..area
            },
        );
    }

    // Scrollable change list.
    let mut lines: Vec<Line> = Vec::new();
    for r in l.rows.iter().skip(l.offset).take(l.list_h as usize) {
        lines.push(git_row_line(model, r, width));
    }
    let list_area = Rect {
        y: l.content_y,
        height: l.list_h,
        ..area
    };
    let p = Paragraph::new(lines).style(Style::new().bg(th.bg_alt));
    frame.render_widget(p, list_area);

    if l.has_box {
        render_commit_box(frame, area, &l, model, width);
        render_git_actions(frame, area, &l, model, width);
    }
}

/// The fetch / pull / push button row at the bottom of the panel.
fn render_git_actions(frame: &mut Frame, area: Rect, l: &GitLayout, model: &Model, width: usize) {
    let th = &model.theme;
    let g = &model.sidebar.git;
    let (w0, w1, w2) = action_segments(width);

    let up = if model.ascii_icons { "" } else { "↑" };
    let down = if model.ascii_icons { "" } else { "↓" };
    let fetch_label = "Fetch".to_string();
    let pull_label = if g.behind > 0 {
        format!("Pull {down}{}", g.behind)
    } else {
        "Pull".to_string()
    };
    let push_label = if g.ahead > 0 {
        format!("Push {up}{}", g.ahead)
    } else {
        "Push".to_string()
    };

    // Fetch needs a remote; Pull needs an upstream; Push needs something to push.
    let fetch_enabled = g.has_remote;
    let pull_enabled = g.has_upstream;
    let push_enabled = g.can_push();

    // Distinct backgrounds separate the cells; widths match the thirds used by
    // `action_at_col` so the visuals and hit-testing line up exactly.
    let zone = g.zone;
    let cell = |label: &str, enabled: bool, w: usize, z: GitZone| -> Span<'static> {
        Span::styled(
            format!("{label:^w$}"),
            button_style(th, enabled, zone == z),
        )
    };

    let gap = || Span::styled(" ", Style::new().bg(th.bg_alt));
    let spans = vec![
        cell(&fetch_label, fetch_enabled, w0, GitZone::Fetch),
        gap(),
        cell(&pull_label, pull_enabled, w1, GitZone::Pull),
        gap(),
        cell(&push_label, push_enabled, w2, GitZone::Push),
    ];
    let p = Paragraph::new(Line::from(spans)).style(Style::new().bg(th.bg_alt));
    frame.render_widget(
        p,
        Rect {
            y: l.actions_y,
            height: 1,
            ..area
        },
    );
}

/// Converts a single git content row into a drawable `Line`.
fn git_row_line(model: &Model, kind: &GitRowKind, width: usize) -> Line<'static> {
    let g = &model.sidebar.git;
    let th = &model.theme;
    match kind {
        GitRowKind::StagedHeader => header_line(format!(" STAGED ({})", g.staged.len()), th),
        GitRowKind::ChangesHeader => header_line(format!(" CHANGES ({})", g.unstaged.len()), th),
        GitRowKind::Info(s) => {
            Line::from(Span::styled(format!(" {s}"), Style::new().fg(th.fg_dim)))
        }
        GitRowKind::StageAll => action_line(" Stage All", "+", th.git_added, th, width),
        GitRowKind::UnstageAll => action_line(" Unstage All", "-", th.git_deleted, th, width),
        GitRowKind::Separator => {
            Line::from(Span::styled("─".repeat(width), Style::new().fg(th.border)))
                .style(Style::new().bg(th.bg_alt))
        }
        GitRowKind::Dir { name, depth } => Line::from(vec![
            Span::raw("  ".repeat(*depth)),
            Span::styled("▾ ", Style::new().fg(th.fg_dim)),
            Span::styled(name.clone(), Style::new().fg(th.fg)),
        ])
        .style(Style::new().bg(th.bg_alt)),
        GitRowKind::Staged { idx, depth } => {
            let sel = g.selected == *idx;
            entry_line(model, &g.staged[*idx], true, sel, *depth, width)
        }
        GitRowKind::Unstaged { idx, depth } => {
            let sel = g.selected == g.staged.len() + *idx;
            entry_line(model, &g.unstaged[*idx], false, sel, *depth, width)
        }
        GitRowKind::Hint => {
            // Drop the labels the panel is too narrow for rather than letting the
            // line wrap into the next row.
            let full = " Stage/Unstage: a, Revert: r";
            let text = if full.chars().count() <= width {
                full
            } else if " a: stage, r: revert".chars().count() <= width {
                " a: stage, r: revert"
            } else {
                ""
            };
            hint_line(text, th)
        }
        GitRowKind::HistoryHeader => header_line(" HISTORY".to_string(), th),
        GitRowKind::HistoryHint => hint_line(" Show diff: Enter", th),
        GitRowKind::Commit { idx } => {
            let sel = g.selected == g.changes_len() + *idx;
            commit_line(&g.history[*idx], th, width, sel)
        }
    }
}

/// A previous-commit row: short hash (dim) + summary, trimmed to the panel width.
fn commit_line(
    c: &crate::services::git::GitCommit,
    th: &crate::core::theme::Theme,
    width: usize,
    selected: bool,
) -> Line<'static> {
    let hash = format!(" {} ", c.hash);
    let avail = width.saturating_sub(hash.chars().count());
    let summary = if c.summary.chars().count() > avail {
        let keep = avail.saturating_sub(1);
        format!("{}…", c.summary.chars().take(keep).collect::<String>())
    } else {
        c.summary.clone()
    };
    let bg = if selected { th.selected_bg() } else { th.bg_alt };
    Line::from(vec![
        Span::styled(hash, Style::new().fg(th.accent)),
        Span::styled(summary, Style::new().fg(th.fg_dim)),
    ])
    .style(Style::new().bg(bg))
}

/// Bulk action row ("Stage All +" / "Unstage All -"); icon on the right at width-2 (aligned with entry).
fn action_line(
    label: &str,
    icon: &str,
    icon_color: ratatui::style::Color,
    th: &crate::core::theme::Theme,
    width: usize,
) -> Line<'static> {
    let w = width.saturating_sub(4);
    let field = format!("{label:<w$}");
    Line::from(vec![
        Span::styled(field, Style::new().fg(th.accent)),
        Span::raw("  "),
        Span::styled(icon.to_string(), Style::new().fg(icon_color)),
        Span::raw(" "),
    ])
    .style(Style::new().bg(th.bg_alt))
}

/// A dim italic keyboard hint row.
fn hint_line(text: &str, th: &crate::core::theme::Theme) -> Line<'static> {
    Line::from(Span::styled(
        text.to_string(),
        Style::new().fg(th.fg_dim).add_modifier(Modifier::ITALIC),
    ))
    .style(Style::new().bg(th.bg_alt))
}

fn header_line(text: String, th: &crate::core::theme::Theme) -> Line<'static> {
    Line::from(Span::styled(
        text,
        Style::new().fg(th.fg_dim).add_modifier(Modifier::BOLD),
    ))
    .style(Style::new().bg(th.bg_alt))
}

/// A git change row shown in the tree: `[<indent> M name       + ↺]` (unstaged) /
/// `[<indent> M name   -]` (staged). Only the file name is drawn; the directory
/// path is conveyed by the tree indent and parent `Dir` rows.
fn entry_line(
    model: &Model,
    e: &GitEntry,
    staged: bool,
    selected: bool,
    depth: usize,
    width: usize,
) -> Line<'static> {
    let th = &model.theme;
    let color = match e.state {
        GitState::Added => th.git_added,
        GitState::Modified => th.git_modified,
        GitState::Deleted => th.git_deleted,
        GitState::Untracked => th.git_untracked,
        _ => th.fg_dim,
    };
    let indent = "  ".repeat(depth);
    let name = e.rel.rsplit('/').next().unwrap_or(e.rel.as_str());
    let file_icon = if model.ascii_icons { "•" } else { "" };
    // icon (FILE_ICON_W) + indent + prefix (3) + name + suffix = width.
    // Unstaged suffix is one wider: "↺  +" (revert, gap, stage) vs staged "  -".
    let suffix_w = if staged { 3 } else { 4 };
    let avail = width.saturating_sub(FILE_ICON_W + indent.len() + 3 + suffix_w);
    let name_field = format!("{:<avail$}", fit_path(name, avail));
    let line_bg = if selected {
        th.selected_bg()
    } else {
        th.bg_alt
    };

    // Leftmost: a file icon that opens the plain file (not the diff view).
    let mut spans = vec![
        Span::styled(file_icon.to_string(), Style::new().fg(th.fg_dim)),
        Span::raw(indent),
        Span::styled(format!(" {} ", e.state.short()), Style::new().fg(color)),
        Span::styled(name_field, Style::new().fg(th.fg)),
    ];

    if staged {
        spans.push(Span::from("  "));
        spans.push(Span::styled(
            "-".to_string(),
            Style::new().fg(th.git_deleted),
        ));
    } else {
        let revert = if model.ascii_icons { "x" } else { "↺ " };

        spans.push(Span::styled(
            revert.to_string(),
            Style::new().fg(th.git_deleted),
        ));
        spans.push(Span::raw(" "));
        spans.push(Span::styled("+".to_string(), Style::new().fg(th.git_added)));
    }
    Line::from(spans).style(Style::new().bg(line_bg))
}

/// Fits the path into `max` characters; if too long, trims from the front and prepends `…` (the file name stays visible).
fn fit_path(rel: &str, max: usize) -> String {
    let len = rel.chars().count();
    if len <= max {
        return rel.to_string();
    }
    if max <= 1 {
        return "…".to_string();
    }
    let tail: String = rel.chars().skip(len - (max - 1)).collect();
    format!("…{tail}")
}

/// The commit message box (4 rows) and the commit button, below the branch row.
fn render_commit_box(frame: &mut Frame, area: Rect, l: &GitLayout, model: &Model, width: usize) {
    let th = &model.theme;
    let g = &model.sidebar.git;
    let focused = model.focus == Focus::GitCommit;

    // Multi-line, vertically-scrolling commit input (shared text-input widget).
    frame.render_widget(
        TextInput::new(&g.commit, th)
            .placeholder("Message")
            .focused(focused)
            .pad(1),
        Rect {
            y: l.input_top,
            height: COMMIT_INPUT_H,
            ..area
        },
    );

    // Uncommit (left) and Commit (right), always shown, enabled per state. Both
    // share the Fetch/Pull button color scheme.
    let can_commit = !g.staged.is_empty() && !g.commit.content().trim().is_empty();
    let can_undo = g.can_undo_commit();

    // Same colors as the fetch/pull/push cells.
    let zone = g.zone;
    let cell = |label: &str, enabled: bool, z: GitZone| -> Span<'static> {
        Span::styled(
            label.to_string(),
            button_style(th, enabled, zone == z).add_modifier(Modifier::BOLD),
        )
    };

    let uncommit_w = UNCOMMIT_LABEL.chars().count();
    let commit_w = COMMIT_LABEL.chars().count();
    let filler = width.saturating_sub(uncommit_w + commit_w).max(1);
    let spans = vec![
        cell(UNCOMMIT_LABEL, can_undo, GitZone::Uncommit),
        Span::styled(" ".repeat(filler), Style::new().bg(th.bg_alt)),
        cell(COMMIT_LABEL, can_commit, GitZone::Commit),
    ];

    let btn = Paragraph::new(Line::from(spans)).style(Style::new().bg(th.bg_alt));
    frame.render_widget(
        btn,
        Rect {
            y: l.button_y,
            height: 1,
            ..area
        },
    );
}

/// Target of a mouse click in the Git panel.
pub enum GitHit {
    /// Open the change row (combined index).
    Entry(usize),
    Stage(String),
    Unstage(String),
    Revert(String),
    StageAll,
    UnstageAll,
    CommitInput,
    CommitButton,
    UndoLastCommit,
    Fetch,
    Pull,
    Push,
    /// Reload git status (the Refresh button on the branch row).
    Refresh,
    /// Open the plain file (not the diff) — the file icon on a change row.
    OpenFile(String),
}

/// Width of the Refresh button at the right end of the branch row.
const REFRESH_W: usize = 3;

/// Converts the mouse (x, y) into a Git panel target. `area` is the full sidebar area.
pub fn git_hit(model: &Model, area: Rect, x: u16, y: u16) -> Option<GitHit> {
    let area = panel_area(area);
    let l = git_layout(model, area);
    let g = &model.sidebar.git;
    // Refresh button: right end of the branch row.
    if l.branch_shown && y == l.branch_y {
        let rel = x.saturating_sub(area.x) as usize;
        if rel >= (area.width as usize).saturating_sub(REFRESH_W) {
            return Some(GitHit::Refresh);
        }
        return None;
    }
    if l.has_box {
        if y == l.actions_y {
            return match action_at_col(area, x)? {
                GitAction::Fetch => Some(GitHit::Fetch),
                GitAction::Pull => Some(GitHit::Pull),
                GitAction::Push => Some(GitHit::Push),
            };
        }
        if y == l.button_y {
            let rel = x.saturating_sub(area.x) as usize;
            let width = area.width as usize;
            let uncommit_w = UNCOMMIT_LABEL.chars().count();
            let commit_w = COMMIT_LABEL.chars().count();
            // Uncommit is left-aligned, Commit is right-aligned; the gap is inert.
            if rel < uncommit_w {
                return Some(GitHit::UndoLastCommit);
            }
            if rel >= width.saturating_sub(commit_w) {
                return Some(GitHit::CommitButton);
            }
            return None;
        }
        if y >= l.input_top && y < l.input_top + COMMIT_INPUT_H {
            return Some(GitHit::CommitInput);
        }
        if y < l.content_y {
            return None; // branch / blanks in the fixed top block
        }
    }
    if y < l.content_y || y >= l.content_y + l.list_h {
        return None;
    }
    let row_idx = l.offset + (y - l.content_y) as usize;
    let kind = l.rows.get(row_idx)?;
    let col = x.saturating_sub(area.x) as usize;
    let width = area.width as usize;
    let wide = width >= 8;
    match kind {
        GitRowKind::Staged { idx, .. } => {
            let e = g.staged.get(*idx)?;
            // The file_icon glyph is 1 col but reserves FILE_ICON_W (2), so the
            // suffix ends one short of the right edge: rendered "  -" puts "-" at
            // width-2, with width-1 left blank.
            if col < FILE_ICON_W {
                Some(GitHit::OpenFile(e.rel.clone()))
            } else if wide && col == width - 2 {
                Some(GitHit::Unstage(e.rel.clone()))
            } else {
                Some(GitHit::Entry(*idx))
            }
        }
        GitRowKind::StageAll => Some(GitHit::StageAll),
        GitRowKind::UnstageAll => Some(GitHit::UnstageAll),
        GitRowKind::Unstaged { idx, .. } => {
            let e = g.unstaged.get(*idx)?;
            // Rendered suffix "↺ " + " " + "+" ends at width-2 (see Staged note):
            // ↺ at width-5, its space width-4, gap width-3, "+" at width-2, width-1
            // blank. Revert owns width-5..=width-4; the gap at width-3 is inert.
            if col < FILE_ICON_W {
                Some(GitHit::OpenFile(e.rel.clone()))
            } else if wide && col == width - 2 {
                Some(GitHit::Stage(e.rel.clone()))
            } else if wide && (col == width - 5 || col == width - 4) {
                Some(GitHit::Revert(e.rel.clone()))
            } else {
                Some(GitHit::Entry(g.staged.len() + *idx))
            }
        }
        // A history row opens the commit's patch; its combined index sits after
        // the change rows.
        GitRowKind::Commit { idx } => Some(GitHit::Entry(g.changes_len() + *idx)),
        _ => None,
    }
}
