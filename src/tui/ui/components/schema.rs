//! The schema box and the selected method / operation boxes.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::client::SchemaSource;

use crate::tui::app::{App, Focus, SchemaMode};

use super::*;

/// gRPC: where the schema comes from, and what was loaded from it.
pub(crate) fn draw_schema(
    frame: &mut Frame,
    app: &App,
    request: &crate::tui::app::RequestView,
    previewing: bool,
    area: Rect,
) {
    let status = if previewing {
        None
    } else {
        Some(app.schema_status())
    };
    let mut block = block(
        &numbered(app, Focus::Schema, "Schema"),
        app.focus == Focus::Schema,
    );
    if let Some(status) = status {
        block = block.title_top(
            Line::styled(format!(" {status} "), Style::new().fg(Color::DarkGray)).right_aligned(),
        );
    }
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let file_mode = match (previewing, &request.grpc.schema) {
        (true, SchemaSource::File(_)) => true,
        (true, SchemaSource::Reflection) => false,
        (false, _) => app.schema_mode == SchemaMode::File,
    };
    if file_mode && !previewing {
        if app.schema_path_text().is_empty() && app.focus != Focus::Schema {
            frame.render_widget(
                Paragraph::new(Line::styled(
                    "file: path to a .proto, .bin, .pb or .desc (Ctrl+O: reflection)",
                    Style::new().fg(Color::DarkGray),
                )),
                inner,
            );
        } else {
            render_editor(
                frame,
                &app.schema_path,
                inner,
                app.focus == Focus::Schema,
                Syntax::Plain,
            );
        }
    } else if let (true, SchemaSource::File(path)) = (previewing, &request.grpc.schema) {
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("file: ", Style::new().fg(Color::Cyan)),
                Span::raw(path.to_string_lossy().into_owned()),
            ])),
            inner,
        );
    } else {
        let hint = if previewing {
            ""
        } else {
            "  (Ctrl+O: use a file, Ctrl+L: load)"
        };
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("reflection", Style::new().fg(Color::Cyan)),
                Span::styled(hint, Style::new().fg(Color::DarkGray)),
            ])),
            inner,
        );
    }
}

/// gRPC: the selected `service/method` and its call kind.
pub(crate) fn draw_rpc(
    frame: &mut Frame,
    app: &App,
    request: &crate::tui::app::RequestView,
    previewing: bool,
    area: Rect,
) {
    let title = if previewing {
        "Method (preview)"
    } else {
        "Method (Enter: pick)"
    };
    let block = block(
        &numbered(app, Focus::Method, title),
        app.focus == Focus::Method,
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let line = if request.grpc.method.is_empty() {
        Line::styled(
            "none selected: load the schema, then pick a method",
            Style::new().fg(Color::DarkGray),
        )
    } else {
        let mut spans = vec![Span::styled(
            request.grpc.method.clone(),
            Style::new().fg(Color::Yellow),
        )];
        if let Some(kind) = app.method_kind(&request.grpc.method) {
            spans.push(Span::styled(
                format!("  [{}]", kind.label()),
                Style::new().fg(Color::Magenta),
            ));
        }
        Line::from(spans)
    };
    frame.render_widget(Paragraph::new(line), inner);
}

/// GraphQL: the selected `Query/field`, picked from the introspected schema.
pub(crate) fn draw_operation(
    frame: &mut Frame,
    app: &App,
    request: &crate::tui::app::RequestView,
    previewing: bool,
    area: Rect,
) {
    let title = if previewing {
        "Operation (preview)"
    } else {
        "Operation (Enter: pick, Ctrl+L: introspect)"
    };
    let mut block = block(
        &numbered(app, Focus::Method, title),
        app.focus == Focus::Method,
    );
    if !previewing {
        block = block.title_top(
            Line::styled(
                format!(" {} ", app.schema_status()),
                Style::new().fg(Color::DarkGray),
            )
            .right_aligned(),
        );
    }
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let line = if request.grpc.method.is_empty() {
        Line::styled(
            "none selected: introspect the endpoint, then pick a query or mutation",
            Style::new().fg(Color::DarkGray),
        )
    } else {
        Line::styled(request.grpc.method.clone(), Style::new().fg(Color::Yellow))
    };
    frame.render_widget(Paragraph::new(line), inner);
}
