//! Application state (the `Model` of the Elm Architecture).

use std::path::PathBuf;

use crate::app::cmd::Cmd;
use crate::core::buffer::{Buffer, Cursor};
use crate::core::filetree::FileTree;
use crate::core::highlight::{self, HlLine};
use crate::core::text_input::TextInputState;
use crate::core::theme::Theme;
use crate::services::git::{CommitRow, GitCommit, GitEntry, GutterKind};
use crate::services::pty::PtySession;
use crate::services::search::SearchMatch;

/// Left activity bar panels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Panel {
    Files,
    Search,
    Git,
    Extensions,
    /// Theme picker — right below Extensions.
    Themes,
    /// Editor preferences (format on save, etc.) — right below Themes.
    Settings,
}

impl Panel {
    pub const ALL: [Panel; 6] = [
        Panel::Files,
        Panel::Search,
        Panel::Git,
        Panel::Extensions,
        Panel::Themes,
        Panel::Settings,
    ];

    pub fn icon(&self) -> &'static str {
        match self {
            Panel::Files => "\u{f4a5}", // file
            Panel::Search => "\u{f002}",
            Panel::Git => "\u{f419}",
            Panel::Extensions => "\u{f12e}",
            Panel::Themes => "\u{f1fc}", // palette
            Panel::Settings => "\u{f013}", // gear
        }
    }

    pub fn ascii_icon(&self) -> &'static str {
        match self {
            Panel::Files => "Fil",
            Panel::Search => "Src",
            Panel::Git => "Git",
            Panel::Extensions => "Ext",
            Panel::Themes => "Thm",
            Panel::Settings => "Set",
        }
    }

    pub fn title(&self) -> &'static str {
        match self {
            Panel::Files => "EXPLORER",
            Panel::Search => "SEARCH",
            Panel::Git => "SOURCE CONTROL",
            Panel::Extensions => "EXTENSIONS",
            Panel::Themes => "THEMES",
            Panel::Settings => "SETTINGS",
        }
    }
}

/// Editor preferences applied at save time. Edited via `config.toml`, not the UI.
pub struct SettingsState {
    /// Master switch: run the enabled format actions when saving.
    pub format_on_save: bool,
    /// Strip trailing spaces/tabs from each line on save (when format_on_save).
    pub trim_trailing_whitespace: bool,
    /// Ensure the file ends with a single newline on save (when format_on_save).
    pub insert_final_newline: bool,
    /// Show LSP error/warning messages inline at the end of their line.
    pub inline_diagnostics: bool,
}

impl Default for SettingsState {
    fn default() -> Self {
        SettingsState {
            format_on_save: false,
            trim_trailing_whitespace: true,
            insert_final_newline: true,
            inline_diagnostics: true,
        }
    }
}

/// State of the theme picker panel.
pub struct ThemesState {
    /// Names of the loaded syntect themes.
    pub names: Vec<String>,
    /// Index of the theme selected via keyboard/mouse.
    pub selected: usize,
}

impl Default for ThemesState {
    fn default() -> Self {
        let names = highlight::theme_names();
        let selected = names
            .iter()
            .position(|n| n == highlight::DEFAULT_THEME)
            .unwrap_or(0);
        ThemesState { names, selected }
    }
}

/// Focus — the target that keyboard events are routed to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Focus {
    Editor,
    Terminal,
    Sidebar,
    SearchInput,
    /// The commit message input in the Git panel.
    GitCommit,
    /// The in-editor find/replace widget.
    Find,
}

/// Which input field of the in-editor find widget is active.
#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
pub enum FindField {
    #[default]
    Query,
    Replace,
}

/// In-editor find / find-and-replace widget (floats over the top-right of the editor).
#[derive(Default)]
pub struct FindState {
    pub open: bool,
    /// Whether the replace row (input + Replace/Replace All buttons) is shown.
    pub replace_mode: bool,
    pub query: TextInputState,
    pub replace: TextInputState,
    pub field: FindField,
    /// Match ranges in the active buffer, as absolute [start, end) character indices.
    pub matches: Vec<(usize, usize)>,
    /// Index of the current match within `matches`.
    pub current: Option<usize>,
}

impl FindState {
    /// "cur/total" indicator (1-based); "0/0" when there are no matches.
    pub fn count_label(&self) -> String {
        let total = self.matches.len();
        let cur = self.current.map(|i| i + 1).unwrap_or(0);
        format!("{cur}/{total}")
    }
}

/// Mouse drag target (panel resizing / text selection).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DragTarget {
    SidebarBorder,
    TerminalBorder,
    EditorSelect,
    /// Dragging the editor scrollbar thumb.
    Scrollbar,
    /// Dragging inside the terminal area to select text.
    TerminalSelect,
    /// Dragging the terminal scrollback scrollbar thumb.
    TerminalScrollbar,
}

pub struct Tab {
    pub buffer: Buffer,
    /// The file's content at git HEAD, for the change gutter. Loaded async.
    pub head_text: Option<String>,
    /// Opened from the Git panel as a diff: changed lines get a colored background.
    pub diff_mode: bool,
    /// Set for files that could not be opened (binary / unreadable): the editor
    /// shows this message centered instead of the (empty) buffer, and editing is
    /// disabled so the file is never overwritten.
    pub notice: Option<String>,
    /// Tab bar label override, for tabs that are not a file on disk (a commit
    /// patch). `None` = derive it from the buffer's file name.
    pub label: Option<String>,
    /// Generated content that must never be edited or saved (a commit patch).
    pub read_only: bool,
    /// What each buffer line of a commit's diff view is: the gutter shows the
    /// file's own line numbers instead of this view's row count, and the heading
    /// rows are styled rather than highlighted as code. Empty for a normal file.
    pub commit_rows: Vec<CommitRow>,
}

impl Tab {
    pub fn new(buffer: Buffer) -> Self {
        Tab {
            buffer,
            head_text: None,
            diff_mode: false,
            notice: None,
            label: None,
            read_only: false,
            commit_rows: Vec::new(),
        }
    }

    /// A read-only diff tab for a history commit, titled "<hash> diff".
    ///
    /// It is an ordinary diff-mode tab: the commit's side of the changes is the
    /// buffer and the parent's side is the "HEAD" text, so the same machinery that
    /// paints an uncommitted change paints this one — added lines on a green
    /// background, removed lines woven in red.
    ///
    /// The buffer gets a synthetic `<hash>.<ext>` path, never written (the tab is
    /// read-only): it only picks the syntax the code is highlighted with, taken
    /// from the file the commit changed most.
    pub fn commit_diff(hash: &str, diff: &crate::services::git::CommitDiff) -> Self {
        let name = match &diff.syntax_ext {
            Some(ext) => format!("{hash}.{ext}"),
            None => hash.to_string(),
        };
        let mut tab = Tab::new(Buffer::new(Some(std::path::PathBuf::from(name)), &diff.new));
        tab.head_text = Some(diff.old.clone());
        tab.commit_rows = diff.rows.clone();
        tab.diff_mode = true;
        tab.label = Some(format!("{hash} diff"));
        tab.read_only = true;
        tab
    }

    /// A read-only tab that just shows an error message (binary / unreadable file).
    pub fn notice(path: std::path::PathBuf, message: String) -> Self {
        let mut tab = Tab::new(Buffer::new(Some(path), ""));
        tab.notice = Some(message);
        tab
    }

    /// Tab bar label: file name, with a "(diff)" suffix for diff-mode tabs.
    pub fn title(&self) -> String {
        if let Some(label) = &self.label {
            return label.clone();
        }
        let name = self.buffer.display_name();
        if self.diff_mode {
            format!("{name} (diff)")
        } else {
            name
        }
    }
}

/// The active input field in the search panel.
#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
pub enum SearchField {
    #[default]
    Query,
    Replace,
}

#[derive(Default)]
pub struct SearchState {
    pub query: TextInputState,
    pub replace: TextInputState,
    /// Should the query be interpreted as a regex?
    pub use_regex: bool,
    /// Case-sensitive matching when true (default: case-insensitive).
    pub match_case: bool,
    /// Search .gitignore'd and hidden (dot) files when true (default: skip them).
    pub search_hidden: bool,
    /// Which field keyboard input goes to.
    pub field: SearchField,
    pub results: Vec<SearchMatch>,
    pub selected: usize,
}


/// Keyboard focus zone inside the Git panel, cycled with Tab.
///
/// The zone decides what Enter activates and which widget is drawn highlighted.
/// `Message` is the one zone that also changes the app-level [`Focus`] (to
/// `Focus::GitCommit`, so typing reaches the commit input); every other zone
/// keeps `Focus::Sidebar`.
#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
pub enum GitZone {
    /// The commit message box.
    Message,
    Fetch,
    Pull,
    Push,
    Uncommit,
    Commit,
    /// The change / history list at the bottom of the panel.
    #[default]
    Files,
}

impl GitZone {
    /// Tab order, top to bottom, wrapping back to the start.
    const ORDER: [GitZone; 7] = [
        GitZone::Message,
        GitZone::Fetch,
        GitZone::Pull,
        GitZone::Push,
        GitZone::Uncommit,
        GitZone::Commit,
        GitZone::Files,
    ];

    /// Steps `delta` places through [`GitZone::ORDER`], wrapping at both ends.
    pub fn step(self, delta: isize) -> GitZone {
        let n = GitZone::ORDER.len() as isize;
        let cur = GitZone::ORDER.iter().position(|z| *z == self).unwrap_or(0) as isize;
        GitZone::ORDER[(cur + delta).rem_euclid(n) as usize]
    }
}

#[derive(Default)]
pub struct GitStatus {
    pub branch: Option<String>,
    pub staged: Vec<GitEntry>,
    pub unstaged: Vec<GitEntry>,
    pub is_repo: bool,
    /// Keyboard selection: index into the combined [staged..., unstaged...] list.
    pub selected: usize,
    /// The commit message input box.
    pub commit: TextInputState,
    /// Commits the local branch is ahead of its upstream.
    pub ahead: usize,
    /// Commits the local branch is behind its upstream.
    pub behind: usize,
    /// Whether the current branch has a configured upstream.
    pub has_upstream: bool,
    /// Whether the repository has at least one remote configured.
    pub has_remote: bool,
    /// Recent commits (newest first) shown under the HISTORY heading.
    pub history: Vec<GitCommit>,
    /// Which part of the panel the keyboard is on (cycled with Tab).
    pub zone: GitZone,
}

impl GitStatus {
    /// Total number of keyboard-navigable items: the changes followed by the
    /// history commits, in the one combined index the selection uses.
    pub fn nav_len(&self) -> usize {
        self.staged.len() + self.unstaged.len() + self.history.len()
    }

    /// Number of change rows — the combined index where the history starts.
    pub fn changes_len(&self) -> usize {
        self.staged.len() + self.unstaged.len()
    }

    /// The history commit at a combined index, or `None` if it names a change row.
    pub fn commit_at(&self, idx: usize) -> Option<&GitCommit> {
        self.history.get(idx.checked_sub(self.changes_len())?)
    }

    /// Whether there is anything to push: a remote must exist and the branch is
    /// either ahead of its upstream or not yet published (no upstream).
    pub fn can_push(&self) -> bool {
        self.is_repo
            && self.branch.is_some()
            && self.has_remote
            && (self.ahead > 0 || !self.has_upstream)
    }

    /// Whether the last commit can be undone: a repo with a local commit that has
    /// not been pushed (ahead of its upstream, or no upstream configured yet).
    pub fn can_undo_commit(&self) -> bool {
        self.is_repo && self.branch.is_some() && (self.ahead > 0 || !self.has_upstream)
    }

    /// Returns the item at the combined index and whether it is staged.
    pub fn entry_at(&self, idx: usize) -> Option<(&GitEntry, bool)> {
        if idx < self.staged.len() {
            self.staged.get(idx).map(|e| (e, true))
        } else {
            self.unstaged.get(idx - self.staged.len()).map(|e| (e, false))
        }
    }
}

pub struct Sidebar {
    pub active: Panel,
    pub files: FileTree,
    pub git: GitStatus,
    pub search: SearchState,
    pub themes: ThemesState,
    pub settings: SettingsState,
    /// Selected row in the Settings panel (index into its action list).
    pub settings_selected: usize,
}

/// General-purpose modal dialog kind.
// The Info kind is not used yet; the general API is for future use.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum DialogKind {
    /// Confirmation dialog (Yes / No).
    Ask,
    /// Information dialog (OK only).
    #[allow(dead_code)]
    Info,
    /// Text input (OK / Cancel).
    Input,
}

/// Action to perform when the dialog is confirmed.
#[derive(Clone)]
pub enum DialogAction {
    /// No action (e.g. an information dialog).
    #[allow(dead_code)]
    None,
    /// Revert the working-tree change for the given path.
    GitRevert(String),
    /// Create a new file with the entered name inside the given directory.
    NewFile(PathBuf),
    /// Create a new directory with the entered name inside the given directory.
    NewFolder(PathBuf),
    /// Rename the given path to the entered name (kept in the same directory).
    Rename(PathBuf),
    /// Delete the given path (recursively for a directory).
    Delete(PathBuf),
    /// Close the tab at the given index (user confirmed the dirty-tab dialog).
    CloseTab(usize, String),
    /// Overwrite `keybindings.toml` with the built-in defaults.
    ResetKeybindings,
    /// Overwrite `config.toml` with the seeded defaults.
    ResetConfig,
}

/// Modal dialog opened in the center of the screen. Captures all input while open.
pub struct Dialog {
    pub kind: DialogKind,
    pub title: String,
    pub message: String,
    /// Text entered for the `Input` kind (with caret, editable).
    pub input: TextInputState,
    /// Button selection: 0 = confirm, 1 = cancel (Ask/Input).
    pub selected: usize,
    pub action: DialogAction,
}

impl Dialog {
    pub fn ask(title: String, message: String, action: DialogAction) -> Self {
        Dialog {
            kind: DialogKind::Ask,
            title,
            message,
            input: TextInputState::default(),
            selected: 0,
            action,
        }
    }

    /// General dialog constructor for future use.
    #[allow(dead_code)]
    pub fn info(title: String, message: String) -> Self {
        Dialog {
            kind: DialogKind::Info,
            title,
            message,
            input: TextInputState::default(),
            selected: 0,
            action: DialogAction::None,
        }
    }

    pub fn input(title: String, message: String, initial: String, action: DialogAction) -> Self {
        let mut input = TextInputState::default();
        input.set_content(initial);
        Dialog {
            kind: DialogKind::Input,
            title,
            message,
            input,
            selected: 0,
            action,
        }
    }
}

/// An entry of the file-tree context menu.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MenuItem {
    NewFile,
    NewFolder,
    Rename,
    Delete,
}

impl MenuItem {
    /// The items shown for a tree row, in order.
    pub const ALL: [MenuItem; 4] = [
        MenuItem::NewFile,
        MenuItem::NewFolder,
        MenuItem::Rename,
        MenuItem::Delete,
    ];

    pub fn label(&self) -> &'static str {
        match self {
            MenuItem::NewFile => "New File",
            MenuItem::NewFolder => "New Folder",
            MenuItem::Rename => "Rename",
            MenuItem::Delete => "Delete",
        }
    }

    /// The keyboard shortcut for the same action (also listed in the status bar).
    pub fn shortcut(&self) -> &'static str {
        match self {
            MenuItem::NewFile => "Ctrl+N",
            MenuItem::NewFolder => "Ctrl+Shift+N",
            MenuItem::Rename => "F2",
            MenuItem::Delete => "Del",
        }
    }
}

/// Context menu opened by right-clicking a file-tree row. Captures all input
/// while open, like `Dialog`.
pub struct ContextMenu {
    /// The visible tree row the menu was opened on.
    pub row: usize,
    pub selected: usize,
    /// Top-left corner requested by the click; clamped to the screen on render.
    pub x: u16,
    pub y: u16,
}

impl ContextMenu {
    pub fn new(row: usize, x: u16, y: u16) -> Self {
        ContextMenu {
            row,
            selected: 0,
            x,
            y,
        }
    }
}

pub struct TerminalState {
    pub parser: vt100::Parser,
    pub session: Option<PtySession>,
    pub rows: u16,
    pub cols: u16,
    /// The PTY spawn Cmd was sent but the session is not ready yet.
    pub spawn_requested: bool,
    /// Scrollback view offset from the live bottom (0 = following the bottom,
    /// higher = further back in history). Mirrors vt100's internal position.
    pub scroll_offset: usize,
    /// Total scrollback rows currently held by vt100 (the max scroll offset).
    /// Used to size the scrollbar thumb.
    pub scrollback_lines: usize,
    /// Active text selection in visible-grid coordinates:
    /// (start_row, start_col, end_row, end_col).
    pub selection: Option<(u16, u16, u16, u16)>,
}

impl TerminalState {
    fn new() -> Self {
        TerminalState {
            parser: vt100::Parser::new(24, 80, 2000),
            session: None,
            rows: 24,
            cols: 80,
            spawn_requested: false,
            scroll_offset: 0,
            scrollback_lines: 0,
            selection: None,
        }
    }

    pub fn resize(&mut self, rows: u16, cols: u16) {
        if rows == 0 || cols == 0 {
            return;
        }
        if rows != self.rows || cols != self.cols {
            self.rows = rows;
            self.cols = cols;
            self.parser.set_size(rows, cols);
            if let Some(s) = self.session.as_ref() {
                s.resize(rows, cols);
            }
        }
        self.sync_scroll_bounds();
    }

    /// Re-reads the total scrollback vt100 holds and re-applies the view
    /// position. Call after every `parser.process()`. When the user is scrolled
    /// back into history, the view stays anchored to the same rows as new output
    /// pushes older rows further up; when following the bottom it keeps
    /// following. vt100 clamps the offset, so the read-back value is authoritative.
    pub fn sync_scroll_bounds(&mut self) {
        let prev_total = self.scrollback_lines;
        // Probe the maximum offset (== total scrollback rows).
        self.parser.set_scrollback(usize::MAX);
        self.scrollback_lines = self.parser.screen().scrollback();
        // Keep the same history in view as new rows are appended.
        let mut target = self.scroll_offset;
        if target > 0 {
            target += self.scrollback_lines.saturating_sub(prev_total);
        }
        self.parser.set_scrollback(target);
        self.scroll_offset = self.parser.screen().scrollback();
    }

    /// Scrolls the view by `delta` rows (positive = back into history). Syncs
    /// vt100 and stores the clamped offset.
    pub fn scroll_by(&mut self, delta: isize) {
        let new = (self.scroll_offset as isize + delta).max(0) as usize;
        self.parser.set_scrollback(new);
        self.scroll_offset = self.parser.screen().scrollback();
    }

    /// Jumps the view to an absolute scrollback offset (0 = live bottom).
    pub fn scroll_to(&mut self, offset: usize) {
        self.parser.set_scrollback(offset);
        self.scroll_offset = self.parser.screen().scrollback();
    }

    /// Extracts the selected text from the currently visible grid.
    pub fn selected_text(&self) -> String {
        let Some((r1, c1, r2, c2)) = self.selection else {
            return String::new();
        };
        // Order by row, then column, so a bottom-up drag copies in reading order.
        let ((min_r, min_c), (max_r, max_c)) = if (r1, c1) <= (r2, c2) {
            ((r1, c1), (r2, c2))
        } else {
            ((r2, c2), (r1, c1))
        };
        self.parser
            .screen()
            .contents_between(min_r, min_c, max_r, max_c)
    }
}

pub struct LayoutState {
    pub sidebar_width: u16,
    pub terminal_height: u16,
    pub sidebar_open: bool,
    pub terminal_open: bool,
}

impl Default for LayoutState {
    fn default() -> Self {
        LayoutState {
            sidebar_width: 30,
            terminal_height: 12,
            sidebar_open: true,
            terminal_open: false,
        }
    }
}

/// Running language servers, keyed by language id.
#[derive(Default)]
pub struct LspState {
    /// Initialized-or-initializing servers we hold a handle for.
    pub sessions: std::collections::HashMap<String, crate::services::lsp::LspHandle>,
    /// Languages whose server spawn is in flight (prevents a double-spawn).
    pub starting: std::collections::HashSet<String>,
    /// Languages whose server finished the `initialize` handshake.
    pub initialized: std::collections::HashSet<String>,
}

/// The open completion popup: items from the server plus selection + the range
/// they replace. `requested_version`/`tab_index` discard a stale response.
pub struct CompletionState {
    pub items: Vec<crate::services::lsp::CompletionItem>,
    pub selected: usize,
    /// Start of the identifier prefix being completed (replaced on accept).
    pub anchor: Cursor,
    /// The tab the popup belongs to (guards accept against a tab switch).
    pub tab_index: usize,
}

/// An outstanding LSP format request: whether to write the file once its edits
/// apply (format-on-save). Staleness is guarded separately by the request token.
pub struct PendingFormat {
    pub save_after: bool,
}

/// A diagnostic in buffer char coordinates (converted from LSP on receipt).
#[derive(Clone)]
pub struct Diagnostic {
    pub line: usize,
    pub col_start: usize,
    pub col_end: usize,
    pub severity: crate::services::lsp::Severity,
    pub message: String,
}


/// A selectable entry in the quickbar (command palette), opened with Ctrl+P.
/// Entries mix workspace files, workspace directories, and built-in commands.
#[derive(Clone)]
pub enum QuickbarItem {
 /// Open this file in the editor. `rel` is the workspace-relative path used
 /// for display and prefix filtering (what the user sees and types against).
 File { path: PathBuf, rel: String },
 /// Reveal / expand this directory in the file sidebar. (Kept ready for a
 /// future "open folder" entry; the workspace file scan only returns files.)
 #[allow(dead_code)]
 Dir { path: PathBuf, rel: String },
 /// Create a new file inside the workspace root (opens the name dialog).
 NewFile,
 /// Create a new folder inside the workspace root (opens the name dialog).
 NewFolder,
 /// Open the given sidebar panel.
 Panel(Panel),
}

impl QuickbarItem {
 /// A one-char marker rendered before each entry so its kind is clear at a
 /// glance: file, directory, new-entry, or command.
 pub fn marker(&self) -> char {
 match self {
 QuickbarItem::File { .. } => 'F',
 QuickbarItem::Dir { .. } => 'D',
 QuickbarItem::NewFile => '+',
 QuickbarItem::NewFolder => '+',
 QuickbarItem::Panel(_) => '>',
 }
 }

 /// The label shown for the entry. For files/dirs this is the workspace-
 /// relative path; for commands a human title.
 pub fn label(&self) -> String {
 match self {
 QuickbarItem::File { rel, .. } | QuickbarItem::Dir { rel, .. } => rel.clone(),
 QuickbarItem::NewFile => "New File".into(),
 QuickbarItem::NewFolder => "New Folder".into(),
 QuickbarItem::Panel(p) => format!("Open panel: {}", p.title()),
 }
 }

 /// The query text that selects this entry (what the filter matches).
 pub fn filter_text(&self) -> String {
 self.label().to_lowercase()
 }
}

/// The quickbar overlay: an input query up top and a filtered list below.
/// Sort/filter happens in `update` (never here); this is pure state.
pub struct QuickbarState {
 /// The query being typed; filtered against entry `filter_text`.
 pub input: TextInputState,
 /// Every workspace file (from `Msg::FilesListed`), used to build `items`.
 pub files: Vec<PathBuf>,
 /// Whether the async workspace file listing has been delivered.
 pub files_loaded: bool,
 /// Candidate entries (workspace files plus commands), freshly filtered to
 /// the current query. This is what is rendered and traversed by ↑/↓/Enter.
 pub items: Vec<QuickbarItem>,
 /// Index of the highlighted row within `items`.
 pub selected: usize,
}

impl QuickbarState {
 pub fn new() -> Self {
 QuickbarState {
 input: TextInputState::default(),
 files: Vec::new(),
 files_loaded: false,
 items: Vec::new(),
 selected: 0,
 }
 }
}

pub struct Model {
    pub root: PathBuf,
    pub tabs: Vec<Tab>,
    pub active_tab: Option<usize>,
    pub sidebar: Sidebar,
    pub terminal: TerminalState,
    pub layout: LayoutState,
    pub focus: Focus,
    pub should_quit: bool,
    /// User-editable keyboard shortcuts (loaded from `keybindings.toml`).
    pub keybindings: crate::services::keybindings::Keybindings,
    pub internal_clipboard: String,
    pub theme: Theme,
    /// Last known terminal size — for mouse hit-testing and layout.
    pub term_size: (u16, u16),
    pub drag: Option<DragTarget>,
    /// Use ASCII instead of Nerd Font icons (for compatibility).
    pub ascii_icons: bool,
    /// Colored lines for the active buffer, produced off-thread by the highlight
    /// worker (see `app::hlworker`). Indexed from `display_base`; rows outside the
    /// range render as plain text until the worker fills them in.
    display_hl: Vec<HlLine>,
    /// Buffer line index of `display_hl[0]` (the first visible line last shipped).
    display_base: usize,
    /// The (tab, version) `display_hl` was produced for. Colors may lag the current
    /// version by a frame or two while typing; that is the point — text is never
    /// held back waiting for color.
    display_key: Option<(usize, u64)>,
    /// Channel to the highlight worker; `None` until wired up in `run`.
    hl_tx: Option<std::sync::mpsc::Sender<crate::app::hlworker::HlJob>>,
    /// The (tab, version, scroll_y, needed) of the last job sent, to avoid
    /// resubmitting an identical request every frame while still catching a scroll
    /// that reveals lines above or below the shipped slice.
    hl_sent: Option<(usize, u64, usize, usize)>,
    /// Set when the worker must drop its cache before the next job (content replaced
    /// by reload / format, or the theme changed).
    hl_reset: bool,
    /// Line to jump to after the file is loaded (opening from a search result).
    pub pending_goto: Option<(PathBuf, usize)>,
    /// A file whose next load should become a diff-mode tab (opened from the Git panel).
    pub pending_diff: Option<PathBuf>,
    /// A just-opened diff tab that should scroll to its first change once HEAD loads.
    pub pending_diff_scroll: Option<PathBuf>,
    /// The open modal dialog (captures all input when present).
    pub dialog: Option<Dialog>,
    /// The open file-tree context menu (captures all input when present).
    pub context_menu: Option<ContextMenu>,
 /// The open quickbar (command palette) overlay, if any. It captures all input
 /// while present, like `Dialog`/`ContextMenu`.
 pub quickbar: Option<QuickbarState>,
    /// Change-gutter markers for the active buffer, keyed by line index.
    pub active_git_marks: std::collections::HashMap<usize, GutterKind>,
    /// Which tab the current git-diff markers were computed for. The diff is
    /// recomputed only on a tab switch or when `git_marks_dirty` is set (save,
    /// reload, disk change, HEAD load) — never on a plain edit, so the gutter does
    /// not churn a whole-file diff on every keystroke while typing.
    active_git_marks_tab: Option<usize>,
    /// Set when the git diff needs recomputing for a non-edit reason (file saved,
    /// reloaded, changed on disk, or its HEAD text (re)loaded). Consumed by the
    /// next `refresh_git_marks`.
    git_marks_dirty: bool,
    /// Removed line blocks for the active diff tab's inline view: `(anchor, lines)`
    /// renders `lines` right after buffer line `anchor` (`None` = before line 0).
    /// Empty unless the active tab is a diff tab. Recomputed with `active_git_marks`.
    pub active_deleted: Vec<(Option<usize>, Vec<String>)>,
    /// Cached visual rows for the active tab (see `diff_rows`). Rebuilt only when
    /// the buffer version changes — the render path borrows it instead of
    /// re-materializing a whole-file `Vec` two or three times per frame.
    active_display: Vec<DiffRow>,
    /// The (tab index, buffer version) `active_display` was built for.
    active_display_key: Option<(usize, u64)>,
    /// Deadline to fire a debounced autocomplete request, or `None`. Set to
    /// ~400ms ahead on each identifier keystroke and checked every main-loop
    /// iteration, so a burst of typing spawns no timer tasks and only asks the
    /// server once the user pauses.
    autocomplete_at: Option<std::time::Instant>,
    /// Deadline to flush a debounced LSP `didChange`, or `None`. Set ~1s ahead on
    /// each edit; checked every main-loop iteration like `autocomplete_at`.
    didchange_at: Option<std::time::Instant>,
    /// In-editor find / replace widget state.
    pub find: FindState,
    /// Last left-click (time, column, row) for editor double-click detection.
    pub last_click: Option<(std::time::Instant, u16, u16)>,
    /// Installed language extensions (LSP / formatter / linter manifests).
    pub extensions: crate::services::extensions::ExtensionRegistry,
    /// Running language servers.
    pub lsp: LspState,
    /// Diagnostics per file (buffer char coordinates).
    pub diagnostics: std::collections::HashMap<PathBuf, Vec<Diagnostic>>,
    /// Whether each tool binary (lsp / formatter / linter command) is installed
    /// on PATH, keyed by command name. Filled by `Cmd::CheckTools`; a missing
    /// key means "not probed yet".
    pub tool_available: std::collections::HashMap<String, bool>,
    /// The open completion popup, if any.
    pub completion: Option<CompletionState>,
    /// An in-flight format request awaiting edits.
    pub pending_format: Option<PendingFormat>,
    /// A transient toast notification shown bottom-center, or `None`.
    pub toast: Option<Toast>,
}

/// How long a toast stays on screen.
pub const TOAST_DURATION: std::time::Duration = std::time::Duration::from_millis(2500);

/// A transient bottom-center notification (e.g. "Copied to clipboard").
pub struct Toast {
    pub message: String,
    /// When the toast was raised; it is shown while `elapsed < TOAST_DURATION`.
    pub shown_at: std::time::Instant,
}

impl Toast {
    pub fn is_expired(&self) -> bool {
        self.shown_at.elapsed() >= TOAST_DURATION
    }
}

/// One visual row of the editor. In a diff tab, removed lines are woven in as
/// `Deleted` rows between the real buffer lines; every other tab is all `Real`.
#[derive(Clone)]
pub enum DiffRow {
    /// A real buffer line (0-based index).
    Real(usize),
    /// A removed line's text (shown red, not part of the buffer).
    Deleted(String),
}

impl Model {
    pub fn new(root: PathBuf) -> Self {
        let config = crate::services::config::load();
        let mut model = Model::with_defaults(root);
        model.apply_config(&config);
        model
    }

    fn with_defaults(root: PathBuf) -> Self {
        Model {
            keybindings: crate::services::keybindings::load(),
            sidebar: Sidebar {
                active: Panel::Files,
                files: FileTree::new(root.clone()),
                git: GitStatus::default(),
                search: SearchState::default(),
                themes: ThemesState::default(),
                settings: SettingsState::default(),
                settings_selected: 0,
            },
            tabs: Vec::new(),
            active_tab: None,
            terminal: TerminalState::new(),
            layout: LayoutState::default(),
            focus: Focus::Sidebar,
            should_quit: false,
            internal_clipboard: String::new(),
            // Keep the UI palette and the syntax theme consistent at startup.
            theme: highlight::theme_for(highlight::DEFAULT_THEME),
            term_size: (80, 24),
            drag: None,
            ascii_icons: std::env::var("CODER_ASCII").is_ok(),
            display_hl: Vec::new(),
            display_base: 0,
            display_key: None,
            hl_tx: None,
            hl_sent: None,
            hl_reset: false,
            pending_goto: None,
            pending_diff: None,
            pending_diff_scroll: None,
            dialog: None,
            context_menu: None,
            quickbar: None,
            active_git_marks: std::collections::HashMap::new(),
            active_git_marks_tab: None,
            git_marks_dirty: false,
            active_deleted: Vec::new(),
            active_display: Vec::new(),
            active_display_key: None,
            autocomplete_at: None,
            didchange_at: None,
            find: FindState::default(),
            last_click: None,
            extensions: crate::services::extensions::ExtensionRegistry::default(),
            lsp: LspState::default(),
            diagnostics: std::collections::HashMap::new(),
            tool_available: std::collections::HashMap::new(),
            completion: None,
            pending_format: None,
            toast: None,
            root,
        }
    }

    /// Raises a transient toast notification (bottom-center, auto-hides). Returns
    /// the command that schedules its disappearance.
    pub fn show_toast(&mut self, message: impl Into<String>) -> Cmd {
        self.toast = Some(Toast {
            message: message.into(),
            shown_at: std::time::Instant::now(),
        });
        Cmd::ScheduleToastExpiry
    }

    /// Surfaces a transient status message to the user as a toast. Fire-and-forget
    /// convenience for the many sync handlers that previously wrote the status bar;
    /// the toast's auto-hide is gated by `Toast::is_expired` at render time.
    pub fn notify(&mut self, message: impl Into<String>) {
        let _ = self.show_toast(message);
    }

    /// (errors, warnings) in the active buffer, from its stored diagnostics.
    pub fn active_diagnostic_counts(&self) -> (usize, usize) {
        use crate::services::lsp::Severity;
        self.active_buffer()
            .and_then(|b| b.path.as_ref())
            .and_then(|p| self.diagnostics.get(p))
            .map(|diags| {
                let e = diags.iter().filter(|d| d.severity == Severity::Error).count();
                let w = diags.iter().filter(|d| d.severity == Severity::Warning).count();
                (e, w)
            })
            .unwrap_or((0, 0))
    }

    /// The colored pieces for buffer line `row`, or `None` when the worker has not
    /// colored it yet (the renderer then draws it as plain text). Colors belong to
    /// the active tab and may trail the current version by a frame while typing.
    pub fn hl_line(&self, row: usize) -> Option<&HlLine> {
        let (tab, _) = self.display_key?;
        if Some(tab) != self.active_tab || row < self.display_base {
            return None;
        }
        self.display_hl.get(row - self.display_base)
    }

    /// Wires up the highlight worker channel (called once at startup).
    pub fn set_hl_worker(&mut self, tx: std::sync::mpsc::Sender<crate::app::hlworker::HlJob>) {
        self.hl_tx = Some(tx);
    }

    /// Stores a worker result, ignoring one older than what is already shown.
    pub fn set_display_hl(&mut self, tab: usize, version: u64, base: usize, lines: Vec<HlLine>) {
        if let Some((t, v)) = self.display_key
            && t == tab
            && v > version
        {
            return;
        }
        self.display_key = Some((tab, version));
        self.display_base = base;
        self.display_hl = lines;
    }

    /// Submits a highlight job for the active buffer's current viewport (called
    /// before render). Never runs syntect itself — the worker does, off-thread, so
    /// the render loop stays responsive no matter how slow the syntax is.
    pub fn refresh_highlight(&mut self) {
        if self.hl_tx.is_none() {
            return; // no worker wired up (e.g. in tests)
        }
        let Some(i) = self.active_tab else {
            return;
        };
        let ver = self.tabs[i].buffer.version;
        let sy = self.tabs[i].buffer.scroll_y;
        // Lines from the top down to the viewport bottom need color; overestimate
        // with the full terminal height so a partial editor pane is always covered.
        let needed = sy + self.term_size.1 as usize + 8;
        // Resubmit when the buffer changed, the tab changed, a reset was forced, or
        // the viewport scrolled to reveal lines above (`sy < s`) or below (`needed
        // > n`) the slice last shipped.
        let need_send = self.hl_reset
            || match self.hl_sent {
                Some((t, v, s, n)) => t != i || v != ver || sy < s || needed > n,
                None => true,
            };
        if !need_send {
            return;
        }
        let (dirty_from, wide) = self.tabs[i].buffer.take_dirty();
        let job = crate::app::hlworker::HlJob {
            tab: i,
            version: ver,
            path: self.tabs[i].buffer.path.clone(),
            theme_name: self.current_theme_name().to_string(),
            reset: self.hl_reset,
            text: self.tabs[i].buffer.full_text(),
            dirty_from,
            wide,
            scroll_y: sy,
            needed,
        };
        self.hl_sent = Some((i, ver, sy, needed));
        self.hl_reset = false;
        if let Some(tx) = &self.hl_tx {
            let _ = tx.send(job);
        }
    }

    /// Invalidates highlighting (content replaced externally, or theme changed):
    /// drops the shown colors so text falls back to plain until the worker — which
    /// is told to reset its cache — returns fresh ones.
    pub fn invalidate_highlight(&mut self) {
        self.hl_reset = true;
        self.hl_sent = None;
        self.display_key = None;
        self.active_display_key = None;
        // External content replacement (reload / format) changes the git diff too.
        self.git_marks_dirty = true;
    }

    /// Schedules a debounced autocomplete request ~400ms out, resetting the timer
    /// on every keystroke so the server is only asked once typing pauses.
    pub fn schedule_autocomplete(&mut self) {
        self.autocomplete_at =
            Some(std::time::Instant::now() + std::time::Duration::from_millis(400));
    }

    /// Schedules a debounced LSP `didChange` flush ~1s out (reset on every edit).
    pub fn schedule_didchange(&mut self) {
        self.didchange_at =
            Some(std::time::Instant::now() + std::time::Duration::from_secs(1));
    }

    /// Cancels any pending autocomplete deadline (e.g. the popup was dismissed).
    pub fn cancel_autocomplete(&mut self) {
        self.autocomplete_at = None;
    }

    /// Cancels any pending `didChange` deadline (its text was already flushed).
    pub fn cancel_didchange(&mut self) {
        self.didchange_at = None;
    }

    /// Returns `(autocomplete_due, didchange_due)` for deadlines that have elapsed
    /// by `now`, clearing each that fired. Called once per main-loop iteration.
    pub fn take_due_timers(&mut self, now: std::time::Instant) -> (bool, bool) {
        let ac = self.autocomplete_at.is_some_and(|t| t <= now);
        if ac {
            self.autocomplete_at = None;
        }
        let dc = self.didchange_at.is_some_and(|t| t <= now);
        if dc {
            self.didchange_at = None;
        }
        (ac, dc)
    }

    /// Forces the change-gutter diff to recompute on the next `refresh_git_marks`
    /// (file saved, reloaded, changed on disk, or its HEAD text (re)loaded).
    pub fn mark_git_dirty(&mut self) {
        self.git_marks_dirty = true;
    }

    /// Refreshes the change-gutter state before render. The visual row map
    /// (`active_display`) is rebuilt immediately on any version change — the
    /// renderer maps screen rows to buffer lines through it, so it must never lag.
    /// The git *diff* itself recomputes only on a tab switch or when
    /// `git_marks_dirty` is set (save / reload / disk change / HEAD load); a plain
    /// edit leaves the last-computed markers frozen, so typing never runs the
    /// whole-file diff.
    pub fn refresh_git_marks(&mut self) {
        let Some(i) = self.active_tab else {
            self.active_git_marks.clear();
            self.active_deleted.clear();
            self.active_display.clear();
            self.active_git_marks_tab = None;
            self.active_display_key = None;
            return;
        };
        let ver = self.tabs[i].buffer.version;
        // On a tab switch or an explicit trigger, rerun the whole-file diff; it
        // rebuilds the row map itself (woven deletions may have changed). Otherwise
        // a plain edit only needs the row map to track the new line count.
        if self.active_git_marks_tab != Some(i) || self.git_marks_dirty {
            self.recompute_git_marks();
        } else if self.active_display_key != Some((i, ver)) {
            self.rebuild_display(i);
            self.active_display_key = Some((i, ver));
        }
    }

    /// Runs the whole-file HEAD-vs-buffer diff for the active tab, refreshing the
    /// gutter markers and any woven deletion rows. Called from `refresh_git_marks`
    /// only on a tab switch or an explicit `git_marks_dirty` trigger.
    fn recompute_git_marks(&mut self) {
        let Some(i) = self.active_tab else {
            return;
        };
        let ver = self.tabs[i].buffer.version;
        self.active_git_marks.clear();
        self.active_deleted.clear();
        if let Some(head) = self.tabs[i].head_text.clone() {
            let new = self.tabs[i].buffer.full_text();
            for (ln, kind) in crate::services::git::gutter_marks(&head, &new) {
                self.active_git_marks.insert(ln, kind);
            }
            // Removed lines are only woven into the inline diff view.
            if self.tabs[i].diff_mode {
                self.active_deleted = crate::services::git::deleted_blocks(&head, &new);
            }
        }
        // Woven deletions may have changed -> refresh the visual row map with them.
        self.rebuild_display(i);
        self.active_display_key = Some((i, ver));
        self.active_git_marks_tab = Some(i);
        self.git_marks_dirty = false;
    }

    /// Rebuilds `active_display` for tab `i` from its line count and any woven
    /// deletions. Called only when the buffer version changes, so the render path
    /// can borrow the result instead of rebuilding it every frame.
    fn rebuild_display(&mut self, i: usize) {
        let n = self.tabs[i].buffer.line_count();
        self.active_display.clear();
        if !self.has_inline_deletions() {
            self.active_display.extend((0..n).map(DiffRow::Real));
            return;
        }
        self.active_display.reserve(n + self.active_deleted.len());
        // Removals anchored before the first line.
        for (anchor, lines) in &self.active_deleted {
            if anchor.is_none() {
                self.active_display
                    .extend(lines.iter().cloned().map(DiffRow::Deleted));
            }
        }
        for r in 0..n {
            self.active_display.push(DiffRow::Real(r));
            for (anchor, lines) in &self.active_deleted {
                if *anchor == Some(r) {
                    self.active_display
                        .extend(lines.iter().cloned().map(DiffRow::Deleted));
                }
            }
        }
    }

    /// Whether the active tab weaves removed lines into its view (diff tab with deletions).
    pub fn has_inline_deletions(&self) -> bool {
        self.active_is_diff() && !self.active_deleted.is_empty()
    }

    /// The visual rows for the active tab: `Real(0..n)` normally, or real lines
    /// interleaved with `Deleted` rows in a diff tab that has removals. Borrowed
    /// from a cache rebuilt only on edit (see `refresh_git_marks`), so the render
    /// path pays nothing to read it.
    pub fn diff_rows(&self) -> &[DiffRow] {
        &self.active_display
    }

    /// Display index of the first row to draw for a given buffer scroll offset.
    pub fn diff_start(&self, rows: &[DiffRow], scroll_y: usize) -> usize {
        if scroll_y == 0 {
            return 0;
        }
        // No woven deletions means the rows are the identity mapping, so line
        // `scroll_y` is at index `scroll_y` — skip scanning the whole prefix.
        if matches!(rows.get(scroll_y), Some(DiffRow::Real(l)) if *l == scroll_y) {
            return scroll_y;
        }
        rows.iter()
            .position(|r| matches!(r, DiffRow::Real(l) if *l == scroll_y))
            .unwrap_or(0)
    }

    /// Buffer line under a viewport row `offset` (0 = top visible row), mapping
    /// `Deleted` rows to the nearest following (then preceding) real line.
    pub fn screen_row_to_line(&self, offset: usize) -> usize {
        let Some(i) = self.active_tab else {
            return 0;
        };
        let buf = &self.tabs[i].buffer;
        let last = buf.line_count().saturating_sub(1);
        if !self.has_inline_deletions() {
            return (buf.scroll_y + offset).min(last);
        }
        let rows = self.diff_rows();
        let start = self.diff_start(rows, buf.scroll_y);
        let idx = (start + offset).min(rows.len().saturating_sub(1));
        for r in &rows[idx..] {
            if let DiffRow::Real(l) = r {
                return *l;
            }
        }
        for r in rows[..=idx].iter().rev() {
            if let DiffRow::Real(l) = r {
                return *l;
            }
        }
        last
    }

    /// Whether the editor should reserve a change-gutter column (active file is tracked).
    pub fn git_gutter(&self) -> bool {
        self.active_tab
            .map(|i| self.tabs[i].head_text.is_some())
            .unwrap_or(false)
    }

    /// Applies the theme at the given index: UI palette + syntax theme for all tabs.
    pub fn apply_theme(&mut self, idx: usize) {
        let Some(name) = self.sidebar.themes.names.get(idx).cloned() else {
            return;
        };
        self.sidebar.themes.selected = idx;
        self.theme = highlight::theme_for(&name);
        // The worker re-highlights with the new theme (carried in the next job);
        // invalidation drops the old colors and forces a fresh submission.
        self.invalidate_highlight();
    }

    /// Applies persisted preferences: selected theme + editor settings.
    pub fn apply_config(&mut self, config: &crate::services::config::Config) {
        if let Some(idx) = self
            .sidebar
            .themes
            .names
            .iter()
            .position(|n| n == &config.theme)
        {
            self.apply_theme(idx);
        }
        let s = &mut self.sidebar.settings;
        s.format_on_save = config.format_on_save;
        s.trim_trailing_whitespace = config.trim_trailing_whitespace;
        s.insert_final_newline = config.insert_final_newline;
        s.inline_diagnostics = config.inline_diagnostics;
        self.extensions =
            crate::services::extensions::ExtensionRegistry::from_config(&config.languages);
    }

    /// Snapshot of the current preferences, for persisting to disk.
    pub fn config_snapshot(&self) -> crate::services::config::Config {
        let s = &self.sidebar.settings;
        crate::services::config::Config {
            theme: self.current_theme_name().to_string(),
            format_on_save: s.format_on_save,
            trim_trailing_whitespace: s.trim_trailing_whitespace,
            insert_final_newline: s.insert_final_newline,
            inline_diagnostics: s.inline_diagnostics,
            languages: self.extensions.to_language_configs(),
        }
    }

    /// Name of the currently selected theme.
    pub fn current_theme_name(&self) -> &str {
        let t = &self.sidebar.themes;
        t.names
            .get(t.selected)
            .map(String::as_str)
            .unwrap_or(highlight::DEFAULT_THEME)
    }

    pub fn active_buffer(&self) -> Option<&Buffer> {
        self.active_tab.map(|i| &self.tabs[i].buffer)
    }

    /// The diagnostic under the active buffer's cursor, most severe first — used
    /// by the status bar to surface the message (a "hover" without LSP hover).
    pub fn diagnostic_at_cursor(&self) -> Option<&Diagnostic> {
        let buf = self.active_buffer()?;
        let path = buf.path.as_ref()?;
        let (line, col) = (buf.cursor.line, buf.cursor.col);
        self.diagnostics
            .get(path)?
            .iter()
            .filter(|d| d.line == line && col >= d.col_start && col <= d.col_end)
            .min_by_key(|d| match d.severity {
                crate::services::lsp::Severity::Error => 0u8,
                crate::services::lsp::Severity::Warning => 1,
                crate::services::lsp::Severity::Info => 2,
                crate::services::lsp::Severity::Hint => 3,
            })
    }

    pub fn active_buffer_mut(&mut self) -> Option<&mut Buffer> {
        let i = self.active_tab?;
        Some(&mut self.tabs[i].buffer)
    }

    /// Is there already an open *normal* (non-diff) tab for a given file?
    /// Diff tabs are excluded so a file and its diff live in separate tabs.
    pub fn tab_index_for(&self, path: &std::path::Path) -> Option<usize> {
        self.tabs
            .iter()
            .position(|t| !t.diff_mode && t.buffer.path.as_deref() == Some(path))
    }

    /// Indices of every open tab (normal or diff) for a given file.
    pub fn all_tabs_for(&self, path: &std::path::Path) -> Vec<usize> {
        self.tabs
            .iter()
            .enumerate()
            .filter(|(_, t)| t.buffer.path.as_deref() == Some(path))
            .map(|(i, _)| i)
            .collect()
    }

    /// Is there already an open diff-mode tab for a given file?
    pub fn diff_tab_index_for(&self, path: &std::path::Path) -> Option<usize> {
        self.tabs
            .iter()
            .position(|t| t.diff_mode && t.buffer.path.as_deref() == Some(path))
    }

    /// Whether the active tab is a diff-mode tab (changed lines get a colored background).
    pub fn active_is_diff(&self) -> bool {
        self.active_tab.map(|i| self.tabs[i].diff_mode).unwrap_or(false)
    }

    /// The open "<hash> diff" tab for a commit, if any.
    pub fn commit_diff_tab_index(&self, hash: &str) -> Option<usize> {
        let label = format!("{hash} diff");
        self.tabs.iter().position(|t| t.label.as_deref() == Some(label.as_str()))
    }

    /// What the active tab's buffer line `row` is, when it is a commit's diff
    /// view: the gutter and the row styling follow from it. `None` for a file.
    pub fn commit_row(&self, row: usize) -> Option<CommitRow> {
        let i = self.active_tab?;
        self.tabs[i].commit_rows.get(row).copied()
    }

    /// The largest number the gutter has to fit: the buffer's line count, or the
    /// highest file line number in a commit's diff view (which skips lines, so it
    /// can run past the number of rows shown).
    pub fn max_gutter_number(&self) -> usize {
        let Some(i) = self.active_tab else {
            return 1;
        };
        let tab = &self.tabs[i];
        if tab.commit_rows.is_empty() {
            return tab.buffer.line_count();
        }
        tab.commit_rows
            .iter()
            .filter_map(|r| match r {
                CommitRow::Line(n) => Some(*n),
                _ => None,
            })
            .max()
            .unwrap_or(1)
    }

    /// Whether the active tab holds generated content that must not be edited.
    pub fn active_read_only(&self) -> bool {
        self.active_tab.map(|i| self.tabs[i].read_only).unwrap_or(false)
    }

    /// The notice message of the active tab, if it is a read-only error tab.
    pub fn active_notice(&self) -> Option<&str> {
        let i = self.active_tab?;
        self.tabs[i].notice.as_deref()
    }

    pub fn panel_icon(&self, panel: Panel) -> &'static str {
        if self.ascii_icons {
            panel.ascii_icon()
        } else {
            panel.icon()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn git_zone_tab_order_wraps_both_ways() {
        // Tab walks top to bottom and wraps back to the commit box.
        assert_eq!(GitZone::Message.step(1), GitZone::Fetch);
        assert_eq!(GitZone::Push.step(1), GitZone::Uncommit);
        assert_eq!(GitZone::Commit.step(1), GitZone::Files);
        assert_eq!(GitZone::Files.step(1), GitZone::Message);
        // Shift+Tab is the exact inverse.
        assert_eq!(GitZone::Message.step(-1), GitZone::Files);
        assert_eq!(GitZone::Fetch.step(-1), GitZone::Message);
        for z in GitZone::ORDER {
            assert_eq!(z.step(1).step(-1), z);
        }
    }

    fn entry(rel: &str) -> GitEntry {
        GitEntry {
            path: PathBuf::from(rel),
            rel: rel.to_string(),
            state: crate::services::git::GitState::Modified,
        }
    }

    #[test]
    fn selection_runs_changes_then_history() {
        let mut g = GitStatus {
            staged: vec![entry("a.rs")],
            unstaged: vec![entry("b.rs"), entry("c.rs")],
            ..GitStatus::default()
        };
        g.history = vec![
            GitCommit {
                hash: "aaaaaaa".to_string(),
                summary: "first".to_string(),
            },
            GitCommit {
                hash: "bbbbbbb".to_string(),
                summary: "second".to_string(),
            },
        ];
        assert_eq!(g.changes_len(), 3);
        assert_eq!(g.nav_len(), 5);
        // The change rows come first: they resolve as entries, not commits.
        assert_eq!(g.entry_at(0).map(|(e, staged)| (e.rel.as_str(), staged)), Some(("a.rs", true)));
        assert_eq!(g.entry_at(2).map(|(e, staged)| (e.rel.as_str(), staged)), Some(("c.rs", false)));
        assert!(g.commit_at(2).is_none());
        // The history follows, in order.
        assert_eq!(g.commit_at(3).map(|c| c.hash.as_str()), Some("aaaaaaa"));
        assert_eq!(g.commit_at(4).map(|c| c.hash.as_str()), Some("bbbbbbb"));
        assert!(g.entry_at(4).is_none());
        assert!(g.commit_at(5).is_none());
    }

    #[test]
    fn commit_diff_tab_is_a_read_only_diff_tab() {
        let diff = crate::services::git::CommitDiff {
            old: "a\nb\n".to_string(),
            new: "a\nB\n".to_string(),
            rows: vec![CommitRow::Line(1), CommitRow::Line(2)],
            syntax_ext: Some("rs".to_string()),
        };
        let tab = Tab::commit_diff("2ea14b1", &diff);
        assert_eq!(tab.title(), "2ea14b1 diff");
        assert!(tab.read_only);
        assert!(tab.diff_mode); // green/red backgrounds, like an uncommitted change
        assert_eq!(tab.head_text.as_deref(), Some("a\nb\n"));
        // The synthetic path only picks the syntax the code is colored with.
        assert_eq!(tab.buffer.path.as_deref(), Some(std::path::Path::new("2ea14b1.rs")));
    }
}
