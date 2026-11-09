//! The log panel.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::tui::app::{App, Focus, LeftTab};
use crate::tui::log::message_color;

use super::super::layout::captures_areas;
use super::*;

/// The lower left panel: the log or the variables, switched by the tabs in its title.
pub(crate) fn draw_log(frame: &mut Frame, app: &App, area: Rect) {
    let variables = app.left_tab == LeftTab::Variables;
    let active = Style::new()
        .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
        .fg(Color::Yellow);
    let (logs_style, vars_style) = if variables {
        (Style::new(), active)
    } else {
        (active, Style::new())
    };
    let hint = if variables { "" } else { " (Ctrl+E: open)" };
    let title = Line::from(vec![
        Span::raw(" "),
        Span::styled("Logs", logs_style),
        Span::raw("|"),
        Span::styled("Variables", vars_style),
        Span::raw(hint),
        Span::raw(" "),
    ]);
    let outer = block_titled(title, app.focus == Focus::Captures);
    let inner = outer.inner(area);
    frame.render_widget(outer, area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }
    if variables {
        return draw_variables(frame, app, inner);
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

/// The captures to define, the values captured so far and the keys that edit them.
fn draw_variables(frame: &mut Frame, app: &App, inner: Rect) {
    let (rows, values, hint) = captures_areas(inner, app.captures.rows.len());
    draw_capture_rows(frame, app, rows);
    let dim = Style::new().fg(Color::DarkGray);
    if values.height > 0 {
        let lines: Vec<Line> = if app.vars.is_empty() {
            vec![Line::styled("nothing captured yet", dim)]
        } else {
            app.vars
                .iter()
                .map(|(name, value)| {
                    Line::from(vec![
                        Span::styled(format!("${name}"), Style::new().fg(Color::Cyan)),
                        Span::styled(" = ", dim),
                        Span::raw(value.replace('\n', " ")),
                    ])
                })
                .collect()
        };
        frame.render_widget(Paragraph::new(lines), values);
    }
    frame.render_widget(
        Paragraph::new(Line::styled("Ctrl+D: delete row and variable", dim)),
        hint,
    );
}
