//! Reusable text-input widget: renders a [`TextInputState`] into a `Rect`.
//!
//! Multi-line and vertically scrollable — content is wrapped to the area width,
//! and the visible window scrolls so the caret's row is always shown. Single-line
//! fields are just the degenerate case (one logical line, area height 1).
//!
//! The logical state (content + caret) lives in [`TextInputState`]; this only
//! draws it, so it fits the read-only Elm `view` (`Widget`, not `StatefulWidget`).

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};

use crate::core::text_input::TextInputState;
use crate::core::theme::Theme;

/// Caret placement within a wrapped display row.
enum Caret {
    /// Over the char at this column (0-based within the row's content).
    Over(usize),
    /// A block just past the last char.
    End,
}

pub struct TextInput<'a> {
    state: &'a TextInputState,
    theme: &'a Theme,
    placeholder: &'a str,
    focused: bool,
    /// Left padding columns inside the field (a leading gutter space).
    pad: u16,
}

impl<'a> TextInput<'a> {
    pub fn new(state: &'a TextInputState, theme: &'a Theme) -> Self {
        TextInput {
            state,
            theme,
            placeholder: "",
            focused: false,
            pad: 0,
        }
    }

    pub fn placeholder(mut self, p: &'a str) -> Self {
        self.placeholder = p;
        self
    }

    pub fn focused(mut self, f: bool) -> Self {
        self.focused = f;
        self
    }

    pub fn pad(mut self, p: u16) -> Self {
        self.pad = p;
        self
    }

    /// Builds the wrapped display rows and the index of the caret's row.
    fn build(&self, width: u16) -> (Vec<Line<'static>>, usize) {
        let th = self.theme;
        let bg = th.input_bg();
        let bg_style = Style::new().bg(bg);
        let pad = self.pad as usize;
        let width = width as usize;
        let inner = width.saturating_sub(pad).max(1);

        // Empty: a dim placeholder, with the caret in front when focused.
        if self.state.is_empty() {
            let mut spans: Vec<Span> = Vec::new();
            if pad > 0 {
                spans.push(Span::styled(" ".repeat(pad), bg_style));
            }
            if self.focused {
                spans.push(caret_block(th, bg));
            }
            spans.push(Span::styled(
                self.placeholder.to_string(),
                Style::new().fg(th.fg_dim).bg(bg),
            ));
            return (vec![Line::from(spans)], 0);
        }

        let content = self.state.content();
        let (cl, cc) = line_col(content, self.state.cursor());
        let logical: Vec<Vec<char>> = content.split('\n').map(|l| l.chars().collect()).collect();

        let mut rows: Vec<Line<'static>> = Vec::new();
        let mut caret_row = 0;
        for (li, chars) in logical.iter().enumerate() {
            let len = chars.len();
            let nrows = if len == 0 { 1 } else { len.div_ceil(inner) };
            for r in 0..nrows {
                let s = r * inner;
                let e = ((r + 1) * inner).min(len);
                let full = e - s == inner;
                let caret = if self.focused && li == cl {
                    if cc >= s && cc < e {
                        Some(Caret::Over(cc - s))
                    } else if cc == e && e == len && !full {
                        Some(Caret::End)
                    } else {
                        None
                    }
                } else {
                    None
                };
                if caret.is_some() {
                    caret_row = rows.len();
                }
                rows.push(self.row_line(&chars[s..e], caret, width, bg));
            }
            // Caret past a line whose length is an exact multiple of the wrap
            // width lands at column 0 of a fresh row.
            if self.focused && li == cl && cc == len && len > 0 && len % inner == 0 {
                caret_row = rows.len();
                rows.push(self.row_line(&[], Some(Caret::End), width, bg));
            }
        }
        (rows, caret_row)
    }

    /// Renders one display row's chars (+ optional caret) as a full-width line.
    fn row_line(
        &self,
        chars: &[char],
        caret: Option<Caret>,
        width: usize,
        bg: ratatui::style::Color,
    ) -> Line<'static> {
        let th = self.theme;
        let bg_style = Style::new().bg(bg);
        let pad = self.pad as usize;
        let mut spans: Vec<Span> = Vec::new();
        if pad > 0 {
            spans.push(Span::styled(" ".repeat(pad), bg_style));
        }
        let over = match caret {
            Some(Caret::Over(c)) => Some(c),
            _ => None,
        };
        for (i, ch) in chars.iter().enumerate() {
            let style = if over == Some(i) {
                Style::new()
                    .fg(bg)
                    .bg(th.accent)
                    .add_modifier(Modifier::SLOW_BLINK)
            } else {
                Style::new().fg(th.fg).bg(bg)
            };
            spans.push(Span::styled(ch.to_string(), style));
        }
        let mut used = pad + chars.len();
        if matches!(caret, Some(Caret::End)) {
            spans.push(caret_block(th, bg));
            used += 1;
        }
        // Fill the rest of the row so the sunken background spans the full width
        // (matters when the widget is drawn inside another paragraph's area).
        if width > used {
            spans.push(Span::styled(" ".repeat(width - used), bg_style));
        }
        Line::from(spans)
    }
}

impl Widget for TextInput<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        let bg = self.theme.input_bg();
        buf.set_style(area, Style::new().bg(bg));

        let (rows, caret_row) = self.build(area.width);
        let h = area.height as usize;
        // Scroll vertically so the caret's row stays visible.
        let scroll = if caret_row >= h { caret_row + 1 - h } else { 0 };
        let visible: Vec<Line> = rows.into_iter().skip(scroll).take(h).collect();
        Paragraph::new(visible)
            .style(Style::new().bg(bg))
            .render(area, buf);
    }
}

/// A blinking block caret on the input background.
fn caret_block(th: &Theme, bg: ratatui::style::Color) -> Span<'static> {
    Span::styled(
        "█",
        Style::new()
            .fg(th.accent)
            .bg(bg)
            .add_modifier(Modifier::SLOW_BLINK),
    )
}

/// Maps a char-index caret into (logical line, column).
fn line_col(content: &str, cursor: usize) -> (usize, usize) {
    let mut rem = cursor;
    for (i, line) in content.split('\n').enumerate() {
        let ll = line.chars().count();
        if rem <= ll {
            return (i, rem);
        }
        rem -= ll + 1; // line + '\n'
    }
    (0, 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Renders `state` into a `w`x`h` buffer and returns each row as a string.
    fn render(state: &TextInputState, focused: bool, pad: u16, w: u16, h: u16) -> Vec<String> {
        let theme = Theme::default();
        let area = Rect::new(0, 0, w, h);
        let mut buf = Buffer::empty(area);
        TextInput::new(state, &theme)
            .placeholder("Find...")
            .focused(focused)
            .pad(pad)
            .render(area, &mut buf);
        (0..h)
            .map(|y| {
                (0..w)
                    .map(|x| buf[(x, y)].symbol().to_string())
                    .collect::<String>()
            })
            .collect()
    }

    #[test]
    fn empty_shows_placeholder() {
        let s = TextInputState::default();
        let rows = render(&s, false, 0, 10, 1);
        assert_eq!(rows[0], "Find...   ");
    }

    #[test]
    fn short_text_renders_with_end_caret() {
        let mut s = TextInputState::default();
        s.set_content("hi");
        let rows = render(&s, true, 0, 10, 1);
        // "hi" then a block caret.
        assert!(rows[0].starts_with("hi█"), "got {:?}", rows[0]);
    }

    #[test]
    fn long_single_line_scrolls_to_caret() {
        let mut s = TextInputState::default();
        s.set_content("abcdefghijklmnop"); // 16 chars, caret at end
        // Width 5, height 1: only the caret's wrapped row is visible; it must
        // contain the tail, not the head.
        let rows = render(&s, true, 0, 5, 1);
        assert!(rows[0].contains('p'), "tail not visible: {:?}", rows[0]);
        assert!(!rows[0].contains('a'), "head should have scrolled off: {:?}", rows[0]);
    }

    #[test]
    fn multiline_shows_all_rows() {
        let mut s = TextInputState::default();
        s.set_content("one\ntwo\nthree");
        let rows = render(&s, false, 1, 10, 3);
        assert_eq!(rows[0].trim(), "one");
        assert_eq!(rows[1].trim(), "two");
        assert_eq!(rows[2].trim(), "three");
    }

    #[test]
    fn multiline_scrolls_when_caret_below_fold() {
        let mut s = TextInputState::default();
        s.set_content("l1\nl2\nl3\nl4"); // caret at end (on l4)
        // Height 2: must scroll so l4 (caret row) is visible.
        let rows = render(&s, true, 0, 8, 2);
        assert!(rows.iter().any(|r| r.contains("l4")), "caret row hidden: {:?}", rows);
        assert!(!rows.iter().any(|r| r.contains("l1")), "should have scrolled past l1: {:?}", rows);
    }
}
