//! Persisted user preferences in a hand-editable TOML file.
//!
//! The file holds the editor settings (theme, format-on-save flags) plus a
//! section per language declaring its LSP server / formatter / linter commands.
//! Each language is a top-level table keyed by its name:
//!
//! ```toml
//! theme = "base16-ocean.dark"
//! format_on_save = false
//!
//! [python]
//! extensions = ["py", "pyi"]
//! lsp = "ruff server"
//! formatter = "ruff format -"
//! linter = ""
//! ```
//!
//! `lsp`/`formatter`/`linter` are plain command strings (first word is the
//! binary, the rest are arguments); an empty string means "not configured".
//!
//! There are no in-code language fallbacks: the file is seeded once with the
//! sections below (`seed`) when it doesn't exist yet, and from then on the
//! languages come only from what the file declares.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use toml::Value;

use crate::core::highlight::DEFAULT_THEME;

/// Top-level keys that are editor settings, not language sections.
const SETTING_KEYS: [&str; 7] = [
    "theme",
    "format_on_save",
    "format_on_paste",
    "trim_trailing_whitespace",
    "insert_final_newline",
    "inline_diagnostics",
    "ascii_icons",
];

/// A single language's tooling, as written in its `[<name>]` section. The
/// command fields are shell-style strings ("" = disabled).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct LanguageConfig {
    /// File extensions (without the dot) this language claims.
    pub extensions: Vec<String>,
    /// Language server command, e.g. "rust-analyzer" ("" = none).
    pub lsp: String,
    /// Standalone formatter command (stdin -> stdout), e.g. "black - -q".
    pub formatter: String,
    /// Standalone linter command (stdin -> stdout diagnostics).
    pub linter: String,
}

/// User preferences serialized to `~/.config/coder/config.toml`.
#[derive(Debug, Clone)]
pub struct Config {
    /// Selected theme name.
    pub theme: String,
    /// Run the enabled format actions when saving.
 pub format_on_save: bool,
 /// Run the language formatter after a paste (off by default).
 pub format_on_paste: bool,
    /// Strip trailing whitespace on save.
    pub trim_trailing_whitespace: bool,
    /// Ensure a single final newline on save.
    pub insert_final_newline: bool,
    /// Append LSP error/warning messages at the end of their line, colored by
    /// severity (red/yellow).
    pub inline_diagnostics: bool,
    /// Use plain ASCII glyphs (e.g. "f"/"d" for new-file/new-folder) instead of
    /// Nerd Font / Codicon Private-Use-Area icons. The latter render as tiny
    /// fallback/placeholder glyphs in a terminal font that doesn't bundle those
    /// codepoints — this is the escape hatch, previously only reachable via
    /// the `CODER_ASCII` env var (still honored, and takes priority over this
    /// persisted setting when set).
    pub ascii_icons: bool,
    /// Language tooling, keyed by language name (the `[<name>]` section).
    pub languages: BTreeMap<String, LanguageConfig>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            theme: DEFAULT_THEME.to_string(),
            format_on_save: false,
 format_on_paste: false,
            trim_trailing_whitespace: true,
            insert_final_newline: true,
            inline_diagnostics: true,
            ascii_icons: false,
            // No languages by default; they live in the file (see `seed`).
            languages: BTreeMap::new(),
        }
    }
}

/// The config written to disk the first time it is missing: editor defaults plus
/// starter language sections. This is the *only* place language tooling is
/// hardcoded, and only to materialize an editable file — never a live fallback.
pub fn seed() -> Config {
    let lang = |exts: &[&str], lsp: &str, formatter: &str| LanguageConfig {
        extensions: exts.iter().map(|s| s.to_string()).collect(),
        lsp: lsp.to_string(),
        formatter: formatter.to_string(),
        linter: String::new(),
    };
    let mut languages = BTreeMap::new();
    languages.insert(
        "python".to_string(),
        lang(&["py", "pyi"], "ruff server", "ruff format -"),
    );
    languages.insert(
        "rust".to_string(),
        lang(&["rs"], "rust-analyzer", "rustfmt --edition 2021"),
    );
    Config {
        languages,
        ..Config::default()
    }
}

/// Path of the config file: `$CODER_CONFIG` override, else `~/.config/coder/config.toml`.
pub fn config_path() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("CODER_CONFIG") {
        return Some(PathBuf::from(p));
    }
    let home = std::env::var("HOME").ok()?;
    Some(PathBuf::from(home).join(".config/coder/config.toml"))
}

/// Parses config text. Unknown scalar keys are ignored; every table becomes a
/// language section. A file with no language sections yields no languages — the
/// file is authoritative, nothing is injected. Unparseable text yields defaults.
pub fn parse(text: &str) -> Config {
    let Ok(table) = text.parse::<toml::Table>() else {
        return Config::default();
    };
    let mut cfg = Config::default();
    cfg.languages.clear();

    if let Some(v) = table.get("theme").and_then(Value::as_str) {
        cfg.theme = v.to_string();
    }
	if let Some(v) = table.get("format_on_save").and_then(Value::as_bool) {
		cfg.format_on_save = v;
	}
	if let Some(v) = table.get("format_on_paste").and_then(Value::as_bool) {
		cfg.format_on_paste = v;
	}
    if let Some(v) = table.get("trim_trailing_whitespace").and_then(Value::as_bool) {
        cfg.trim_trailing_whitespace = v;
    }
    if let Some(v) = table.get("insert_final_newline").and_then(Value::as_bool) {
        cfg.insert_final_newline = v;
    }
    if let Some(v) = table.get("inline_diagnostics").and_then(Value::as_bool) {
        cfg.inline_diagnostics = v;
    }
    if let Some(v) = table.get("ascii_icons").and_then(Value::as_bool) {
        cfg.ascii_icons = v;
    }
    for (key, value) in &table {
        if SETTING_KEYS.contains(&key.as_str()) {
            continue;
        }
        if value.is_table()
            && let Ok(lc) = value.clone().try_into::<LanguageConfig>()
        {
            cfg.languages.insert(key.clone(), lc);
        }
    }
    cfg
}

/// Loads the config. When the file is missing it is seeded on disk with the
/// starter languages (see `seed`) and that seed is returned, so the languages
/// always come from the file — nothing is injected at runtime.
pub fn load() -> Config {
    let Some(path) = config_path() else {
        return Config::default();
    };
    match std::fs::read_to_string(&path) {
        Ok(text) => parse(&text),
        Err(_) => {
            let seed = seed();
            save(&seed);
            seed
        }
    }
}

/// Serializes the config to TOML: settings first, then a `[<name>]` table per
/// language. Building a `toml::Table` (rather than serializing the struct) keeps
/// the scalar settings ahead of the language tables, which TOML requires.
pub fn to_toml(config: &Config) -> String {
    let mut table = toml::Table::new();
    table.insert("theme".into(), Value::String(config.theme.clone()));
    table.insert("format_on_save".into(), Value::Boolean(config.format_on_save));
 table.insert("format_on_paste".into(), Value::Boolean(config.format_on_paste));
    table.insert(
        "trim_trailing_whitespace".into(),
        Value::Boolean(config.trim_trailing_whitespace),
    );
    table.insert(
        "insert_final_newline".into(),
        Value::Boolean(config.insert_final_newline),
    );
    table.insert(
        "inline_diagnostics".into(),
        Value::Boolean(config.inline_diagnostics),
    );
    table.insert("ascii_icons".into(), Value::Boolean(config.ascii_icons));
    for (name, lang) in &config.languages {
        if let Ok(v) = Value::try_from(lang) {
            table.insert(name.clone(), v);
        }
    }
    toml::to_string_pretty(&table).unwrap_or_default()
}

/// Writes the config to disk (creating the parent directory). Errors are ignored.
pub fn save(config: &Config) {
    let Some(path) = config_path() else {
        return;
    };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(&path, to_toml(config));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_has_no_languages() {
        assert!(Config::default().languages.is_empty());
    }

    #[test]
    fn seed_includes_rust_and_ruff_python() {
        let cfg = seed();
        assert!(cfg.languages.contains_key("rust"));
        assert_eq!(cfg.languages["python"].lsp, "ruff server");
        assert_eq!(cfg.languages["python"].formatter, "ruff format -");
    }

    #[test]
    fn parses_top_level_language_sections() {
        let cfg = parse(
            r#"
            theme = "my-theme"
            format_on_save = true

            [python]
            extensions = ["py"]
            lsp = "pyright-langserver --stdio"
            formatter = "ruff"
        "#,
        );
        assert_eq!(cfg.theme, "my-theme");
        assert!(cfg.format_on_save);
        assert_eq!(cfg.languages["python"].formatter, "ruff");
        assert_eq!(cfg.languages["python"].lsp, "pyright-langserver --stdio");
        // A file that declares its own languages replaces the defaults.
        assert!(!cfg.languages.contains_key("rust"));
    }

    #[test]
    fn a_file_with_no_language_sections_stays_empty() {
        let cfg = parse("theme = \"x\"\n");
        assert!(cfg.languages.is_empty());
    }

    #[test]
    fn round_trips_through_toml() {
        let cfg = seed();
        let restored = parse(&to_toml(&cfg));
        assert_eq!(restored.theme, cfg.theme);
        assert_eq!(restored.languages["rust"].lsp, cfg.languages["rust"].lsp);
        assert_eq!(
            restored.languages["python"].extensions,
            cfg.languages["python"].extensions
        );
    }

    #[test]
    fn ascii_icons_round_trips_through_toml() {
        // Explicitly non-default (true), so a broken parse/write can't hide
        // behind both sides coincidentally being `false`.
        let mut cfg = seed();
        cfg.ascii_icons = true;
        let restored = parse(&to_toml(&cfg));
        assert!(restored.ascii_icons);
    }

    #[test]
    fn ascii_icons_parses_from_toml() {
        let cfg = parse("ascii_icons = true\n");
        assert!(cfg.ascii_icons);
    }
}
