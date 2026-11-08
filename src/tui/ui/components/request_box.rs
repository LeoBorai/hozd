//! The Body / Headers box and the header rows.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};

use crate::client::{BodyKind, Kind};

use crate::tui::app::{App, Focus, RequestTab};
use crate::tui::headers::{self, Col, HeaderEditor};
use crate::tui::json_editor::{JsonCol, JsonKind};
use crate::tui::syntax_highlight::graphql;
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
    let mut body_block = block_titled(Line::from(title_spans), lower_focused);
    if request.kind == Kind::Http && app.request_tab == RequestTab::Body {
        body_block = body_block.title_top(
            Line::styled(
                body_kind_title(request.body_kind),
                Style::new().fg(Color::Cyan),
            )
            .right_aligned(),
        );
    }
    let body_inner = body_block.inner(area);
    frame.render_widget(body_block, area);
    let graphql = request.kind == Kind::Graphql;
    match (app.request_tab, previewing) {
        (RequestTab::Body, true) if graphql => {
            let lines = graphql::highlight_lines(request.body.lines());
            frame.render_widget(Paragraph::new(lines), body_inner);
        }
        (RequestTab::Body, true) => {
            let json = request.kind != Kind::Http
                || matches!(request.body_kind, BodyKind::Json | BodyKind::JsonRaw);
            let lines: Vec<Line> = request
                .body
                .lines()
                .map(|l| {
                    if json {
                        highlight_line(l)
                    } else {
                        Line::raw(l)
                    }
                })
                .collect();
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
        (RequestTab::Body, false) if request.kind == Kind::Http => {
            draw_http_body(frame, app, body_inner)
        }
        (RequestTab::Body, false) => render_editor(
            frame,
            &app.body,
            body_inner,
            app.focus == Focus::Body,
            if graphql {
                Syntax::Graphql
            } else {
                Syntax::Json
            },
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

/// The REST body editor for the selected body kind.
fn draw_http_body(frame: &mut Frame, app: &App, area: Rect) {
    let focused = app.focus == Focus::Body;
    match app.body_kind {
        BodyKind::Text => render_editor(frame, &app.body, area, focused, Syntax::Plain),
        BodyKind::JsonRaw => render_editor(frame, &app.body, area, focused, Syntax::Json),
        BodyKind::Json => draw_json_rows(frame, app, area),
        BodyKind::Form => draw_pair_rows(
            frame,
            &app.form,
            focused,
            area,
            &Pairs {
                validate_names: false,
                files: false,
                value: "Value",
                add: "+ add field (Enter on the last value, Ctrl+D removes a row)",
            },
        ),
        BodyKind::Multipart => draw_pair_rows(
            frame,
            &app.multipart,
            focused,
            area,
            &Pairs {
                validate_names: false,
                files: true,
                value: "Value or @file",
                add: "+ add part (@ picks a file, Ctrl+D removes a row)",
            },
        ),
        BodyKind::File => {
            if app.file_text().is_empty() && !focused {
                frame.render_widget(
                    Paragraph::new(Line::styled(
                        "@ picks the file to send as application/octet-stream",
                        Style::new().fg(Color::DarkGray),
                    )),
                    area,
                );
            } else {
                render_editor(frame, &app.file, area, focused, Syntax::File);
            }
        }
    }
}

/// The JSON fields: indented key `:` value, with the type at the right edge.
fn draw_json_rows(frame: &mut Frame, app: &App, area: Rect) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let editor = &app.json;
    let focused = app.focus == Focus::Body;
    let depths: Vec<usize> = editor.rows.iter().map(|r| r.depth).collect();
    let grid = json_grid(area, &depths, editor.row);
    let dim = Style::new().fg(Color::DarkGray);
    for cells in &grid.rows {
        let row = &editor.rows[cells.row];
        let on_row = focused && cells.row == editor.row;
        let (key_here, value_here) = (
            on_row && editor.col == JsonCol::Key,
            on_row && editor.col == JsonCol::Value,
        );
        if editor.in_array(cells.row) {
            let index = editor.array_index(cells.row);
            frame.render_widget(
                Paragraph::new(Line::styled(format!("{index}"), dim)),
                cells.key,
            );
        } else {
            if key_here {
                frame.render_widget(
                    Block::default().style(Style::new().bg(Color::DarkGray)),
                    cells.key,
                );
            }
            if row.key_text().is_empty() && !key_here {
                frame.render_widget(Paragraph::new(Line::styled("key", dim)), cells.key);
            } else {
                render_editor(frame, &row.key, cells.key, key_here, Syntax::HeaderName);
            }
        }
        let sep = Rect::new(cells.key.right(), cells.key.y, 1, 1);
        frame.render_widget(Paragraph::new(Span::styled(":", dim)), sep);
        if value_here {
            frame.render_widget(
                Block::default().style(Style::new().bg(Color::DarkGray)),
                cells.value,
            );
        }
        match row.kind {
            JsonKind::String | JsonKind::Number => {
                if row.value_text().is_empty() && !value_here {
                    frame.render_widget(Paragraph::new(Line::styled("value", dim)), cells.value);
                } else {
                    let syntax = if row.is_invalid_number() {
                        Syntax::Invalid
                    } else {
                        Syntax::Plain
                    };
                    render_editor(frame, &row.value, cells.value, value_here, syntax);
                }
            }
            JsonKind::Bool => frame.render_widget(
                Paragraph::new(Line::styled(
                    row.value_text(),
                    Style::new().fg(Color::Magenta),
                )),
                cells.value,
            ),
            JsonKind::Null => frame.render_widget(
                Paragraph::new(Line::styled("null", Style::new().fg(Color::Magenta))),
                cells.value,
            ),
            JsonKind::Object | JsonKind::Array => {
                let (open, close) = if row.kind == JsonKind::Object {
                    ('{', '}')
                } else {
                    ('[', ']')
                };
                frame.render_widget(
                    Paragraph::new(Line::styled(
                        format!("{open} {} {close}", editor.children(cells.row)),
                        dim,
                    )),
                    cells.value,
                );
            }
        }
        frame.render_widget(
            Paragraph::new(Line::styled(row.kind.label(), dim)),
            cells.tag,
        );
    }
    if let Some(rect) = grid.hint {
        let root = if editor.root_array { "[]" } else { "{}" };
        frame.render_widget(
            Paragraph::new(Line::styled(
                format!(
                    "Enter next/add | Ctrl+R type | Alt+Left/Right nest | Ctrl+D remove | Alt+T root {root}"
                ),
                dim,
            )),
            rect,
        );
    }
}

/// How a list of name/value rows looks.
struct Pairs {
    /// Whether a name that cannot be an HTTP header is shown as invalid.
    validate_names: bool,
    /// Whether a value starting with `@` is a file.
    files: bool,
    value: &'static str,
    add: &'static str,
}

/// The header inputs: name `:` value, one row each, then the "add header" line.
pub(crate) fn draw_header_rows(frame: &mut Frame, app: &App, area: Rect) {
    draw_pair_rows(
        frame,
        &app.headers,
        app.focus == Focus::Headers,
        area,
        &Pairs {
            validate_names: true,
            files: false,
            value: "Value",
            add: "+ add header (Enter on the last value, Ctrl+D removes a row)",
        },
    );
}

fn draw_pair_rows(
    frame: &mut Frame,
    editor: &HeaderEditor,
    focused: bool,
    area: Rect,
    look: &Pairs,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }
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
            (cells.value, Col::Value, &row.value, look.value),
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
                if look.validate_names && headers::name_is_invalid(row) {
                    Syntax::Invalid
                } else {
                    Syntax::HeaderName
                }
            } else if look.files && row.value_text().starts_with('@') {
                Syntax::File
            } else {
                Syntax::Plain
            };
            render_editor(frame, ta, rect, here, syntax);
        }
    }
    if let Some(rect) = grid.add {
        frame.render_widget(
            Paragraph::new(Line::styled(look.add, Style::new().fg(Color::DarkGray))),
            rect,
        );
    }
}
