//! Generates the shell-completion scripts at build time so the .deb / .rpm
//! packages can ship them (see `[package.metadata.deb]` in Cargo.toml).
//!
//! Output goes to `target/completions/` (a fixed path, not `OUT_DIR`, because
//! the packaging metadata has to name the files).

use std::path::PathBuf;

use clap::CommandFactory;
use clap_complete::Shell;

#[allow(dead_code)]
#[path = "src/cli.rs"]
mod cli;

fn main() {
    println!("cargo:rerun-if-changed=src/cli.rs");
    println!("cargo:rerun-if-changed=build.rs");

    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/completions");
    if let Err(e) = std::fs::create_dir_all(&out) {
        println!("cargo:warning=cannot create {}: {e}", out.display());
        return;
    }

    let mut cmd = cli::Cli::command();
    for shell in [Shell::Bash, Shell::Zsh, Shell::Fish] {
        if let Err(e) = clap_complete::generate_to(shell, &mut cmd, "coder", &out) {
            println!("cargo:warning=completion generation failed for {shell}: {e}");
        }
    }
}
