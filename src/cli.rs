//! Command-line interface: argument parsing (clap), shell-completion script
//! generation, and the `complete -C` helper that powers file-name completion.

use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueHint};
use clap_complete::Shell;

#[derive(Parser)]
#[command(
    name = "coder",
    version,
    about = "Lightweight Terminal IDE like VSCode",
    long_about = "Lightweight Terminal IDE like VSCode. Supports mouse clicks, git, autocomplete, LSP and more.",
    args_conflicts_with_subcommands = true
)]
pub struct Cli {
    /// File or directory to open (defaults to the current directory).
    #[arg(value_name = "PATH", value_hint = ValueHint::AnyPath)]
    pub path: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand)]
pub enum Command {
    /// Print a shell completion script, e.g. `coder completions bash`.
    Completions {
        /// Shell to generate the script for (bash, zsh, fish, ...).
        shell: Shell,
    },
}

/// Prints the completion script for `shell` to stdout.
pub fn print_completions(shell: Shell) {
    use clap::CommandFactory;
    let mut cmd = Cli::command();
    clap_complete::generate(shell, &mut cmd, "coder", &mut std::io::stdout());
}

/// True when the shell invoked us as a `complete -C` completion helper.
pub fn is_completion_helper() -> bool {
    std::env::var_os("COMP_LINE").is_some()
}

/// Emits file/dir candidates for the word being completed, then returns. Never
/// launches the UI — otherwise Tab would leave the shell frozen behind our
/// full-screen alternate buffer.
pub fn completion_reply() {
    // `complete -C cmd`: argv = [cmd, current_word, previous_word].
    let word = std::env::args().nth(2).unwrap_or_default();
    for cand in path_candidates(&word) {
        println!("{cand}");
    }
}

/// Lists filesystem entries matching a half-typed path (`src/ma` -> `src/main.rs`).
fn path_candidates(word: &str) -> Vec<String> {
    // Split the typed word into the directory to list and the name prefix.
    let (dir_part, prefix) = match word.rfind('/') {
        Some(i) => (&word[..=i], &word[i + 1..]),
        None => ("", word),
    };

    // Expand a leading `~/` to $HOME for the directory we actually read.
    let read_dir = if let Some(rest) = dir_part.strip_prefix("~/") {
        match std::env::var_os("HOME") {
            Some(home) => PathBuf::from(home).join(rest),
            None => PathBuf::from(dir_part),
        }
    } else if dir_part.is_empty() {
        PathBuf::from(".")
    } else {
        PathBuf::from(dir_part)
    };

    let Ok(entries) = std::fs::read_dir(&read_dir) else {
        return Vec::new();
    };

    let mut out: Vec<String> = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        // Only offer dotfiles once the user has typed a leading dot.
        if name.starts_with('.') && !prefix.starts_with('.') {
            continue;
        }
        if !name.starts_with(prefix) {
            continue;
        }
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        let mut cand = format!("{dir_part}{name}");
        if is_dir {
            cand.push('/');
        }
        out.push(cand);
    }
    out.sort();
    out
}
