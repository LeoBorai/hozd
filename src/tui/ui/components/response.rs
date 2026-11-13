//! The response panel.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use ratatui_image::{Resize, StatefulImage};

use crate::history::Outcome;

use crate::tui::app::{App, Focus};
use crate::tui::syntax_highlight::json::{highlight_line, pretty};

use super::super::layout::*;
use super::*;

pub(crate) fn draw_right(frame: &mut Frame, app: &App, outcome: Option<&Outcome>, area: Rect) {
    let (status_area, headers_area, body_area) = right_areas(area);

    let status_block = block("Response", false);
    let status_inner = status_block.inner(status_area);
    frame.render_widget(status_block, status_area);
    let status_line = match outcome {
        Some(Outcome::Ws(w)) if app.ws_open() && !app.sending => {
            let mut spans = vec![
                Span::styled(
                    "OPEN",
                    Style::new().fg(Color::Green).add_modifier(Modifier::BOLD),
                ),
                Span::raw(format!("  {} messages", w.messages.len())),
            ];
            spans.push(Span::styled("  ^X close", Style::new().fg(Color::Yellow)));
            Line::from(spans)
        }
        Some(Outcome::Grpc(g)) if app.sending => {
            let label = if g.messages.is_empty() && !g.response_stream {
                "sending...".to_owned()
            } else {
                format!("streaming {}", g.messages.len())
            };
            let mut spans = rainbow_spans(&label, app.frame);
            spans.push(Span::styled("  ^X cancel", Style::new().fg(Color::Yellow)));
            Line::from(spans)
        }
        _ if app.sending => Line::from(rainbow_spans("sending...", app.frame)),
        Some(Outcome::Response(r)) => Line::from(vec![
            Span::styled(
                r.status.to_string(),
                status_style(r.status).add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!("  {} ms", r.elapsed.as_millis())),
        ]),
        Some(Outcome::Grpc(g)) => match &g.status {
            Some(s) => Line::from(vec![
                Span::styled(
                    format!("{} ({})", s.name(), s.code),
                    Style::new()
                        .fg(if s.code == 0 {
                            Color::Green
                        } else {
                            Color::Red
                        })
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(format!("  {} ms", g.elapsed.as_millis())),
                Span::styled(
                    match s.trailers.len() {
                        0 => String::new(),
                        1 => "  1 trailer".to_owned(),
                        n => format!("  {n} trailers"),
                    },
                    Style::new().fg(Color::DarkGray),
                ),
            ]),
            None => Line::styled("failed", Style::new().fg(Color::Red)),
        },
        Some(Outcome::Ws(w)) => match (&w.error, w.connected) {
            (Some(_), false) => Line::styled("failed", Style::new().fg(Color::Red)),
            (Some(_), true) => Line::from(vec![
                Span::styled(
                    "ERROR",
                    Style::new().fg(Color::Red).add_modifier(Modifier::BOLD),
                ),
                Span::raw(format!("  {} ms", w.elapsed.as_millis())),
            ]),
            (None, _) => Line::from(vec![
                Span::styled(
                    match &w.close {
                        Some(c) => format!("CLOSED ({})", c.code),
                        None => "CLOSED".to_owned(),
                    },
                    Style::new().add_modifier(Modifier::BOLD),
                ),
                Span::raw(format!("  {} ms", w.elapsed.as_millis())),
            ]),
        },
        Some(Outcome::Error(_)) => Line::styled("failed", Style::new().fg(Color::Red)),
        None => Line::styled("no response yet", Style::new().fg(Color::DarkGray)),
    };
    frame.render_widget(Paragraph::new(status_line), status_inner);

    let headers_block = block("Headers", false);
    let headers_inner = headers_block.inner(headers_area);
    frame.render_widget(headers_block, headers_area);
    let header_lines: Vec<Line> = match outcome {
        Some(Outcome::Ws(w)) => w
            .headers
            .iter()
            .map(|h| {
                Line::from(vec![
                    Span::styled(format!("{}: ", h.name), Style::new().fg(Color::Cyan)),
                    Span::raw(h.value.clone()),
                ])
            })
            .collect(),
        Some(Outcome::Response(r)) => r
            .headers
            .iter()
            .map(|h| {
                Line::from(vec![
                    Span::styled(format!("{}: ", h.name), Style::new().fg(Color::Cyan)),
                    Span::raw(h.value.clone()),
                ])
            })
            .collect(),
        Some(Outcome::Grpc(g)) => {
            let trailers = g.status.iter().flat_map(|s| s.trailers.iter());
            g.metadata
                .iter()
                .map(|h| (h, Color::Cyan, ""))
                .chain(trailers.map(|h| (h, Color::Blue, "trailer ")))
                .map(|(h, color, prefix)| {
                    Line::from(vec![
                        Span::styled(format!("{prefix}{}: ", h.name), Style::new().fg(color)),
                        Span::raw(h.value.clone()),
                    ])
                })
                .collect()
        }
        _ => Vec::new(),
    };
    frame.render_widget(Paragraph::new(header_lines), headers_inner);

    let body_block = block(
        &numbered(app, Focus::Response, "Body (Enter: open)"),
        app.focus == Focus::Response,
    );
    let body_inner = body_block.inner(body_area);
    frame.render_widget(body_block, body_area);
    let body_lines: Vec<Line> = match outcome {
        Some(Outcome::Response(r)) => pretty(&r.body).lines().map(highlight_line).collect(),
        Some(Outcome::Error(msg)) => vec![Line::styled(msg.clone(), Style::new().fg(Color::Red))],
        Some(Outcome::Grpc(g)) => grpc_body_lines(g),
        Some(Outcome::Ws(w)) => ws_body_lines(w),
        None => Vec::new(),
    };
    let streaming = matches!(outcome, Some(Outcome::Grpc(g)) if !g.messages.is_empty());
    if app.sending && !streaming {
        draw_spinner(frame, body_inner, app.frame);
        return;
    }
    if let Some(Outcome::Response(r)) = outcome
        && let Some(bytes) = &r.image
    {
        let drawn = app.with_image(bytes, |protocol| match protocol {
            Some(protocol) => {
                frame.render_stateful_widget(
                    StatefulImage::default().resize(Resize::Fit(None)),
                    body_inner,
                    protocol,
                );
                true
            }
            None => false,
        });
        if !drawn {
            let line = Line::styled("cannot decode image", Style::new().fg(Color::Red));
            frame.render_widget(
                Paragraph::new(vec![line, Line::raw(r.body.clone())]),
                body_inner,
            );
        }
        return;
    }
    frame.render_widget(
        Paragraph::new(body_lines)
            .wrap(ratatui::widgets::Wrap { trim: false })
            .scroll((app.scroll, 0)),
        body_inner,
    );
}

/// The messages of a gRPC call, numbered when there can be several, then the status
/// message of a call that did not end `OK`.
pub(crate) fn grpc_body_lines(g: &crate::client::GrpcOutcome) -> Vec<Line<'static>> {
    if let (None, Some(error)) = (&g.status, &g.error) {
        return vec![Line::styled(error.clone(), Style::new().fg(Color::Red))];
    }
    let numbered = g.response_stream || g.messages.len() > 1;
    let mut lines = Vec::new();
    for (i, message) in g.messages.iter().enumerate() {
        if numbered {
            if i > 0 {
                lines.push(Line::raw(""));
            }
            lines.push(Line::styled(
                format!("#{}", i + 1),
                Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD),
            ));
        }
        lines.extend(pretty(message).lines().map(highlight_line));
    }
    if let Some(status) = g.status.as_ref().filter(|s| s.code != 0) {
        if !lines.is_empty() {
            lines.push(Line::raw(""));
        }
        let text = if status.message.is_empty() {
            status.name().to_owned()
        } else {
            format!("{}: {}", status.name(), status.message)
        };
        lines.push(Line::styled(text, Style::new().fg(Color::Red)));
    }
    lines
}

/// The conversation of a WebSocket session: `->` sent (green), `<-` received (cyan), then
/// how it ended.
pub(crate) fn ws_body_lines(w: &crate::client::WsOutcome) -> Vec<Line<'static>> {
    use crate::client::WsDirection;
    let mut lines = Vec::new();
    for message in &w.messages {
        let (arrow, color) = match message.direction {
            WsDirection::Sent => ("->", Color::Green),
            WsDirection::Received => ("<-", Color::Cyan),
        };
        let text = pretty(&message.text);
        let mut body = text.lines().map(highlight_line);
        let mut first = body.next().unwrap_or_default();
        first
            .spans
            .insert(0, Span::styled(format!("{arrow} "), Style::new().fg(color)));
        lines.push(first);
        lines.extend(body.map(|mut l| {
            l.spans.insert(0, Span::raw("   "));
            l
        }));
    }
    if let Some(error) = &w.error {
        lines.push(Line::styled(error.clone(), Style::new().fg(Color::Red)));
    } else if let Some(close) = &w.close {
        lines.push(Line::styled(
            close.to_string(),
            Style::new().fg(Color::DarkGray),
        ));
    }
    lines
}
