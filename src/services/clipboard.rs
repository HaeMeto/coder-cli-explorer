//! System clipboard access.
//!
//! Two independent channels are used so copies work both on a local desktop and
//! across SSH / remote / tmux-like sessions (no X11/Wayland display). The first
//! is **OSC-52**, a terminal escape sequence (`ESC ] 52 ; c ; <base64> ESC \`)
//! that asks the *local* terminal emulator to write the payload to its own
//! clipboard — this is what makes copy reach a Windows/macOS host through an
//! SSH remote or a multiplexer: the bytes travel over the PTY stream, so no
//! remote display server is needed. OSC-52 must be written on the main thread
//! (the same one that renders the TUI) to avoid racing the TUI's own stdout
//! writes. The second is **arboard**, the native X11/Wayland clipboard of the
//! machine coder runs on, which is the fallback for a local desktop.
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

/// Writes text to the native (X11/Wayland) system clipboard. Silent no-op if
/// unavailable. Prefer this only on a local desktop; on a remote/SSH session
/// there is no display, so call [`emit_osc52`] as well.
pub fn set_system(text: &str) {
 if let Some(cb) = instance()
 && let Ok(mut cb) = cb.lock()
 {
 let _ = cb.set_text(text.to_owned());
 }
}

// A soft practical ceiling for OSC-52 payloads. The sequence base64-encodes the
// text, so a huge selection would blow past most terminal emulators' OSC buffer
// (and some cap/drop oversized responses). Beyond this we still try the native
// clipboard; on a remote with no display it just won't reach the host, but an
// oversized copy is rare and better dropped than corrupting the terminal.
const OSC52_MAX_BYTES: usize = 1 << 20; // 1 MiB

/// Emits an OSC-52 clipboard sequence on stdout so the *local* terminal
/// emulator writes `text` to its own clipboard (bypassing any remote SSH host
/// that has no display). Written atomically with a single flush; call only from
/// the main thread so it never races the TUI's own stdout writes.
pub fn emit_osc52(text: &str) {
 if text.is_empty() || text.len() > OSC52_MAX_BYTES {
 return;
 }
 use std::io::Write;
 // 4 output chars per 3 input bytes, rounded up, plus the wrapping fence.
 let mut buf = String::with_capacity(text.len().div_ceil(3) * 4 + 8);
 buf.push_str("\x1b]52;c;");
 buf.push_str(&base64(text.as_bytes()));
 buf.push_str("\x1b\\");
 let _ = std::io::stdout().flush().and_then(|_| {
 let mut out = std::io::stdout().lock();
 out.write_all(buf.as_bytes())?;
 out.flush()
 });
}

/// Reads text from the native system clipboard, or an empty string if
/// unavailable. In a remote/SSH session this is usually empty; arriving text is
/// pasted locally via the terminal's OSC-52 / bracketed-paste input path.
pub fn get_text() -> String {
 if let Some(cb) = instance()
 && let Ok(mut cb) = cb.lock()
 && let Ok(text) = cb.get_text()
 {
 return text;
 }
 String::new()
}

/// Standard RFC 4648 base64 with padding.
fn base64(data: &[u8]) -> String {
 const ALPHABET: &[u8; 64] =
 b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
 let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
 let (chunks, rem) = data.as_chunks::<3>();
 for c in chunks {
 let n = ((c[0] as u32) << 16) | ((c[1] as u32) << 8) | (c[2] as u32);
 out.push(ALPHABET[(n >> 18) as usize & 63] as char);
 out.push(ALPHABET[(n >> 12) as usize & 63] as char);
 out.push(ALPHABET[(n >> 6) as usize & 63] as char);
 out.push(ALPHABET[n as usize & 63] as char);
 }
 match rem.len() {
 1 => {
 let n = (rem[0] as u32) << 16;
 out.push(ALPHABET[(n >> 18) as usize & 63] as char);
 out.push(ALPHABET[(n >> 12) as usize & 63] as char);
 out.push('=');
 out.push('=');
 }
 2 => {
 let n = ((rem[0] as u32) << 16) | ((rem[1] as u32) << 8);
 out.push(ALPHABET[(n >> 18) as usize & 63] as char);
 out.push(ALPHABET[(n >> 12) as usize & 63] as char);
 out.push(ALPHABET[(n >> 6) as usize & 63] as char);
 out.push('=');
 }
 _ => {}
 }
 out
}

#[cfg(test)]
mod tests {
 use super::base64;

 #[test]
 fn base64_encodes_rfc4648_vectors() {
 assert_eq!(base64(b""), "");
 assert_eq!(base64(b"f"), "Zg==");
 assert_eq!(base64(b"fo"), "Zm8=");
 assert_eq!(base64(b"foo"), "Zm9v");
 assert_eq!(base64(b"foob"), "Zm9vYg==");
 assert_eq!(base64(b"fooba"), "Zm9vYmE=");
 assert_eq!(base64(b"foobar"), "Zm9vYmFy");
 }
}
