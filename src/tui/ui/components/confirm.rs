//! The small centered Ok / Cancel question.

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph, Wrap};

use crate::tui::app::Confirm;

use super::*;

pub(crate) fn draw_confirm(frame: &mut Frame, confirm: &Confirm, screen: Rect) {
    let width = 48.min(screen.width);
    let height = 6.min(screen.height);
    let [_, row, _] = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(height),
        Constraint::Fill(1),
    ])
    .areas(screen);
    let [_, area, _] = Layout::horizontal([
        Constraint::Fill(1),
        Constraint::Length(width),
        Constraint::Fill(1),
    ])
    .areas(row);

    frame.render_widget(Clear, area);
    let outer = block("Delete", true);
    let inner = outer.inner(area);
    frame.render_widget(outer, area);

    let [message, _, buttons] = Layout::vertical([
        Constraint::Min(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(inner);
    frame.render_widget(
        Paragraph::new(confirm.message.as_str())
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: true }),
        message,
    );

    let option = |label: &'static str, active: bool| {
        let style = if active {
            Style::new()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
        } else {
            Style::new().fg(Color::Gray)
        };
        Span::styled(label, style)
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            option("Ok", confirm.ok),
            Span::raw("    "),
            option("Cancel", !confirm.ok),
        ]))
        .alignment(Alignment::Center),
        buttons,
    );
}
