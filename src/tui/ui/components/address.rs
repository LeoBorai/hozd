//! The URL / address box with the kind selector.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::client::Kind;

use crate::tui::app::{App, Focus};

use super::*;

/// The URL (REST) or address (gRPC) box, with the kind selector on its top border.
pub(crate) fn draw_address(
    frame: &mut Frame,
    app: &App,
    request: &crate::tui::app::RequestView,
    previewing: bool,
    area: Rect,
) {
    let title = match (request.kind, previewing) {
        (Kind::Http, false) => "URL",
        (Kind::Http, true) => "URL (preview)",
        (Kind::Grpc, false) => "Address",
        (Kind::Grpc, true) => "Address (preview)",
        (Kind::Ws, false) => "URL (ws://)",
        (Kind::Ws, true) => "URL (ws://) (preview)",
        (Kind::Graphql, false) => "Endpoint",
        (Kind::Graphql, true) => "Endpoint (preview)",
    };
    let selected = Style::new()
        .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
        .fg(Color::Yellow);
    let (rest_style, grpc_style) = match request.kind {
        Kind::Http => (selected, Style::new()),
        Kind::Grpc => (Style::new(), selected),
        Kind::Ws | Kind::Graphql => (Style::new(), Style::new()),
    };
    let graphql_style = if request.kind == Kind::Graphql {
        selected
    } else {
        Style::new()
    };
    let ws_style = if request.kind == Kind::Ws {
        selected
    } else {
        Style::new()
    };
    let mut title_spans = vec![Span::raw(format!(
        " {} ",
        if previewing {
            title.to_owned()
        } else {
            numbered(app, Focus::Url, title)
        }
    ))];
    if request.kind == Kind::Grpc && request.grpc.insecure && is_tls(&request.url) {
        title_spans.push(Span::styled(
            "- NOT VERIFIED ",
            Style::new().fg(Color::Red).add_modifier(Modifier::BOLD),
        ));
    }
    let block = block_titled(Line::from(title_spans), app.focus == Focus::Url).title_top(
        Line::from(vec![
            Span::raw(" "),
            Span::styled("REST", rest_style),
            Span::raw("|"),
            Span::styled("GQL", graphql_style),
            Span::raw("|"),
            Span::styled("gRPC", grpc_style),
            Span::raw("|"),
            Span::styled("WS", ws_style),
            Span::raw(" "),
        ])
        .right_aligned(),
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if previewing {
        frame.render_widget(Paragraph::new(request.url.clone()), inner);
    } else {
        render_editor(
            frame,
            &app.url,
            inner,
            app.focus == Focus::Url,
            Syntax::Plain,
        );
    }
}

pub(crate) fn is_tls(address: &str) -> bool {
    address.trim_start().starts_with("https://")
}

pub(crate) fn tls_span(address: &str, insecure: bool) -> Span<'static> {
    match (is_tls(address), insecure) {
        (false, _) => Span::styled("plain", Style::new().fg(Color::DarkGray)),
        (true, false) => Span::styled("verified", Style::new().fg(Color::Green)),
        (true, true) => Span::styled(
            "insecure",
            Style::new().fg(Color::Red).add_modifier(Modifier::BOLD),
        ),
    }
}
