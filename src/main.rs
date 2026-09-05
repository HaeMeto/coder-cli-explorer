//! coder — a VSCode-like editor running in the terminal (Rust + ratatui + Elm Architecture).

mod app;
mod cli;
mod core;
mod services;
mod ui;

use std::io::{self, Stdout};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture, Event,
    EventStream, KeyEventKind, MouseEventKind, KeyboardEnhancementFlags,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
    supports_keyboard_enhancement,
};
use futures::StreamExt;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use tokio::sync::mpsc::{self, UnboundedSender};

use crate::app::cmd::{self, Cmd};
use crate::app::model::Model;
use crate::app::msg::Msg;
use crate::app::update::{self, update};

type Tui = Terminal<CrosstermBackend<Stdout>>;

#[tokio::main]
async fn main() -> Result<()> {
    // The shell invoked us as a completion helper (`complete -C coder coder`):
    // reply with path candidates and exit before touching the terminal, so Tab
    // never launches the TUI and freezes the shell.
    if cli::is_completion_helper() {
        cli::completion_reply();
        return Ok(());
    }

    let args = <cli::Cli as clap::Parser>::parse();
    if let Some(cli::Command::Completions { shell }) = args.command {
        cli::print_completions(shell);
        return Ok(());
    }

    // The argument may be a directory (workspace root) or a single file.
    let arg = args.path;
    let (root, open_file) = match arg {
        Some(p) => {
            let p = p.canonicalize().unwrap_or(p);
            if p.is_file() {
                // Opened with a file: root is its directory, and we open the file.
                let parent = p
                    .parent()
                    .map(|d| d.to_path_buf())
                    .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| ".".into()));
                (parent, Some(p))
            } else {
                (p, None)
            }
        }
        None => (std::env::current_dir().unwrap_or_else(|_| ".".into()), None),
    };
    let root = root.canonicalize().unwrap_or(root);

    let mut terminal = setup_terminal()?;
    let result = run(&mut terminal, root, open_file).await;
    restore_terminal(&mut terminal)?;
    result
}

/// Whether the kitty keyboard enhancement flags were pushed at startup. Probed
/// once in `setup_terminal`; `restore_terminal` reads this instead of querying
/// the terminal again at shutdown — a second query leaks its Device Attributes
/// reply (e.g. `61;1;21;22;28c`) to the shell after raw mode is off.
static KEYBOARD_ENHANCED: AtomicBool = AtomicBool::new(false);

fn setup_terminal() -> Result<Tui> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    // Bracketed paste makes the terminal wrap a pasted block in
    // ESC[200~ … ESC[201~ and deliver it as one `Event::Paste(String)`, instead
    // of a flood of ordinary key events. Without this, a multi-line paste (e.g.
    // JSON from outside the app) is typed character-by-character and collides
    // with the editor's auto-indent-on-Enter, producing cascading/staircase
    // indentation — see `map_event`'s `Event::Paste` arm.
    execute!(
        stdout,
        EnterAlternateScreen,
        EnableMouseCapture,
        EnableBracketedPaste
    )?;

    // Ask the terminal to disambiguate escape codes (kitty keyboard protocol) so
    // modified keys like Ctrl+Tab / Ctrl+Shift+Tab arrive with their modifiers
    // instead of collapsing to a bare Tab. Only where the terminal supports it.
    let enhanced = supports_keyboard_enhancement().unwrap_or(false);
    KEYBOARD_ENHANCED.store(enhanced, Ordering::Relaxed);
    if enhanced {
        execute!(
            stdout,
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        )?;
    }

    // Restore the terminal on panic.
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        if enhanced {
            let _ = execute!(io::stdout(), PopKeyboardEnhancementFlags);
        }
        let _ = execute!(
            io::stdout(),
            DisableBracketedPaste,
            LeaveAlternateScreen,
            DisableMouseCapture
        );
        original_hook(info);
    }));

    let backend = CrosstermBackend::new(stdout);
    Ok(Terminal::new(backend)?)
}

fn restore_terminal(terminal: &mut Tui) -> Result<()> {
    // Pop the flags we actually pushed (tracked at setup); do NOT re-probe here —
    // querying the terminal at shutdown leaks its reply to the shell.
    if KEYBOARD_ENHANCED.load(Ordering::Relaxed) {
        execute!(terminal.backend_mut(), PopKeyboardEnhancementFlags)?;
    }
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        DisableBracketedPaste,
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;
    Ok(())
}

async fn run(
    terminal: &mut Tui,
    root: std::path::PathBuf,
    open_file: Option<std::path::PathBuf>,
) -> Result<()> {
    // Build the syntax/theme sets on a background thread so the first file open
    // doesn't pay the ~500ms deserialization cost on the render path.
    std::thread::spawn(crate::core::highlight::warm);

    let (tx, mut rx) = mpsc::unbounded_channel::<Msg>();
    let mut model = Model::new(root.clone());

    // Syntax highlighting runs on its own worker thread; results arrive as
    // `Msg::Highlighted`. The render loop never blocks on syntect this way.
    model.set_hl_worker(crate::app::hlworker::spawn(tx.clone()));

    // Initial size.
    if let Ok((w, h)) = crossterm::terminal::size() {
        model.term_size = (w, h);
    }

    // Watch the workspace for external file changes (best-effort; kept alive here).
    // Directories are watched non-recursively and lazily, one per scan, so opening a
    // large tree (e.g. $HOME from the app menu) never blocks startup.
    let mut watcher = create_watcher(tx.clone());

    // Initial side effects: scan the root directory + load git status + probe
    // which configured language tools are installed.
    let mut cmds = vec![
        Cmd::ScanDir(root.clone()),
        Cmd::LoadGitStatus,
        Cmd::CheckTools(model.extensions.tool_commands()),
    ];
    // Opened with a file: keep the sidebar collapsed (the user opens it when needed)
    // and load the file straight into the editor.
    if let Some(file) = open_file {
        model.layout.sidebar_open = false;
        model.focus = crate::app::model::Focus::Editor;
        cmds.push(Cmd::ReadFile(file));
    }
    dispatch(cmds, &model, &tx);

    // Forward terminal events into the same channel as internal messages, so the
    // main loop has one source to drain and never needs `tokio::select!`. Events
    // and async results are then handled in arrival order, all pending ones per
    // iteration, with a single render per batch.
    spawn_event_forwarder(tx.clone());

    // How long the loop waits for a message before waking on its own to fire any
    // due debounce (autocomplete / didChange) and repaint. Keeps the UI live even
    // with no input, and bounds how late a deadline fires to one tick.
    const TICK: Duration = Duration::from_millis(50);

    // Cap on how many already-queued messages are drained before the next
    // render/should_quit check. A continuous flood — a chatty command running
    // in the embedded terminal (`cargo build -v`, `ping`, `tail -f`, ...), or a
    // burst of disk-watch events — can otherwise refill the channel faster than
    // it empties, so the plain `while let Ok(msg) = rx.try_recv()` never exits:
    // the screen stops redrawing and even an already-queued Quit/Save sits
    // unprocessed until the flood stops. Bounding the batch guarantees the loop
    // comes back to redraw (and check `should_quit`) at least this often.
    const MAX_DRAIN_PER_TICK: usize = 256;

    loop {
        // Fire any debounced work whose deadline elapsed (checked every iteration
        // instead of spawning a timer task per keystroke).
        let cmds = update::tick(&mut model);
        dispatch(cmds, &model, &tx);

        model.refresh_highlight();
        model.refresh_git_marks();
        terminal.draw(|f| ui::view(f, &model))?;
        if model.should_quit {
            break;
        }

        // Wait for the next message, but wake at least every TICK so a pending
        // deadline still fires when no event arrives. On timeout just loop.
        match tokio::time::timeout(TICK, rx.recv()).await {
            Ok(Some(msg)) => {
                handle_msg(&mut model, &mut watcher, &tx, msg);
                // Drain more already-queued messages before the next render (a
                // keystroke burst, heavy PTY output, batched async results), but
                // bounded and cut short the moment Quit fires — see
                // `MAX_DRAIN_PER_TICK` above.
                let mut drained = 0;
                while drained < MAX_DRAIN_PER_TICK && !model.should_quit {
                    let Ok(msg) = rx.try_recv() else { break };
                    handle_msg(&mut model, &mut watcher, &tx, msg);
                    drained += 1;
                }
            }
            Ok(None) => break, // channel closed
            Err(_) => {}       // idle tick: just loop and repaint
        }
    }
    Ok(())
}

/// Applies one message: watch bookkeeping, `update`, then dispatch its commands.
fn handle_msg(
    model: &mut Model,
    watcher: &mut Option<notify::RecommendedWatcher>,
    tx: &UnboundedSender<Msg>,
    msg: Msg,
) {
    watch_scanned_dir(watcher, &msg);
    let cmds = update(model, msg);
    dispatch(cmds, model, tx);
}

/// Reads the crossterm event stream on its own task and forwards each event into
/// the message channel as a `Msg`. When the stream ends or errors (terminal
/// closed), it sends `Msg::Quit` so the main loop — which only awaits `rx` — exits
/// instead of blocking forever.
fn spawn_event_forwarder(tx: UnboundedSender<Msg>) {
    tokio::spawn(async move {
        let mut events = EventStream::new();
        loop {
            match events.next().await {
                Some(Ok(event)) => {
                    for msg in map_event(event) {
                        if tx.send(msg).is_err() {
                            return; // receiver gone: app is shutting down
                        }
                    }
                }
                Some(Err(_)) | None => {
                    let _ = tx.send(Msg::Quit);
                    return;
                }
            }
        }
    });
}

/// Adds a non-recursive watch on a directory as soon as it is scanned, so changes
/// in the folders the user actually opened are picked up — without walking the
/// whole tree up front.
fn watch_scanned_dir(watcher: &mut Option<notify::RecommendedWatcher>, msg: &Msg) {
    use notify::{RecursiveMode, Watcher};
    if let (Some(w), Msg::DirScanned { path, .. }) = (watcher.as_mut(), msg) {
        let _ = w.watch(path, RecursiveMode::NonRecursive);
    }
}

/// Builds a filesystem watcher; each change becomes a `Msg::DiskChanged`. No paths
/// are watched yet — directories are added non-recursively as they are scanned (see
/// `watch_scanned_dir`), so startup never walks the whole tree. Returns the watcher
/// (must stay alive to keep watching); `None` if the platform watcher failed.
fn create_watcher(tx: UnboundedSender<Msg>) -> Option<notify::RecommendedWatcher> {
    use notify::EventKind;
    notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if let Ok(event) = res
            && matches!(
                event.kind,
                EventKind::Modify(_) | EventKind::Create(_) | EventKind::Remove(_)
            )
        {
            for path in event.paths {
                let _ = tx.send(Msg::DiskChanged(path));
            }
        }
    })
    .ok()
}

/// Converts a crossterm event into application messages.
fn map_event(event: Event) -> Vec<Msg> {
    match event {
        Event::Key(key) => {
            // Only handle press/repeat (ignore release events).
            if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
                vec![Msg::Key(key)]
            } else {
                Vec::new()
            }
        }
        // Bare mouse movement (no button held) does nothing — drop it here so it
        // never wakes the loop or forces a redraw. Drags (move with a button) still
        // come through for selection / panel resize.
        Event::Mouse(m) if matches!(m.kind, MouseEventKind::Moved) => Vec::new(),
        Event::Mouse(m) => vec![Msg::Mouse(m)],
        Event::Resize(w, h) => vec![Msg::Resize(w, h)],
        // A whole pasted block, delivered atomically thanks to bracketed paste
        // (see `setup_terminal`) instead of as a flood of individual key events.
        Event::Paste(text) => vec![Msg::Paste(text)],
        _ => Vec::new(),
    }
}

fn dispatch(cmds: Vec<Cmd>, model: &Model, tx: &UnboundedSender<Msg>) {
    for c in cmds {
        cmd::execute(c, model.root.clone(), tx.clone());
    }
}
