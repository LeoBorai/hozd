//! Bordered boxes with titles, the shell of every panel.

use ratatui::style::{Color, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, BorderType, Borders};

use crate::tui::app::{App, Focus};

/// `[n] title`: `n` is the key that focuses `panel`.
pub(crate) fn numbered(app: &App, panel: Focus, title: &str) -> String {
    match app.panels().iter().position(|p| *p == panel) {
        Some(i) => format!("[{}] {title}", i + 1),
        None => title.to_owned(),
    }
}

pub(crate) fn block(title: &str, focused: bool) -> Block<'static> {
    block_titled(Line::raw(format!(" {title} ")), focused)
}

pub(crate) fn block_titled(title: Line<'static>, focused: bool) -> Block<'static> {
    let color = if focused { Color::Green } else { Color::Gray };
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(color))
        .title(title)
}
