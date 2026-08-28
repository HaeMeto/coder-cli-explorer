//! Off-thread syntax highlighting.
//!
//! Highlighting runs on a dedicated worker thread that owns the incremental
//! `Highlighter`, so the render loop never blocks on syntect (which is O(n²) on
//! some syntaxes — a bracket-heavy Markdown line alone can cost tens of ms). The
//! main thread renders text immediately, uncolored, and swaps in colors when the
//! worker's `Msg::Highlighted` arrives a frame or two later.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender};

use tokio::sync::mpsc::UnboundedSender;

use crate::app::msg::Msg;
use crate::core::highlight::Highlighter;

/// A request to highlight the active buffer down to the viewport bottom. The main
/// thread emits one whenever the text or the visible range changes.
pub struct HlJob {
    /// Which tab and version this text belongs to (echoed back so a stale result
    /// can be dropped once the buffer has moved on).
    pub tab: usize,
    pub version: u64,
    /// The file path (selects the syntax) and the current UI theme name.
    pub path: Option<PathBuf>,
    pub theme_name: String,
    /// Drop the worker's cache before highlighting (content replaced: reload /
    /// format / theme change).
    pub reset: bool,
    /// The full buffer text and the incremental hints from `Buffer::take_dirty`.
    pub text: String,
    pub dirty_from: usize,
    pub wide: bool,
    /// First visible line — the base index of the slice sent back (lines above the
    /// viewport are never drawn, so they are not shipped across the channel).
    pub scroll_y: usize,
    /// How many lines from the top must be colored this frame.
    pub needed: usize,
}

/// Spawns the highlight worker and returns the channel to submit jobs on. Results
/// come back as `Msg::Highlighted` on `tx`.
pub fn spawn(tx: UnboundedSender<Msg>) -> Sender<HlJob> {
    let (job_tx, job_rx) = std::sync::mpsc::channel::<HlJob>();
    std::thread::spawn(move || worker_loop(job_rx, tx));
    job_tx
}

fn worker_loop(job_rx: Receiver<HlJob>, tx: UnboundedSender<Msg>) {
    let mut hl: Option<Highlighter> = None;
    let mut cur_path: Option<PathBuf> = None;

    while let Ok(job) = job_rx.recv() {
        let job = coalesce(job, &job_rx);

        // (Re)build the highlighter when the file (and thus syntax) changes.
        if hl.is_none() || cur_path != job.path {
            hl = Some(Highlighter::for_path(job.path.as_deref()));
            cur_path = job.path.clone();
        }
        let h = hl.as_mut().expect("just set");
        if job.reset {
            h.invalidate();
        }
        h.set_theme(&job.theme_name);
        h.highlight(&job.text, job.version, job.dirty_from, job.wide, job.needed);

        // Ship only the visible slice (base = scroll_y); off-screen prefix lines are
        // never rendered, so cloning them across the channel would be waste.
        let cached = h.cached();
        let base = job.scroll_y.min(cached.len());
        let lines = cached[base..].to_vec();
        if tx
            .send(Msg::Highlighted {
                tab: job.tab,
                version: job.version,
                base,
                lines,
            })
            .is_err()
        {
            return; // receiver gone: app shutting down
        }
    }
}

/// Drains any jobs already queued behind `job`, keeping the newest text/version
/// but merging the dirty range: the lowest `dirty_from`, and `wide` if any job was
/// wide OR the coalesced edits touched different lines (so the whole tail rebuilds
/// rather than trusting single-line convergence across a skipped edit).
fn coalesce(mut job: HlJob, job_rx: &Receiver<HlJob>) -> HlJob {
    let mut min_dirty = job.dirty_from;
    let mut wide = job.wide;
    while let Ok(next) = job_rx.try_recv() {
        if next.dirty_from != min_dirty {
            wide = true;
        }
        min_dirty = min_dirty.min(next.dirty_from);
        wide |= next.wide;
        job = next;
    }
    job.dirty_from = min_dirty;
    job.wide = wide;
    job
}
