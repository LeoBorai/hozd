//! The center panel: address, schema and request boxes.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;
use ratatui::widgets::Paragraph;

use crate::client::{Kind, Method};

use crate::tui::app::App;

use super::super::layout::*;
use super::*;

pub(crate) fn draw_center(
    frame: &mut Frame,
    app: &App,
    request: &crate::tui::app::RequestView,
    previewing: bool,
    area: Rect,
) {
    let center = center_areas(area, request.kind);

    let grpc = request.kind == Kind::Grpc;
    let ws = request.kind == Kind::Ws;
    let graphql = request.kind == Kind::Graphql;
    let method_block = block(
        match (grpc, ws) {
            (true, _) => "TLS",
            (_, true) => "Socket",
            _ => "Method",
        },
        false,
    );
    let method_inner = method_block.inner(center.method);
    frame.render_widget(method_block, center.method);
    let method_text = if grpc {
        tls_span(&request.url, request.grpc.insecure)
    } else if ws {
        if app.ws_open() && !previewing {
            Span::styled(
                "OPEN",
                Style::new().fg(Color::Green).add_modifier(Modifier::BOLD),
            )
        } else {
            Span::styled("WS", Style::new().fg(Color::Green))
        }
    } else if graphql {
        method_span(Method::Post)
    } else {
        method_span(request.method)
    };
    frame.render_widget(Paragraph::new(method_text), method_inner);

    draw_address(frame, app, request, previewing, center.url);
    if grpc {
        draw_schema(frame, app, request, previewing, center.schema);
        draw_rpc(frame, app, request, previewing, center.rpc);
    } else if graphql {
        draw_operation(frame, app, request, previewing, center.rpc);
    }
    draw_request_box(frame, app, request, previewing, center.body);
    if center.preview.height > 0 {
        draw_http_preview(frame, &app.with_vars(request), center.preview);
    }
}
