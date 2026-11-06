//! The log panel.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::tui::app::App;
use crate::tui::log::message_color;

use super::*;

/// The log, newest entry last; shows the tail that fits.
pub(crate) fn draw_log(frame: &mut Frame, app: &App, area: Rect) {
    let outer = block("Logs (Ctrl+E: open)", false);
    let inner = outer.inner(area);
    frame.render_widget(outer, area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }
    if app.log.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::styled(
                "no events yet",
                Style::new().fg(Color::DarkGray),
            )),
            inner,
        );
        return;
    }
    let mut lines: Vec<Line> = app
        .log
        .entries()
        .rev()
        .take(inner.height as usize)
        .map(|e| {
            let color = message_color(e.level, &e.message);
            Line::from(vec![
                Span::styled(format!("{} ", e.time), Style::new().fg(Color::DarkGray)),
                Span::styled(e.message.clone(), Style::new().fg(color)),
            ])
        })
        .collect();
    lines.reverse();
    frame.render_widget(Paragraph::new(lines), inner);
}
