//! Workspace session persistence: open tabs, cursor/scroll, and unsaved
//! ("dirty") content, so relaunching coder in the same folder resumes where
//! you left off — including after a crash, and even for a file that was
//! deleted outside the editor since (it keeps showing as an unsaved buffer).
//!
//! Keyed by the workspace root directory's `(dev, inode)` (see `root_key`), not
//! its path string, so a `mv`/rename of the folder on the same filesystem does
//! not lose the session — `rename(2)` never changes the inode. It does *not*
//! survive a copy, or a move across filesystems (a new inode either way); that
//! is an accepted, documented limit rather than a bug.
//!
//! Stored globally at `~/.config/coder/sessions/<dev>-<ino>.toml` (or under
//! `$CODER_SESSION_DIR`), one file per workspace — never inside the project
//! folder itself.
//!
//! Dirty files above [`DIFF_THRESHOLD_BYTES`] are stored as a diff (line
//! hunks, via `git2::Patch` — the same machinery `services::git` already uses
//! for the change gutter) against their *current on-disk* content instead of
//! full text, to keep the session file small; restoring re-reads the file and
//! applies the hunks. Small dirty files, untitled buffers, and any file that
//! no longer has an on-disk baseline to diff against always keep their full
//! text — the buffer already holds it, so that fallback is free.
//!
//! Multi-instance (two `coder` processes on the same workspace): a whole-file
//! `generation` counter implements a coarse first-write-wins check (see
//! [`save`]) — an instance that observes a stale generation skips its write
//! and the caller surfaces a toast instead of silently clobbering the other
//! instance's checkpoint. There is no per-file granularity or automatic merge
//! yet; that is a deliberate MVP scope cut (see `AGENTS.md`).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Dirty files at or above this size are stored as a diff against their
/// on-disk baseline instead of full text.
pub const DIFF_THRESHOLD_BYTES: usize = 256 * 1024;

/// How long the editor waits, idle, after an edit before writing a checkpoint
/// (debounced the same way LSP's `didChange` is — see `Model::schedule_didchange`).
pub const CHECKPOINT_DEBOUNCE: std::time::Duration = std::time::Duration::from_secs(2);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSnapshot {
    /// Informational only, not the lookup key: lets a hand-inspection of the
    /// file identify which workspace it belongs to.
    pub root: String,
    /// Bumped on every successful write; see the module doc's multi-instance note.
    #[serde(default)]
    pub generation: u64,
    /// Which tab should end up focused on restore, identified by its key —
    /// the file path, or `"untitled:<id>"` — rather than a raw index, since
    /// generated tabs (a commit patch, a binary notice) are never persisted
    /// and would otherwise shift indices out from under a plain position.
    pub active: Option<String>,
    pub sidebar_panel: String,
    pub sidebar_width: u16,
    pub terminal_open: bool,
    /// Next number to hand out for "Untitled-N" — carried across restarts so
    /// numbering does not restart at 1 and collide with a still-open buffer.
    #[serde(default)]
    pub untitled_seq: u64,
    #[serde(default)]
    pub tabs: Vec<TabEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TabEntry {
    /// `"file"` or `"untitled"`.
    pub kind: String,
    /// The on-disk path, for `kind = "file"`.
    #[serde(default)]
    pub path: Option<String>,
    /// Stable id for `kind = "untitled"`, which has no path to key on.
    #[serde(default)]
    pub untitled_id: Option<String>,
    #[serde(default)]
    pub label: Option<String>,
    pub line: usize,
    pub col: usize,
    pub scroll_y: usize,
    pub scroll_x: usize,
    pub dirty: bool,
    /// Present only when `dirty`: the content that has not reached disk.
    #[serde(default)]
    pub content: Option<Content>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "mode")]
pub enum Content {
    /// The buffer's full text: a small dirty file, an untitled buffer, or a
    /// file with no on-disk baseline left to diff against (deleted outside
    /// the editor since it was opened).
    Full { text: String },
    /// Hunks to apply over the file's *current* on-disk content to reconstruct
    /// the dirty buffer (large dirty files; see [`DIFF_THRESHOLD_BYTES`]).
    Diff { hunks: Vec<Hunk> },
}

/// One diff hunk in old-file line coordinates: replace `old_lines` lines
/// starting at `old_start` (0-based) with `new_text`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hunk {
    pub old_start: usize,
    pub old_lines: usize,
    pub new_text: String,
}

/// Stable key for `root`, tolerant of a same-filesystem rename: the directory's
/// `(dev, ino)`, which `rename(2)` never changes. `None` if `root` can't be
/// `stat`-ed (already gone).
pub fn root_key(root: &Path) -> Option<String> {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::metadata(root).ok()?;
    Some(format!("{:x}-{:x}", meta.dev(), meta.ino()))
}

/// Base directory for session files: `$CODER_SESSION_DIR`, else alongside
/// `config.toml` (`~/.config/coder/sessions`).
fn session_dir() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("CODER_SESSION_DIR") {
        return Some(PathBuf::from(p));
    }
    Some(crate::services::config::config_path()?.parent()?.join("sessions"))
}

fn session_path(root: &Path) -> Option<PathBuf> {
    let key = root_key(root)?;
    Some(session_dir()?.join(format!("{key}.toml")))
}

/// Loads the session for `root`, if one exists and parses.
pub fn load(root: &Path) -> Option<SessionSnapshot> {
    let path = session_path(root)?;
    let text = std::fs::read_to_string(path).ok()?;
    toml::from_str(&text).ok()
}

/// The generation currently on disk for `root` (0 if there is none yet) — used
/// to seed the multi-instance check at startup.
pub fn current_generation(root: &Path) -> u64 {
    load(root).map(|s| s.generation).unwrap_or(0)
}

pub enum SaveOutcome {
    /// Written; carries the new generation.
    Saved(u64),
    /// Skipped: another instance wrote a newer generation than `seen` since
    /// this instance last checked. The caller should surface this and
    /// remember the returned generation as its new baseline.
    Conflict(u64),
    /// No session directory available (e.g. `$HOME` unset).
    NoPath,
}

/// Writes `snapshot` for `root`, first checking the on-disk generation against
/// `seen` (the generation this instance last observed) — a coarse first-
/// write-wins guard against two `coder` instances clobbering each other's
/// checkpoint (see the module doc). Not airtight — there is a small
/// check-then-write race across processes, undefended by a filesystem lock,
/// an accepted MVP trade-off — but the write itself is atomic (temp file +
/// rename), so a torn/partial session file is never observed.
pub fn save(root: &Path, snapshot: &mut SessionSnapshot, seen: u64) -> SaveOutcome {
    let Some(path) = session_path(root) else {
        return SaveOutcome::NoPath;
    };
    let disk_gen = std::fs::read_to_string(&path)
        .ok()
        .and_then(|t| toml::from_str::<SessionSnapshot>(&t).ok())
        .map(|s| s.generation)
        .unwrap_or(0);
    if disk_gen != seen {
        return SaveOutcome::Conflict(disk_gen);
    }
    snapshot.generation = disk_gen + 1;
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let Ok(text) = toml::to_string_pretty(snapshot) else {
        return SaveOutcome::NoPath;
    };
    let tmp = path.with_extension(format!("toml.tmp.{}", std::process::id()));
    if std::fs::write(&tmp, text)
        .and_then(|_| std::fs::rename(&tmp, &path))
        .is_err()
    {
        let _ = std::fs::remove_file(&tmp);
        return SaveOutcome::NoPath;
    }
    SaveOutcome::Saved(snapshot.generation)
}

/// Computes hunks turning `old` into `new`, in old-file line coordinates
/// (`git2::Patch`, the same primitive `services::git::gutter_marks` diffs
/// with). `None` if the buffers can't be diffed at all.
pub fn diff_hunks(old: &str, new: &str) -> Option<Vec<Hunk>> {
    let mut opts = git2::DiffOptions::new();
    opts.context_lines(0);
    let patch = git2::Patch::from_buffers(old.as_bytes(), None, new.as_bytes(), None, Some(&mut opts)).ok()?;
    let new_lines = split_lines_keep(new);
    let mut hunks = Vec::new();
    for h in 0..patch.num_hunks() {
        let (hunk, _) = patch.hunk(h).ok()?;
        let old_lines = hunk.old_lines() as usize;
        // A real (non-empty) old range is 1-based like a unified-diff header,
        // so it converts to a 0-based splice index the usual way. A pure
        // insertion (old_lines == 0) instead already reports the 0-based
        // splice position directly — subtracting 1 would shift it one line
        // early (verified against libgit2's actual output, not just its docs).
        let old_start = if old_lines == 0 {
            hunk.old_start() as usize
        } else {
            (hunk.old_start() as usize).saturating_sub(1)
        };
        let new_start = (hunk.new_start() as usize).saturating_sub(1);
        let new_count = hunk.new_lines() as usize;
        let text: String = new_lines
            .get(new_start..new_start + new_count)
            .map(|s| s.concat())
            .unwrap_or_default();
        hunks.push(Hunk { old_start, old_lines, new_text: text });
    }
    Some(hunks)
}

/// Reconstructs the dirty text by applying `hunks` (bottom-to-top, so earlier
/// offsets stay valid) over `base`. `None` if a hunk's range no longer fits —
/// the base changed too much since the hunks were computed — so the caller can
/// fall back to opening the file unmodified rather than risk corrupting it.
pub fn apply_hunks(base: &str, hunks: &[Hunk]) -> Option<String> {
    let mut lines = split_lines_keep(base);
    for h in hunks.iter().rev() {
        if h.old_start > lines.len() || h.old_start + h.old_lines > lines.len() {
            return None;
        }
        let piece = split_lines_keep(&h.new_text);
        lines.splice(h.old_start..h.old_start + h.old_lines, piece);
    }
    Some(lines.concat())
}

/// Splits `text` into lines that each keep their trailing `\n` (except
/// possibly the last), so concatenating them reconstructs the exact bytes.
fn split_lines_keep(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut start = 0;
    for (i, b) in text.bytes().enumerate() {
        if b == b'\n' {
            out.push(text[start..=i].to_string());
            start = i + 1;
        }
    }
    if start < text.len() {
        out.push(text[start..].to_string());
    }
    out
}

/// A fresh id for an untitled buffer's session key (no path to key on).
/// Time-based + a per-process counter: unique enough for this purpose without
/// pulling in a UUID dependency.
pub fn new_untitled_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{t:x}-{n:x}")
}

/// Serializes every test (in this module and `app::update::session`'s) that
/// sets `$CODER_SESSION_DIR` — an env var is process-global, and Rust runs
/// unit tests concurrently on threads of the same process, so two such tests
/// running at once could each see the other's directory mid-test.
#[cfg(test)]
pub(crate) static TEST_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_key_survives_a_same_filesystem_rename() {
        let base = std::env::temp_dir().join(format!("coder-session-test-{}", std::process::id()));
        let original = base.join("orig");
        std::fs::create_dir_all(&original).unwrap();
        let key_before = root_key(&original).unwrap();
        let renamed = base.join("renamed");
        std::fs::rename(&original, &renamed).unwrap();
        let key_after = root_key(&renamed).unwrap();
        assert_eq!(key_before, key_after, "renaming the folder must keep the same key");
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn diff_hunks_roundtrip_through_apply() {
        let old = "one\ntwo\nthree\nfour\n";
        let new = "one\nTWO\nthree\nFOUR\nfive\n";
        let hunks = diff_hunks(old, new).unwrap();
        assert_eq!(apply_hunks(old, &hunks).as_deref(), Some(new));
    }

    #[test]
    fn diff_hunks_handle_pure_insertion_and_deletion() {
        let old = "a\nb\nc\n";
        let inserted = "a\nb\nX\nc\n";
        let h = diff_hunks(old, inserted).unwrap();
        assert_eq!(apply_hunks(old, &h).as_deref(), Some(inserted));

        let deleted = "a\nc\n";
        let h2 = diff_hunks(old, deleted).unwrap();
        assert_eq!(apply_hunks(old, &h2).as_deref(), Some(deleted));
    }

    #[test]
    fn apply_hunks_rejects_a_base_that_no_longer_fits() {
        let old = "one\ntwo\nthree\n";
        let new = "one\nTWO\nthree\n";
        let hunks = diff_hunks(old, new).unwrap();
        // The "base" shrank too much since the hunk was computed.
        assert!(apply_hunks("one\n", &hunks).is_none());
    }

    #[test]
    fn save_then_load_roundtrips() {
        let _guard = super::TEST_ENV_LOCK.lock().unwrap();
        let dir = std::env::temp_dir().join(format!("coder-session-dir-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        unsafe {
            std::env::set_var("CODER_SESSION_DIR", &dir);
        }
        let root = dir.clone(); // any stat-able path works as a fake "root"
        let mut snap = SessionSnapshot {
            root: root.display().to_string(),
            generation: 0,
            active: Some("src/main.rs".into()),
            sidebar_panel: "files".into(),
            sidebar_width: 30,
            terminal_open: false,
            untitled_seq: 1,
            tabs: vec![TabEntry {
                kind: "file".into(),
                path: Some("src/main.rs".into()),
                untitled_id: None,
                label: None,
                line: 3,
                col: 1,
                scroll_y: 0,
                scroll_x: 0,
                dirty: true,
                content: Some(Content::Full { text: "fn main() {}\n".into() }),
            }],
        };
        match save(&root, &mut snap, 0) {
            SaveOutcome::Saved(g) => assert_eq!(g, 1),
            _ => panic!("expected Saved"),
        }
        let loaded = load(&root).unwrap();
        assert_eq!(loaded.generation, 1);
        assert_eq!(loaded.tabs[0].line, 3);
        unsafe {
            std::env::remove_var("CODER_SESSION_DIR");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_stale_generation_is_reported_as_a_conflict_not_overwritten() {
        let _guard = super::TEST_ENV_LOCK.lock().unwrap();
        let dir = std::env::temp_dir().join(format!("coder-session-conflict-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        unsafe {
            std::env::set_var("CODER_SESSION_DIR", &dir);
        }
        let root = dir.clone();
        let mut snap = SessionSnapshot {
            root: root.display().to_string(),
            generation: 0,
            active: None,
            sidebar_panel: "files".into(),
            sidebar_width: 30,
            terminal_open: false,
            untitled_seq: 0,
            tabs: Vec::new(),
        };
        // Instance A writes first: generation 0 -> 1.
        assert!(matches!(save(&root, &mut snap.clone(), 0), SaveOutcome::Saved(1)));
        // Instance B still thinks the generation is 0 (stale) -> conflict, not clobbered.
        match save(&root, &mut snap, 0) {
            SaveOutcome::Conflict(g) => assert_eq!(g, 1),
            _ => panic!("expected Conflict"),
        }
        unsafe {
            std::env::remove_var("CODER_SESSION_DIR");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
