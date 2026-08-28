//! Async file IO and directory scanning.

use std::path::{Path, PathBuf};

use anyhow::Result;

/// Scans a directory and returns (path, is_dir) entries. Directories first, then alphabetical.
pub fn scan_dir(dir: &Path) -> Result<Vec<(PathBuf, bool)>> {
    let mut entries: Vec<(PathBuf, bool)> = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        // Hide the .git directory.
        if path.file_name().map(|n| n == ".git").unwrap_or(false) {
            continue;
        }
        entries.push((path, is_dir));
    }
    entries.sort_by(|a, b| match (a.1, b.1) {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => a
            .0
            .file_name()
            .unwrap_or_default()
            .to_ascii_lowercase()
            .cmp(&b.0.file_name().unwrap_or_default().to_ascii_lowercase()),
    });
    Ok(entries)
}

/// Creates an empty file. Errors if the path already exists.
pub fn create_file(path: &Path) -> Result<()> {
    if path.exists() {
        anyhow::bail!("'{}' already exists", path.display());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::File::create(path)?;
    Ok(())
}

/// Creates a directory. Errors if the path already exists.
pub fn create_dir(path: &Path) -> Result<()> {
    if path.exists() {
        anyhow::bail!("'{}' already exists", path.display());
    }
    std::fs::create_dir_all(path)?;
    Ok(())
}

/// Renames `from` to `to`. Errors if `to` already exists (never overwrites).
pub fn rename_path(from: &Path, to: &Path) -> Result<()> {
    if to.exists() {
        anyhow::bail!("'{}' already exists", to.display());
    }
    std::fs::rename(from, to)?;
    Ok(())
}

/// Deletes a file, or a directory with everything under it.
pub fn delete_path(path: &Path) -> Result<()> {
    if path.is_dir() {
        std::fs::remove_dir_all(path)?;
    } else {
        std::fs::remove_file(path)?;
    }
    Ok(())
}

pub async fn read_file(path: &Path) -> Result<String> {
    Ok(tokio::fs::read_to_string(path).await?)
}

pub async fn write_file(path: &Path, contents: &str) -> Result<()> {
    tokio::fs::write(path, contents).await?;
    Ok(())
}
