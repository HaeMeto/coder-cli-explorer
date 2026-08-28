//! Extensions panel: the languages defined in `config.toml`, each with the live
//! status of its LSP server and formatter/linter tools.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::model::Model;
use crate::core::theme::Theme;
use crate::services::extensions::{LanguageDef, ToolSpec};

pub(super) fn render(frame: &mut Frame, area: Rect, model: &Model) {
    let th = &model.theme;
    let mut lines: Vec<Line> = Vec::new();

    let manifests = model.extensions.manifests();
    if manifests.is_empty() {
        lines.push(Line::from(Span::styled(
            " No languages configured.",
            Style::new().fg(th.fg_dim),
        )));
    }

    for manifest in manifests {
        for lang in &manifest.languages {
            // Language name header.
            lines.push(Line::from(Span::styled(
                format!(" {}", lang.id),
                Style::new().fg(th.fg),
            )));
            lines.push(lsp_row(model, lang));
            lines.push(tool_row(th, "fmt", lang.formatter.as_ref(), model));
            if lang.linter.is_some() {
                lines.push(tool_row(th, "lint", lang.linter.as_ref(), model));
            }
            lines.push(Line::from(""));
        }
    }

    lines.push(Line::from(Span::styled(
        " ● running  ◐ starting  ○ idle  ✓ installed  ✗ missing",
        Style::new().fg(th.fg_dim),
    )));
    lines.push(Line::from(Span::styled(
        " Edit languages in config.toml (⚙ Settings)",
        Style::new().fg(th.fg_dim),
    )));

    let p = Paragraph::new(lines).style(Style::new().bg(th.bg_alt));
    frame.render_widget(p, area);
}

/// The LSP row: glyph reflects the server's runtime state (running / starting /
/// idle), or install state when it has never been started. Shows the diagnostic
/// count for the language when there is one.
fn lsp_row(model: &Model, lang: &LanguageDef) -> Line<'static> {
    let th = &model.theme;
    let Some(spec) = lang.lsp.as_ref() else {
        return not_configured(th, "lsp");
    };
    let id = &lang.id;
    let (glyph, color) = if model.lsp.initialized.contains(id) {
        ("●", th.git_added)
    } else if model.lsp.sessions.contains_key(id) || model.lsp.starting.contains(id) {
        ("◐", th.git_modified)
    } else {
        // Not started yet: fall back to whether the binary is even installed.
        match model.tool_available.get(&spec.command) {
            Some(false) => ("✗", th.git_deleted),
            _ => ("○", th.fg_dim),
        }
    };

    let mut spans = vec![
        Span::styled(format!("   {glyph} "), Style::new().fg(color)),
        Span::styled("lsp  ", Style::new().fg(th.fg_dim)),
        Span::styled(join(&spec.command, &spec.args), Style::new().fg(th.fg)),
    ];
    let diag_count: usize = model
        .diagnostics
        .iter()
        .filter(|(p, _)| {
            model
                .extensions
                .language_for_path(p)
                .map(|l| l.id == *id)
                .unwrap_or(false)
        })
        .map(|(_, d)| d.len())
        .sum();
    if diag_count > 0 {
        spans.push(Span::styled(
            format!("  {diag_count} diag"),
            Style::new().fg(th.fg_dim),
        ));
    }
    Line::from(spans)
}

/// A formatter/linter row: these are one-shot commands, so the status is simply
/// whether the binary is installed on PATH (`✓`/`✗`, `○` before it's probed).
fn tool_row(th: &Theme, label: &str, spec: Option<&ToolSpec>, model: &Model) -> Line<'static> {
    let Some(spec) = spec else {
        return not_configured(th, label);
    };
    let (glyph, color) = match model.tool_available.get(&spec.command) {
        Some(true) => ("✓", th.git_added),
        Some(false) => ("✗", th.git_deleted),
        None => ("○", th.fg_dim),
    };
    Line::from(vec![
        Span::styled(format!("   {glyph} "), Style::new().fg(color)),
        Span::styled(format!("{label}  "), Style::new().fg(th.fg_dim)),
        Span::styled(join(&spec.command, &spec.args), Style::new().fg(th.fg)),
    ])
}

/// A dimmed row for a capability the language doesn't declare.
fn not_configured(th: &Theme, label: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled("   ─ ", Style::new().fg(th.fg_dim)),
        Span::styled(format!("{label}  "), Style::new().fg(th.fg_dim)),
        Span::styled("not set", Style::new().fg(th.fg_dim)),
    ])
}

/// Rejoins a command and its args back into a display string.
fn join(command: &str, args: &[String]) -> String {
    if args.is_empty() {
        command.to_string()
    } else {
        format!("{command} {}", args.join(" "))
    }
}
