//! User-editable keyboard shortcuts.
//!
//! The discrete *command* shortcuts (quit, save, copy, new file, …) are declared
//! in a hand-editable TOML file and loaded into a lookup that `keymap::resolve`
//! consults before its hardcoded fallback. Text typing and cursor motion are
//! **not** remappable — they stay in code — so this file only ever holds the
//! named commands a user might realistically want to rebind.
//!
//! ```toml
//! [global]
//! quit = "ctrl+q"
//! save = "ctrl+s"
//!
//! [editor]
//! copy = "ctrl+c"
//!
//! [sidebar]
//! new_file = "ctrl+n"
//! ```

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use std::path::PathBuf;

use crate::app::model::{Focus, Panel};
use crate::core::keymap::Action;

/// A remappable command. Each maps to exactly one `Action` and lives in one
/// scope (which focuses it applies to).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bindable {
    // Global (any focus)
    Quit,
    ToggleSidebar,
    ToggleTerminal,
    CloseTab,
    Save,
    NextTab,
    PrevTab,
    Find,
    Replace,
    Shortcuts,
 Quickbar,
    Explorer,
    Search,
    Git,
    Extensions,
    Themes,
    Settings,
    Rename,
    // Editor focus
    Copy,
    Cut,
    Paste,
    Undo,
    Redo,
    SelectAll,
    Completion,
    Format,
    MoveLineUp,
    MoveLineDown,
    // Sidebar focus
    NewFile,
    NewFolder,
    DeleteEntry,
}

/// Which focus modes a binding fires in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// Fires in every focus.
    Global,
    /// Fires only while the editor is focused.
    Editor,
    /// Fires only while the sidebar is focused.
    Sidebar,
}

impl Scope {
    fn matches(self, focus: Focus) -> bool {
        match self {
            Scope::Global => true,
            Scope::Editor => focus == Focus::Editor,
            Scope::Sidebar => focus == Focus::Sidebar,
        }
    }

    /// The `[section]` name this scope is written under in the TOML file.
    fn section(self) -> &'static str {
        match self {
            Scope::Global => "global",
            Scope::Editor => "editor",
            Scope::Sidebar => "sidebar",
        }
    }
}

impl Bindable {
    /// Every command, in file order (also the TOML write order).
    const ALL: [Bindable; 31] = [
        Bindable::Quit,
        Bindable::ToggleSidebar,
        Bindable::ToggleTerminal,
        Bindable::CloseTab,
        Bindable::Save,
        Bindable::NextTab,
        Bindable::PrevTab,
        Bindable::Find,
        Bindable::Replace,
        Bindable::Shortcuts,
 Bindable::Quickbar,
        Bindable::Explorer,
        Bindable::Search,
        Bindable::Git,
        Bindable::Extensions,
        Bindable::Themes,
        Bindable::Settings,
        Bindable::Rename,
        Bindable::Copy,
        Bindable::Cut,
        Bindable::Paste,
        Bindable::Undo,
        Bindable::Redo,
        Bindable::SelectAll,
        Bindable::Completion,
        Bindable::Format,
        Bindable::MoveLineUp,
        Bindable::MoveLineDown,
        Bindable::NewFile,
        Bindable::NewFolder,
        Bindable::DeleteEntry,
    ];

    /// Stable TOML key name.
    fn name(self) -> &'static str {
        match self {
            Bindable::Quit => "quit",
            Bindable::ToggleSidebar => "toggle_sidebar",
            Bindable::ToggleTerminal => "toggle_terminal",
            Bindable::CloseTab => "close_tab",
            Bindable::Save => "save",
            Bindable::NextTab => "next_tab",
            Bindable::PrevTab => "prev_tab",
            Bindable::Find => "find",
            Bindable::Replace => "replace",
            Bindable::Shortcuts => "shortcuts",
 Bindable::Quickbar => "quickbar",
            Bindable::Explorer => "explorer",
            Bindable::Search => "search",
            Bindable::Git => "git",
            Bindable::Extensions => "extensions",
            Bindable::Themes => "themes",
            Bindable::Settings => "settings",
            Bindable::Rename => "rename",
            Bindable::Copy => "copy",
            Bindable::Cut => "cut",
            Bindable::Paste => "paste",
            Bindable::Undo => "undo",
            Bindable::Redo => "redo",
            Bindable::SelectAll => "select_all",
            Bindable::Completion => "completion",
            Bindable::Format => "format",
            Bindable::MoveLineUp => "move_line_up",
            Bindable::MoveLineDown => "move_line_down",
            Bindable::NewFile => "new_file",
            Bindable::NewFolder => "new_folder",
            Bindable::DeleteEntry => "delete_entry",
        }
    }

    fn scope(self) -> Scope {
        match self {
            Bindable::Copy
            | Bindable::Cut
            | Bindable::Paste
            | Bindable::Undo
            | Bindable::Redo
            | Bindable::SelectAll
            | Bindable::Completion
            | Bindable::Format
            | Bindable::MoveLineUp
            | Bindable::MoveLineDown => Scope::Editor,
            Bindable::NewFile | Bindable::NewFolder | Bindable::DeleteEntry => Scope::Sidebar,
            _ => Scope::Global,
        }
    }

    /// The out-of-the-box chord (also what seeds a fresh file).
    fn default_chord(self) -> &'static str {
        match self {
            Bindable::Quit => "ctrl+q",
            Bindable::ToggleSidebar => "ctrl+b",
            Bindable::ToggleTerminal => "ctrl+j",
            Bindable::CloseTab => "ctrl+w",
            Bindable::Save => "ctrl+s",
            Bindable::NextTab => "ctrl+tab",
            // Shift+Tab (BackTab) works in legacy terminals; ctrl+shift+tab does not.
            Bindable::PrevTab => "shift+tab",
            Bindable::Find => "ctrl+f",
            Bindable::Replace => "ctrl+h",
            Bindable::Shortcuts => "alt+7",
 Bindable::Quickbar => "ctrl+p",
            // Panels use Alt+digit: legacy terminals (GNOME Terminal / VTE) can't
            // send Ctrl+Shift+<letter> distinctly — the Shift bit collapses so
            // e.g. Ctrl+Shift+S is byte-identical to Ctrl+S (Save). Alt+digit
            // sends a distinct ESC-prefixed sequence that every terminal reports.
            Bindable::Explorer => "alt+1",
            Bindable::Search => "alt+2",
            Bindable::Git => "alt+3",
            Bindable::Extensions => "alt+4",
            Bindable::Themes => "alt+5",
            Bindable::Settings => "alt+6",
            Bindable::Rename => "f2",
            Bindable::Copy => "ctrl+c",
            Bindable::Cut => "ctrl+x",
            Bindable::Paste => "ctrl+v",
            Bindable::Undo => "ctrl+z",
            Bindable::Redo => "ctrl+y",
            Bindable::SelectAll => "ctrl+a",
            Bindable::Completion => "ctrl+space",
            Bindable::Format => "ctrl+alt+f",
            Bindable::MoveLineUp => "alt+up",
            Bindable::MoveLineDown => "alt+down",
            Bindable::NewFile => "ctrl+n",
            Bindable::NewFolder => "alt+shift+n",
            Bindable::DeleteEntry => "delete",
        }
    }

    /// The concrete `Action` this command triggers.
    fn to_action(self) -> Action {
        match self {
            Bindable::Quit => Action::Quit,
            Bindable::ToggleSidebar => Action::ToggleSidebar,
            Bindable::ToggleTerminal => Action::ToggleTerminal,
            Bindable::CloseTab => Action::CloseTab,
            Bindable::Save => Action::Save,
            Bindable::NextTab => Action::NextTab,
            Bindable::PrevTab => Action::PrevTab,
            Bindable::Find => Action::OpenFind,
            Bindable::Replace => Action::OpenFindReplace,
            Bindable::Shortcuts => Action::ShowShortcuts,
 Bindable::Quickbar => Action::OpenQuickbar,
            Bindable::Explorer => Action::SelectPanel(Panel::Files),
            Bindable::Search => Action::SelectPanel(Panel::Search),
            Bindable::Git => Action::SelectPanel(Panel::Git),
            Bindable::Extensions => Action::SelectPanel(Panel::Extensions),
            Bindable::Themes => Action::SelectPanel(Panel::Themes),
            Bindable::Settings => Action::SelectPanel(Panel::Settings),
            Bindable::Rename => Action::RenameEntry,
            Bindable::Copy => Action::Copy,
            Bindable::Cut => Action::Cut,
            Bindable::Paste => Action::Paste,
            Bindable::Undo => Action::Undo,
            Bindable::Redo => Action::Redo,
            Bindable::SelectAll => Action::SelectAll,
            Bindable::Completion => Action::TriggerCompletion,
            Bindable::Format => Action::Format,
            Bindable::MoveLineUp => Action::MoveLineUp,
            Bindable::MoveLineDown => Action::MoveLineDown,
            Bindable::NewFile => Action::NewFile,
            Bindable::NewFolder => Action::NewFolder,
            Bindable::DeleteEntry => Action::DeleteEntry,
        }
    }

    fn by_name(section: &str, name: &str) -> Option<Bindable> {
        Bindable::ALL
            .into_iter()
            .find(|b| b.scope().section() == section && b.name() == name)
    }
}

/// A key press as a comparable chord (letters normalized to lowercase).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Chord {
    code: KeyCode,
    ctrl: bool,
    shift: bool,
    alt: bool,
}

impl Chord {
    fn from_event(key: KeyEvent) -> Chord {
        // Legacy terminals encode Shift in the character itself (an uppercase
        // letter, or BackTab for Shift+Tab) rather than a modifier bit. Fold that
        // back into `shift` so chords like "shift+tab" / "alt+shift+n" still match.
        let mut shift = key.modifiers.contains(KeyModifiers::SHIFT);
        let code = match key.code {
            KeyCode::BackTab => {
                shift = true;
                KeyCode::Tab
            }
            KeyCode::Char(c) => {
                if c.is_ascii_uppercase() {
                    shift = true;
                }
                KeyCode::Char(c.to_ascii_lowercase())
            }
            other => other,
        };
        Chord {
            code,
            ctrl: key.modifiers.contains(KeyModifiers::CONTROL),
            shift,
            alt: key.modifiers.contains(KeyModifiers::ALT),
        }
    }

    /// Parses `"ctrl+shift+e"`, `"alt+up"`, `"f2"`, `"delete"`, … (case-insensitive).
    fn parse(s: &str) -> Option<Chord> {
        let (mut ctrl, mut shift, mut alt) = (false, false, false);
        let mut code = None;
        for part in s.split('+') {
            match part.trim().to_ascii_lowercase().as_str() {
                "ctrl" => ctrl = true,
                "shift" => shift = true,
                "alt" => alt = true,
                "" => {}
                key => code = Some(parse_key(key)?),
            }
        }
        Some(Chord { code: code?, ctrl, shift, alt })
    }

    fn to_chord_string(self) -> String {
        let mut parts: Vec<String> = Vec::new();
        if self.ctrl {
            parts.push("ctrl".into());
        }
        if self.shift {
            parts.push("shift".into());
        }
        if self.alt {
            parts.push("alt".into());
        }
        parts.push(key_string(self.code));
        parts.join("+")
    }
}

fn parse_key(s: &str) -> Option<KeyCode> {
    Some(match s {
        "space" => KeyCode::Char(' '),
        "tab" => KeyCode::Tab,
        "enter" | "return" => KeyCode::Enter,
        "esc" | "escape" => KeyCode::Esc,
        "delete" | "del" => KeyCode::Delete,
        "backspace" => KeyCode::Backspace,
        "up" => KeyCode::Up,
        "down" => KeyCode::Down,
        "left" => KeyCode::Left,
        "right" => KeyCode::Right,
        "home" => KeyCode::Home,
        "end" => KeyCode::End,
        "pageup" => KeyCode::PageUp,
        "pagedown" => KeyCode::PageDown,
        other => {
            if let Some(n) = other
                .strip_prefix('f')
                .filter(|d| !d.is_empty())
                .and_then(|d| d.parse::<u8>().ok())
            {
                if (1..=12).contains(&n) {
                    KeyCode::F(n)
                } else {
                    return None;
                }
            } else if other.chars().count() == 1 {
                KeyCode::Char(other.chars().next().unwrap().to_ascii_lowercase())
            } else {
                return None;
            }
        }
    })
}

fn key_string(code: KeyCode) -> String {
    match code {
        KeyCode::Char(' ') => "space".into(),
        KeyCode::Char(c) => c.to_string(),
        KeyCode::Tab => "tab".into(),
        KeyCode::Enter => "enter".into(),
        KeyCode::Esc => "esc".into(),
        KeyCode::Delete => "delete".into(),
        KeyCode::Backspace => "backspace".into(),
        KeyCode::Up => "up".into(),
        KeyCode::Down => "down".into(),
        KeyCode::Left => "left".into(),
        KeyCode::Right => "right".into(),
        KeyCode::Home => "home".into(),
        KeyCode::End => "end".into(),
        KeyCode::PageUp => "pageup".into(),
        KeyCode::PageDown => "pagedown".into(),
        KeyCode::F(n) => format!("f{n}"),
        other => format!("{other:?}").to_lowercase(),
    }
}

/// The loaded shortcuts: an ordered `(command, chord)` table.
pub struct Keybindings {
    binds: Vec<(Bindable, Chord)>,
}

impl Default for Keybindings {
    fn default() -> Self {
        let binds = Bindable::ALL
            .into_iter()
            // Defaults are known-valid, so `parse` never fails here.
            .filter_map(|b| Chord::parse(b.default_chord()).map(|c| (b, c)))
            .collect();
        Keybindings { binds }
    }
}

impl Keybindings {
    /// Resolves a key press to its bound `Action` for the given focus, or `None`
    /// (letting the hardcoded typing/motion fallback take over).
    pub fn resolve(&self, key: KeyEvent, focus: Focus) -> Option<Action> {
        let chord = Chord::from_event(key);
        self.binds
            .iter()
            .find(|(b, c)| *c == chord && b.scope().matches(focus))
            .map(|(b, _)| b.to_action())
    }

    fn set(&mut self, b: Bindable, chord: Chord) {
        if let Some(entry) = self.binds.iter_mut().find(|(x, _)| *x == b) {
            entry.1 = chord;
        }
    }
}

/// Path of the keybindings file: alongside `config.toml` (`keybindings.toml`).
pub fn keybindings_path() -> Option<PathBuf> {
    Some(crate::services::config::config_path()?.with_file_name("keybindings.toml"))
}

/// Parses the file, starting from the defaults and overriding any command whose
/// chord is declared. Unknown keys / unparseable chords are ignored, so a
/// partial or slightly malformed file still yields a working keymap.
pub fn parse(text: &str) -> Keybindings {
    let mut kb = Keybindings::default();
    let Ok(table) = text.parse::<toml::Table>() else {
        return kb;
    };
    for (section, value) in &table {
        let Some(entries) = value.as_table() else {
            continue;
        };
        for (name, chord_val) in entries {
            let Some(chord_str) = chord_val.as_str() else {
                continue;
            };
            if let Some(b) = Bindable::by_name(section, name)
                && let Some(chord) = Chord::parse(chord_str)
            {
                kb.set(b, chord);
            }
        }
    }
    kb
}

/// Serializes the shortcuts to a commented, `[section]`-grouped TOML document.
pub fn to_toml(kb: &Keybindings) -> String {
    let chord_of = |b: Bindable| {
        kb.binds
            .iter()
            .find(|(x, _)| *x == b)
            .map(|(_, c)| c.to_chord_string())
            .unwrap_or_default()
    };
    let mut out = String::new();
    out.push_str("# Coder keyboard shortcuts. Edit a value and save (Ctrl+S) to apply live.\n");
    out.push_str("# Chord format: \"ctrl+shift+e\", \"alt+up\", \"f2\", \"delete\", \"ctrl+space\".\n");
    out.push_str("# Modifiers: ctrl, shift, alt. Keys: a-z, 0-9, f1-f12, up/down/left/right,\n");
    out.push_str("# home/end, pageup/pagedown, tab, enter, esc, space, delete.\n");
    out.push_str("# Only these commands are remappable; typing and cursor motion are fixed.\n");
    for scope in [Scope::Global, Scope::Editor, Scope::Sidebar] {
        out.push_str(&format!("\n[{}]\n", scope.section()));
        for b in Bindable::ALL.into_iter().filter(|b| b.scope() == scope) {
            out.push_str(&format!("{} = \"{}\"\n", b.name(), chord_of(b)));
        }
    }
    out
}

/// Writes the shortcuts to disk (creating the parent directory). Errors ignored.
pub fn save(kb: &Keybindings) {
    let Some(path) = keybindings_path() else {
        return;
    };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(&path, to_toml(kb));
}

/// Loads the shortcuts, seeding the file with the defaults when it is missing so
/// there is always something to open and edit.
pub fn load() -> Keybindings {
    let Some(path) = keybindings_path() else {
        return Keybindings::default();
    };
    match std::fs::read_to_string(&path) {
        Ok(text) => parse(&text),
        Err(_) => {
            let kb = Keybindings::default();
            save(&kb);
            kb
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, mods)
    }

    #[test]
    fn defaults_resolve_global_and_scoped() {
        let kb = Keybindings::default();
        // Ctrl+Q quits from any focus.
        assert!(matches!(
            kb.resolve(ev(KeyCode::Char('q'), KeyModifiers::CONTROL), Focus::Editor),
            Some(Action::Quit)
        ));
        // Ctrl+C copies only in the editor.
        assert!(matches!(
            kb.resolve(ev(KeyCode::Char('c'), KeyModifiers::CONTROL), Focus::Editor),
            Some(Action::Copy)
        ));
        assert!(kb
            .resolve(ev(KeyCode::Char('c'), KeyModifiers::CONTROL), Focus::Sidebar)
            .is_none());
    }

    #[test]
    fn alt7_opens_shortcuts() {
        let kb = Keybindings::default();
        assert!(matches!(
            kb.resolve(ev(KeyCode::Char('7'), KeyModifiers::ALT), Focus::Editor),
            Some(Action::ShowShortcuts)
        ));
    }

    #[test]
    fn plain_typing_is_not_a_command() {
        let kb = Keybindings::default();
        assert!(kb
            .resolve(ev(KeyCode::Char('a'), KeyModifiers::NONE), Focus::Editor)
            .is_none());
    }

    #[test]
    fn overrides_apply_and_round_trip() {
        let kb = parse("[global]\nquit = \"ctrl+shift+q\"\n");
        assert!(kb
            .resolve(ev(KeyCode::Char('q'), KeyModifiers::CONTROL), Focus::Editor)
            .is_none());
        assert!(matches!(
            kb.resolve(
                ev(KeyCode::Char('q'), KeyModifiers::CONTROL | KeyModifiers::SHIFT),
                Focus::Editor
            ),
            Some(Action::Quit)
        ));
        // A full round-trip preserves the override.
        let restored = parse(&to_toml(&kb));
        assert!(matches!(
            restored.resolve(
                ev(KeyCode::Char('q'), KeyModifiers::CONTROL | KeyModifiers::SHIFT),
                Focus::Editor
            ),
            Some(Action::Quit)
        ));
    }
}
