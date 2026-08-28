//! General-purpose modal dialog: ask (confirm), info (message), input (text entry).
//! While open it captures all keyboard/mouse input; `view` draws it on top.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph, Wrap};

use crate::app::model::{Dialog, DialogKind, Model};

/// The dialog layout computed once; shared by render and mouse hit-testing.
pub struct DialogLayout {
    pub area: Rect,
    pub message: Rect,
    pub input: Option<Rect>,
    pub confirm: Rect,
    pub cancel: Option<Rect>,
}

/// Centered modal rectangle.
fn dialog_area(d: &Dialog, term: Rect) -> Rect {
    let w = 54u16.min(term.width.saturating_sub(4)).max(24);
    let h: u16 = match d.kind {
        DialogKind::Input => 9,
        _ => 8,
    }
    .min(term.height.saturating_sub(2));
    let x = term.x + term.width.saturating_sub(w) / 2;
    let y = term.y + term.height.saturating_sub(h) / 2;
    Rect { x, y, width: w, height: h }
}

/// (confirm label, cancel label) — None if there is no cancel.
fn button_labels(kind: DialogKind) -> (&'static str, Option<&'static str>) {
    match kind {
        DialogKind::Ask => ("Yes", Some("No")),
        DialogKind::Info => ("OK", None),
        DialogKind::Input => ("OK", Some("Cancel")),
    }
}

fn btn_w(label: &str) -> u16 {
    label.chars().count() as u16 + 2
}

pub fn layout(d: &Dialog, term: Rect) -> DialogLayout {
    let area = dialog_area(d, term);
    let tx = area.x + 2;
    let tw = area.width.saturating_sub(4);
    let buttons_y = area.y + area.height.saturating_sub(2);

    let (input, msg_bottom) = match d.kind {
        DialogKind::Input => {
            let iy = buttons_y.saturating_sub(2);
            (
                Some(Rect {
                    x: tx,
                    y: iy,
                    width: tw,
                    height: 1,
                }),
                iy,
            )
        }
        _ => (None, buttons_y.saturating_sub(1)),
    };
    let msg_y = area.y + 1;
    let message = Rect {
        x: tx,
        y: msg_y,
        width: tw,
        height: msg_bottom.saturating_sub(msg_y),
    };

    let (c_label, x_label) = button_labels(d.kind);
    let cw = btn_w(c_label);
    let right = area.x + area.width.saturating_sub(2);
    let (confirm, cancel) = if let Some(xl) = x_label {
        let xw = btn_w(xl);
        let cancel_rect = Rect {
            x: right.saturating_sub(xw),
            y: buttons_y,
            width: xw,
            height: 1,
        };
        let confirm_rect = Rect {
            x: cancel_rect.x.saturating_sub(1 + cw),
            y: buttons_y,
            width: cw,
            height: 1,
        };
        (confirm_rect, Some(cancel_rect))
    } else {
        (
            Rect {
                x: right.saturating_sub(cw),
                y: buttons_y,
                width: cw,
                height: 1,
            },
            None,
        )
    };

    DialogLayout {
        area,
        message,
        input,
        confirm,
        cancel,
    }
}

pub fn render(frame: &mut Frame, model: &Model) {
    let Some(d) = model.dialog.as_ref() else {
        return;
    };
    let term = frame.area();
    let l = layout(d, term);
    let th = &model.theme;

    frame.render_widget(Clear, l.area);
    let block = Block::bordered()
        .title(format!(" {} ", d.title))
        .border_style(Style::new().fg(th.accent))
        .style(Style::new().bg(th.bg_alt).fg(th.fg));
    frame.render_widget(block, l.area);

    let msg = Paragraph::new(d.message.clone())
        .style(Style::new().fg(th.fg).bg(th.bg_alt))
        .wrap(Wrap { trim: true });
    frame.render_widget(msg, l.message);

    if let Some(irect) = l.input {
        frame.render_widget(
            crate::ui::text_input::TextInput::new(&d.input, th)
                .focused(true)
                .pad(1),
            irect,
        );
    }

    let (c_label, x_label) = button_labels(d.kind);
    render_button(frame, l.confirm, c_label, d.selected == 0, th);
    if let (Some(rect), Some(label)) = (l.cancel, x_label) {
        render_button(frame, rect, label, d.selected == 1, th);
    }
}

fn render_button(
    frame: &mut Frame,
    rect: Rect,
    label: &str,
    selected: bool,
    th: &crate::core::theme::Theme,
) {
    let (bg, fg) = if selected {
        (th.accent, th.statusbar_fg)
    } else {
        (th.tab_inactive_bg, th.fg)
    };
    let p = Paragraph::new(Line::from(Span::styled(
        format!(" {label} "),
        Style::new().fg(fg).bg(bg).add_modifier(Modifier::BOLD),
    )));
    frame.render_widget(p, rect);
}

fn rect_contains(r: Rect, x: u16, y: u16) -> bool {
    x >= r.x && x < r.x + r.width && y >= r.y && y < r.y + r.height
}

/// Converts a mouse click into a button target: Some(true)=confirm, Some(false)=cancel.
pub fn hit(d: &Dialog, term: Rect, x: u16, y: u16) -> Option<bool> {
    let l = layout(d, term);
    if rect_contains(l.confirm, x, y) {
        return Some(true);
    }
    if let Some(rect) = l.cancel
        && rect_contains(rect, x, y)
    {
        return Some(false);
    }
    None
}
