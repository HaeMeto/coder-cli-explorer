//! Language tooling: maps a language to an LSP server plus optional standalone
//! formatter/linter commands.
//!
//! The definitions come from the main config file (`config.toml`), one
//! `[<name>]` section per language. The user installs the actual binaries
//! (rust-analyzer, pyright, black, ruff); the editor only orchestrates them.
//! See `crate::services::config` for the on-disk format and built-in defaults.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use crate::services::config::LanguageConfig;

/// One language group: a name and the languages it supports (currently always a
/// single language, mirroring a `[<name>]` config section).
#[derive(Debug, Clone)]
pub struct ExtensionManifest {
    pub name: String,
    pub languages: Vec<LanguageDef>,
}

/// A single language's capabilities.
#[derive(Debug, Clone)]
pub struct LanguageDef {
    /// LSP language identifier (e.g. "rust", "python").
    pub id: String,
    /// File extensions (without the dot) this language claims.
    pub extensions: Vec<String>,
    /// Language server to launch (stdio JSON-RPC).
    pub lsp: Option<ServerSpec>,
    /// Standalone formatter: stdin -> stdout (used when the LSP can't format).
    pub formatter: Option<ToolSpec>,
    /// Standalone linter: stdin -> stdout diagnostics (for LSP-less languages).
    pub linter: Option<ToolSpec>,
}

/// A language-server command.
#[derive(Debug, Clone)]
pub struct ServerSpec {
    pub command: String,
    pub args: Vec<String>,
    /// Extra environment variables passed to the server process.
    pub env: Vec<(String, String)>,
}

/// A standalone command-line tool that reads stdin and writes stdout.
#[derive(Debug, Clone)]
pub struct ToolSpec {
    pub command: String,
    pub args: Vec<String>,
}

/// Splits a shell-style command string into (binary, args). Returns `None` for
/// an empty / whitespace-only string ("not configured"). Quoting is not handled
/// (whitespace-split only), which is enough for the tools we invoke.
fn parse_command(spec: &str) -> Option<(String, Vec<String>)> {
    let mut parts = spec.split_whitespace();
    let command = parts.next()?.to_string();
    let args = parts.map(str::to_string).collect();
    Some((command, args))
}

/// All loaded extensions plus a fast extension -> language lookup.
#[derive(Debug, Clone, Default)]
pub struct ExtensionRegistry {
    manifests: Vec<ExtensionManifest>,
    /// Maps a file extension to `(manifest index, language index)`.
    by_ext: HashMap<String, (usize, usize)>,
}

impl ExtensionRegistry {
    /// The installed extensions, for the Extensions panel.
    pub fn manifests(&self) -> &[ExtensionManifest] {
        &self.manifests
    }

    /// Builds a registry from config language sections. Each `[<name>]` becomes
    /// a one-language manifest; the command strings are split into binary+args.
    pub fn from_config(languages: &BTreeMap<String, LanguageConfig>) -> Self {
        let manifests = languages
            .iter()
            .map(|(name, lc)| {
                let lsp = parse_command(&lc.lsp).map(|(command, args)| ServerSpec {
                    command,
                    args,
                    env: Vec::new(),
                });
                let formatter =
                    parse_command(&lc.formatter).map(|(command, args)| ToolSpec { command, args });
                let linter =
                    parse_command(&lc.linter).map(|(command, args)| ToolSpec { command, args });
                ExtensionManifest {
                    name: name.clone(),
                    languages: vec![LanguageDef {
                        id: name.clone(),
                        extensions: lc.extensions.clone(),
                        lsp,
                        formatter,
                        linter,
                    }],
                }
            })
            .collect();
        let mut registry = ExtensionRegistry {
            manifests,
            by_ext: HashMap::new(),
        };
        registry.reindex();
        registry
    }

    /// Reconstructs the config language sections from the loaded registry, so a
    /// settings save (e.g. a theme change) re-persists the languages unchanged.
    pub fn to_language_configs(&self) -> BTreeMap<String, LanguageConfig> {
        let join = |command: &str, args: &[String]| {
            if args.is_empty() {
                command.to_string()
            } else {
                format!("{command} {}", args.join(" "))
            }
        };
        let mut map = BTreeMap::new();
        for manifest in &self.manifests {
            for lang in &manifest.languages {
                map.insert(
                    manifest.name.clone(),
                    LanguageConfig {
                        extensions: lang.extensions.clone(),
                        lsp: lang.lsp.as_ref().map(|s| join(&s.command, &s.args)).unwrap_or_default(),
                        formatter: lang
                            .formatter
                            .as_ref()
                            .map(|t| join(&t.command, &t.args))
                            .unwrap_or_default(),
                        linter: lang
                            .linter
                            .as_ref()
                            .map(|t| join(&t.command, &t.args))
                            .unwrap_or_default(),
                    },
                );
            }
        }
        map
    }

    /// Every distinct binary referenced by the configured languages (lsp +
    /// formatter + linter commands), sorted and de-duplicated. Used to probe
    /// which tools are actually installed on PATH.
    pub fn tool_commands(&self) -> Vec<String> {
        let mut cmds: Vec<String> = self
            .manifests
            .iter()
            .flat_map(|m| &m.languages)
            .flat_map(|l| {
                [
                    l.lsp.as_ref().map(|s| s.command.clone()),
                    l.formatter.as_ref().map(|t| t.command.clone()),
                    l.linter.as_ref().map(|t| t.command.clone()),
                ]
            })
            .flatten()
            .collect();
        cmds.sort();
        cmds.dedup();
        cmds
    }

    /// Resolves the language for a file by its extension.
    pub fn language_for_path(&self, path: &Path) -> Option<&LanguageDef> {
        let ext = path.extension().and_then(|e| e.to_str())?;
        let &(m, l) = self.by_ext.get(ext)?;
        Some(&self.manifests[m].languages[l])
    }

    /// Builds the extension -> language index from the current manifests. A later
    /// manifest wins over an earlier one for the same extension.
    fn reindex(&mut self) {
        self.by_ext.clear();
        for (m, manifest) in self.manifests.iter().enumerate() {
            for (l, lang) in manifest.languages.iter().enumerate() {
                for ext in &lang.extensions {
                    self.by_ext.insert(ext.clone(), (m, l));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::config;

    #[test]
    fn config_defaults_resolve_by_extension() {
        let reg = ExtensionRegistry::from_config(&config::seed().languages);
        assert_eq!(
            reg.language_for_path(Path::new("main.rs")).map(|l| &l.id[..]),
            Some("rust")
        );
        assert_eq!(
            reg.language_for_path(Path::new("a/b/app.py"))
                .map(|l| &l.id[..]),
            Some("python")
        );
        assert!(reg.language_for_path(Path::new("notes.txt")).is_none());
        assert_eq!(
            reg.language_for_path(Path::new("main.rs"))
                .and_then(|l| l.lsp.as_ref())
                .map(|s| &s.command[..]),
            Some("rust-analyzer")
        );
    }

    #[test]
    fn command_string_splits_into_binary_and_args() {
        let cfg = config::parse(
            r#"
            [rust]
            extensions = ["rs"]
            formatter = "rustfmt --edition 2021"
        "#,
        );
        let reg = ExtensionRegistry::from_config(&cfg.languages);
        let lang = reg.language_for_path(Path::new("x.rs")).unwrap();
        let fmt = lang.formatter.as_ref().unwrap();
        assert_eq!(fmt.command, "rustfmt");
        assert_eq!(fmt.args, ["--edition", "2021"]);
        // Empty command string => not configured.
        assert!(lang.lsp.is_none());
    }

    #[test]
    fn to_language_configs_round_trips_commands() {
        let reg = ExtensionRegistry::from_config(&config::seed().languages);
        let langs = reg.to_language_configs();
        assert_eq!(langs["rust"].lsp, "rust-analyzer");
        assert_eq!(langs["rust"].formatter, "rustfmt --edition 2021");
        assert_eq!(langs["python"].lsp, "ruff server");
        assert_eq!(langs["python"].formatter, "ruff format -");
    }
}
