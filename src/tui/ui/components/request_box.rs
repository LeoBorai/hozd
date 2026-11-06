//! The Body / Headers box and the header rows.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};

use crate::client::Kind;

use crate::tui::app::{App, Focus, RequestTab};
use crate::tui::headers::{self, Col};
use crate::tui::syntax_highlight::json::highlight_line;

use super::super::layout::*;
use super::*;

/// The Body / Headers (Metadata) box.
pub(crate) fn draw_request_box(
    frame: &mut Frame,
    app: &App,
    request: &crate::tui::app::RequestView,
    previewing: bool,
    area: Rect,
) {
    let lower_focused = matches!(app.focus, Focus::Body | Focus::Variables | Focus::Headers);
    let labels = request_tab_labels(
        request.headers.len(),
        request.kind,
        app.panels()
            .iter()
            .position(|p| *p == app.editor_focus())
            .map_or(0, |i| i + 1),
    );
    let active = Style::new()
        .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
        .fg(Color::Yellow);
    let mut title_spans = Vec::new();
    for (tab, label) in labels {
        if !title_spans.is_empty() {
            title_spans.push(Span::raw("|"));
        }
        let style = if tab == app.request_tab {
            active
        } else {
            Style::new()
        };
        title_spans.push(Span::styled(label, style));
    }
    let body_block = block_titled(Line::from(title_spans), lower_focused);
    let body_inner = body_block.inner(area);
    frame.render_widget(body_block, area);
    let graphql = request.kind == Kind::Graphql;
    match (app.request_tab, previewing) {
        (RequestTab::Body, true) if graphql => {
            let lines: Vec<Line> = request.body.lines().map(Line::raw).collect();
            frame.render_widget(Paragraph::new(lines), body_inner);
        }
        (RequestTab::Body, true) => {
            let lines: Vec<Line> = request.body.lines().map(highlight_line).collect();
            frame.render_widget(Paragraph::new(lines), body_inner);
        }
        (RequestTab::Variables, true) => {
            let lines: Vec<Line> = request.variables.lines().map(highlight_line).collect();
            frame.render_widget(Paragraph::new(lines), body_inner);
        }
        (RequestTab::Headers, true) => {
            let lines: Vec<Line> = headers::to_lines(&request.headers)
                .iter()
                .map(|l| headers::highlight_line(l))
                .collect();
            frame.render_widget(Paragraph::new(lines), body_inner);
        }
        (RequestTab::Body, false) => render_editor(
            frame,
            &app.body,
            body_inner,
            app.focus == Focus::Body,
            if graphql { Syntax::Plain } else { Syntax::Json },
        ),
        (RequestTab::Variables, false) => render_editor(
            frame,
            &app.variables,
            body_inner,
            app.focus == Focus::Variables,
            Syntax::Json,
        ),
        (RequestTab::Headers, false) => draw_header_rows(frame, app, body_inner),
    }
}

/// The header inputs: name `:` value, one row each, then the "add header" line.
pub(crate) fn draw_header_rows(frame: &mut Frame, app: &App, area: Rect) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let editor = &app.headers;
    let focused = app.focus == Focus::Headers;
    let grid = header_grid(area, editor.rows.len(), editor.row);
    for cells in &grid.rows {
        let row = &editor.rows[cells.row];
        let sep = Rect::new(cells.name.right(), cells.name.y, 1, 1);
        frame.render_widget(
            Paragraph::new(Span::styled(":", Style::new().fg(Color::DarkGray))),
            sep,
        );
        for (rect, col, ta, placeholder) in [
            (cells.name, Col::Name, &row.name, "Name"),
            (cells.value, Col::Value, &row.value, "Value"),
        ] {
            let here = focused && cells.row == editor.row && col == editor.col;
            if here {
                frame.render_widget(
                    Block::default().style(Style::new().bg(Color::DarkGray)),
                    rect,
                );
            }
            if ta.lines()[0].is_empty() && !here {
                frame.render_widget(
                    Paragraph::new(Line::styled(placeholder, Style::new().fg(Color::DarkGray))),
                    rect,
                );
                continue;
            }
            let syntax = if col == Col::Name {
                if headers::name_is_invalid(row) {
                    Syntax::Invalid
                } else {
                    Syntax::HeaderName
                }
            } else {
                Syntax::Plain
            };
            render_editor(frame, ta, rect, here, syntax);
        }
    }
    if let Some(rect) = grid.add {
        frame.render_widget(
            Paragraph::new(Line::styled(
                "+ add header (Enter on the last value, Ctrl+D removes a row)",
                Style::new().fg(Color::DarkGray),
            )),
            rect,
        );
    }
}
