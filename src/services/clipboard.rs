//! System clipboard access.
//!
//! arboard owns the X11/Wayland selection only while its `Clipboard` lives, and
//! prints a warning to stderr ("Clipboard was dropped very quickly after
//! writing…") when an instance is dropped right after a write — which corrupts
//! the TUI. So we keep a single long-lived instance for the whole process
//! instead of creating one per copy.

use std::sync::{Mutex, OnceLock};

use arboard::Clipboard;

fn instance() -> Option<&'static Mutex<Clipboard>> {
    static CLIPBOARD: OnceLock<Option<Mutex<Clipboard>>> = OnceLock::new();
    CLIPBOARD
        .get_or_init(|| Clipboard::new().ok().map(Mutex::new))
        .as_ref()
}

/// Writes text to the system clipboard. Silent no-op if unavailable.
pub fn set_text(text: String) {
    if let Some(cb) = instance()
        && let Ok(mut cb) = cb.lock()
    {
        let _ = cb.set_text(text);
    }
}

/// Reads text from the system clipboard, or an empty string if unavailable.
pub fn get_text() -> String {
    if let Some(cb) = instance()
        && let Ok(mut cb) = cb.lock()
        && let Ok(text) = cb.get_text()
    {
        return text;
    }
    String::new()
}
