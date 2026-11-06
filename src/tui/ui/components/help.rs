//! The keybindings menu.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, List, ListItem, ListState};

use crate::tui::app::App;

use super::super::layout::*;
use super::*;

/// The keybindings menu: the center panel with a one-cell margin, like the picker.
pub fn help_area(area: Rect) -> Rect {
    picker_area(area)
}

pub(crate) fn draw_help(frame: &mut Frame, app: &App, screen: Rect) {
    let Some(selected) = app.help else {
        return;
    };
    let area = help_area(screen);
    frame.render_widget(Clear, area);
    let title = Line::from(vec![
        Span::styled(" Keybindings ", Style::new().add_modifier(Modifier::BOLD)),
        Span::styled(
            "| Up/Down | Enter: run | Esc ",
            Style::new().fg(Color::DarkGray),
        ),
    ]);
    let outer = block_titled(title, true);
    let list_area = outer.inner(area);
    frame.render_widget(outer, area);
    if list_area.height == 0 {
        return;
    }
    let commands = app.commands();
    let key_width = commands.iter().map(|c| c.key.len()).max().unwrap_or(0);
    let rows: Vec<ListItem> = commands
        .iter()
        .map(|c| {
            ListItem::new(Line::from(vec![
                Span::styled(
                    format!("{:<key_width$}", c.key),
                    Style::new().fg(Color::Green),
                ),
                Span::raw("  "),
                Span::raw(c.description),
            ]))
        })
        .collect();
    let mut state = ListState::default()
        .with_selected(Some(selected))
        .with_offset(list_offset(Some(selected), list_area.height));
    frame.render_stateful_widget(
        List::new(rows).highlight_style(
            Style::new()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        ),
        list_area,
        &mut state,
    );
}
