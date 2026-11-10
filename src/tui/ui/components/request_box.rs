//! The Body / Headers box and the header rows.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph, Wrap};
use unicode_width::UnicodeWidthStr;

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
    let lower_focused = matches!(
        app.focus,
        Focus::Body | Focus::Variables | Focus::Headers | Focus::Params | Focus::Auth
    );
    let labels = request_tab_labels(
        app.headers_count(request, previewing),
        app.params_count(request, previewing),
        app.auth_label(request, previewing),
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
        body_block = body_block
            .title_top(
                Line::styled(
                    body_kind_title(request.body_kind),
                    Style::new().fg(Color::Cyan),
                )
                .right_aligned(),
            )
            .title_bottom(
                Line::styled(
                    vars_title(app.use_vars),
                    Style::new().fg(if app.use_vars {
                        Color::Green
                    } else {
                        Color::DarkGray
                    }),
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
        (RequestTab::Params, true) => {
            let lines: Vec<Line> = request_params(&request.url)
                .into_iter()
                .map(|(k, v)| Line::raw(format!("{k}={v}")))
                .collect();
            frame.render_widget(Paragraph::new(lines), body_inner);
        }
        (RequestTab::Auth, true) => {
            let value = request
                .headers
                .iter()
                .find(|h| h.name.eq_ignore_ascii_case("authorization"))
                .map_or_else(
                    || "no authorization".to_owned(),
                    |h| format!("Authorization: {}", h.value),
                );
            frame.render_widget(Paragraph::new(value), body_inner);
        }
        (RequestTab::Auth, false) => draw_auth(frame, app, body_inner),
        (RequestTab::Params, false) => draw_pair_rows(
            frame,
            &app.params,
            app.focus == Focus::Params,
            body_inner,
            &Pairs {
                validate_names: false,
                files: false,
                value: "Value",
                add: "+ add param (Enter on the last value, Ctrl+D removes a row)",
                from_start: true,
            },
        ),
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
                from_start: false,
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
                from_start: false,
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
pub(crate) struct Pairs {
    /// Whether a name that cannot be an HTTP header is shown as invalid.
    validate_names: bool,
    /// Whether a value starting with `@` is a file.
    files: bool,
    value: &'static str,
    add: &'static str,
    /// Whether an input that does not have the cursor shows the start of a long text
    /// (with `…`) instead of the end, so nothing reads as missing.
    from_start: bool,
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
            from_start: false,
        },
    );
}

/// The Variables panel's capture inputs: variable name `:` path into the response.
pub(crate) fn draw_capture_rows(frame: &mut Frame, app: &App, area: Rect) {
    draw_pair_rows(
        frame,
        &app.captures,
        app.focus == Focus::Captures,
        area,
        &Pairs {
            validate_names: false,
            files: false,
            value: ".path.in.response",
            add: "+ add capture (Enter on the last path)",
            from_start: false,
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
            let text = ta.lines()[0].clone();
            if look.from_start && !here && text.width() > rect.width as usize {
                let keep = (rect.width as usize).saturating_sub(1);
                let mut shown = String::new();
                for c in text.chars() {
                    if shown.width() + c.to_string().width() > keep {
                        break;
                    }
                    shown.push(c);
                }
                shown.push('…');
                frame.render_widget(Paragraph::new(shown), rect);
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

/// The `key=value` pairs of the query string in `url`, still encoded.
fn request_params(url: &str) -> Vec<(String, String)> {
    let query = url.split('#').next().unwrap_or("");
    let Some((_, query)) = query.split_once('?') else {
        return Vec::new();
    };
    query
        .split('&')
        .filter(|p| !p.is_empty())
        .map(|p| match p.split_once('=') {
            Some((k, v)) => (k.to_owned(), v.to_owned()),
            None => (p.to_owned(), String::new()),
        })
        .collect()
}

/// The request as it goes over the wire: request line, headers, blank line, body. Other
/// kinds show the HTTP request they are carried by.
pub(crate) fn draw_http_preview(
    frame: &mut Frame,
    request: &crate::tui::app::RequestView,
    area: Rect,
) {
    let block = block("HTTP", false);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let dim = Style::new().fg(Color::DarkGray);
    let (host, mut target) = split_url(&request.url);
    let mut version = "HTTP/1.1";
    let mut method = request.method.as_str();
    let mut defaults: Vec<(&str, String)> = Vec::new();
    let mut body = request.body.clone();
    let user_agent = format!(
        "hozd/{} ({} {})",
        &VERSION[1..],
        std::env::consts::OS,
        std::env::consts::ARCH
    );
    match request.kind {
        Kind::Http => {
            if !body.is_empty() {
                defaults.push(("Content-Type", request.body_kind.content_type().to_owned()));
            }
        }
        Kind::Graphql => {
            method = "POST";
            let mut payload = serde_json::Map::new();
            payload.insert("query".into(), request.body.clone().into());
            if let Ok(v @ serde_json::Value::Object(_)) =
                serde_json::from_str(request.variables.trim())
            {
                payload.insert("variables".into(), v);
            }
            body = serde_json::Value::Object(payload).to_string();
            defaults.push(("Content-Type", "application/json".to_owned()));
        }
        Kind::Ws => {
            method = "GET";
            body.clear();
            defaults.extend([
                ("Upgrade", "websocket".to_owned()),
                ("Connection", "Upgrade".to_owned()),
                ("Sec-WebSocket-Version", "13".to_owned()),
                ("Sec-WebSocket-Key", "<generated>".to_owned()),
            ]);
        }
        Kind::Grpc => {
            method = "POST";
            target = format!("/{}", request.grpc.method.trim_start_matches('/'));
            version = "HTTP/2";
            defaults.extend([
                ("Content-Type", "application/grpc".to_owned()),
                ("TE", "trailers".to_owned()),
            ]);
        }
    }
    // gRPC addresses are `host:port` without a scheme; the host is the authority either way
    let has = |name: &str| {
        request
            .headers
            .iter()
            .any(|h| h.name.eq_ignore_ascii_case(name))
    };
    let mut lines = vec![Line::from(vec![
        Span::styled(
            method.to_owned(),
            Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD),
        ),
        Span::raw(format!(" {target} ")),
        Span::styled(version, dim),
    ])];
    let mut header = |name: &str, value: String| {
        lines.push(Line::from(vec![
            Span::styled(format!("{name}: "), Style::new().fg(Color::Cyan)),
            Span::raw(value),
        ]));
    };
    if !has("Host") {
        header("Host", host);
    }
    for h in &request.headers {
        header(&h.name, h.value.clone());
    }
    if !has("User-Agent") {
        header("User-Agent", user_agent);
    }
    for (name, value) in defaults {
        if !has(name) {
            header(name, value);
        }
    }
    if !body.is_empty() {
        header("Content-Length", body.len().to_string());
    }
    if request.kind != Kind::Ws && !has("Connection") {
        header("Connection", "close".to_owned());
    }
    if !body.is_empty() {
        lines.push(Line::raw(""));
        lines.extend(body.lines().map(|l| Line::raw(l.to_owned())));
    }
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

/// `(host, request target)` of `url`; the scheme and fragment are not sent.
fn split_url(url: &str) -> (String, String) {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let rest = rest.split('#').next().unwrap_or("");
    let at = rest.find(['/', '?']).unwrap_or(rest.len());
    let (host, target) = rest.split_at(at);
    let target = if target.starts_with('/') {
        target.to_owned()
    } else {
        format!("/{target}")
    };
    (host.to_owned(), target)
}
