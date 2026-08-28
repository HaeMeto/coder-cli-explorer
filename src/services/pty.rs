//! A real shell PTY session via portable-pty.

use std::io::{Read, Write};

use anyhow::Result;
use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};
use tokio::sync::mpsc::UnboundedSender;

use crate::app::msg::Msg;

/// An open PTY session; held inside the Model.
pub struct PtySession {
    pub writer: Box<dyn Write + Send>,
    master: Box<dyn MasterPty + Send>,
    _child: Box<dyn portable_pty::Child + Send + Sync>,
}

impl PtySession {
    pub fn write(&mut self, data: &[u8]) {
        let _ = self.writer.write_all(data);
        let _ = self.writer.flush();
    }

    pub fn resize(&self, rows: u16, cols: u16) {
        let _ = self.master.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        });
    }
}

/// Opens a new PTY, starts the user's shell, and sets up the reader thread.
/// Bytes read are sent as `Msg::PtyOutput`, and closure as `Msg::PtyExited`.
pub fn spawn(
    cwd: std::path::PathBuf,
    rows: u16,
    cols: u16,
    tx: UnboundedSender<Msg>,
) -> Result<PtySession> {
    let pty_system = native_pty_system();
    let pair = pty_system.openpty(PtySize {
        rows,
        cols,
        pixel_width: 0,
        pixel_height: 0,
    })?;

    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
    let mut cmd = CommandBuilder::new(shell);
    cmd.cwd(cwd);
    cmd.env("TERM", "xterm-256color");

    let child = pair.slave.spawn_command(cmd)?;
    drop(pair.slave);

    let mut reader = pair.master.try_clone_reader()?;
    let writer = pair.master.take_writer()?;

    // Blocking read on a separate thread; forwards bytes to the channel.
    std::thread::spawn(move || {
        let mut buf = [0u8; 8192];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if tx.send(Msg::PtyOutput(buf[..n].to_vec())).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        let _ = tx.send(Msg::PtyExited);
    });

    Ok(PtySession {
        writer,
        master: pair.master,
        _child: child,
    })
}
