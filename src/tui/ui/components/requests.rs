//! The left panel: stored requests and history.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, ListState, Tabs};

use crate::client::{Kind, Method, status_name};
use crate::project::TemplateKind;

use crate::tui::app::{App, Focus, Tab};

use super::super::layout::*;
use super::*;

pub(crate) fn draw_left(frame: &mut Frame, app: &App, area: Rect) {
    let (requests, log) = left_split(area);
    draw_requests(frame, app, requests);
    draw_log(frame, app, log);
}

pub(crate) fn draw_requests(frame: &mut Frame, app: &App, area: Rect) {
    let focused = app.focus == Focus::List;
    let outer = block(&numbered(app, Focus::List, "Requests"), focused);
    frame.render_widget(outer, area);

    let (tabs, list) = left_areas(area);
    // The Stored tab is hidden; History is the only one shown.
    let selected = 0;
    frame.render_widget(
        Tabs::new(["History"])
            .select(selected)
            .highlight_style(
                Style::new()
                    .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
                    .fg(Color::Yellow),
            )
            .divider("|"),
        tabs,
    );

    let (items, selected_row) = match app.tab {
        Tab::Stored => stored_rows(app),
        Tab::History => history_rows(app),
    };
    let mut state = ListState::default()
        .with_selected(selected_row)
        .with_offset(list_offset(selected_row, list.height));
    let highlight = if focused {
        Style::new()
            .bg(Color::DarkGray)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::new().add_modifier(Modifier::BOLD)
    };
    frame.render_stateful_widget(
        List::new(items).highlight_style(highlight),
        list,
        &mut state,
    );
}

pub(crate) fn stored_rows(app: &App) -> (Vec<ListItem<'static>>, Option<usize>) {
    if app.templates.is_empty() {
        return (vec![dim("no templates in ~/.hozd/projects")], None);
    }
    let mut rows = Vec::new();
    let mut selected = None;
    let mut last_project: Option<&str> = None;
    for (i, t) in app.templates.iter().enumerate() {
        if last_project != Some(t.project.as_str()) {
            rows.push(ListItem::new(Line::styled(
                format!("{}/", t.project),
                Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD),
            )));
            last_project = Some(&t.project);
        }
        if i == app.stored_sel {
            selected = Some(rows.len());
        }
        rows.push(ListItem::new(Line::from(vec![
            Span::raw("  "),
            match &t.template.kind {
                TemplateKind::Http(h) => method_span(h.method),
                TemplateKind::Grpc(_) => Span::styled("gRPC  ", Style::new().fg(Color::Magenta)),
                TemplateKind::Ws(_) => Span::styled("WS    ", Style::new().fg(Color::Green)),
                TemplateKind::Graphql(_) => {
                    Span::styled("GQL   ", Style::new().fg(Color::LightMagenta))
                }
            },
            Span::raw(format!(" {}", t.template.name)),
        ])));
    }
    (rows, selected)
}

pub(crate) fn history_rows(app: &App) -> (Vec<ListItem<'static>>, Option<usize>) {
    if app.history.is_empty() {
        return (vec![dim("no requests sent yet")], None);
    }
    let rows = app
        .history
        .iter()
        .map(|s| match s.kind {
            Kind::Http => {
                let status = match s.status {
                    Some(code) => Span::styled(code.to_string(), status_style(code)),
                    None => Span::styled("ERR", Style::new().fg(Color::Red)),
                };
                let method = match s.method.parse::<Method>() {
                    Ok(method) => method_span(method),
                    Err(_) => Span::raw(format!("{:<6}", s.method)),
                };
                ListItem::new(Line::from(vec![
                    status,
                    Span::raw(" "),
                    method,
                    Span::raw(format!(" {}", s.url)),
                ]))
            }
            Kind::Graphql => {
                let status = match s.status {
                    Some(code) => Span::styled(code.to_string(), status_style(code)),
                    None => Span::styled("ERR", Style::new().fg(Color::Red)),
                };
                ListItem::new(Line::from(vec![
                    status,
                    Span::raw(" "),
                    Span::styled("GQL  ", Style::new().fg(Color::LightMagenta)),
                    Span::raw(s.url.clone()),
                ]))
            }
            Kind::Ws => {
                let status = match s.status {
                    Some(code) => Span::styled(code.to_string(), status_style(code)),
                    None => Span::styled("ERR", Style::new().fg(Color::Red)),
                };
                ListItem::new(Line::from(vec![
                    status,
                    Span::raw(" "),
                    Span::styled("WS   ", Style::new().fg(Color::Green)),
                    Span::raw(s.url.clone()),
                ]))
            }
            Kind::Grpc => {
                let status = match s.status {
                    Some(0) => Span::styled("OK", Style::new().fg(Color::Green)),
                    Some(code) => {
                        Span::styled(status_name(i32::from(code)), Style::new().fg(Color::Red))
                    }
                    None => Span::styled("ERR", Style::new().fg(Color::Red)),
                };
                ListItem::new(Line::from(vec![
                    status,
                    Span::raw(" "),
                    Span::styled("gRPC ", Style::new().fg(Color::Magenta)),
                    Span::raw(short_method(&s.method).to_owned()),
                ]))
            }
        })
        .collect();
    (rows, Some(app.history_sel))
}

/// `user.v1.UserService/GetUser` -> `UserService/GetUser`.
pub(crate) fn short_method(full: &str) -> &str {
    let service_end = full.find('/').unwrap_or(full.len());
    let start = full[..service_end].rfind('.').map_or(0, |i| i + 1);
    &full[start..]
}

pub(crate) fn dim(text: &str) -> ListItem<'static> {
    ListItem::new(Line::styled(
        text.to_owned(),
        Style::new().fg(Color::DarkGray),
    ))
}

pub(crate) fn method_span(method: Method) -> Span<'static> {
    let color = match method {
        Method::Get => Color::Green,
        Method::Post => Color::Yellow,
        Method::Put | Method::Patch => Color::Blue,
        Method::Delete => Color::Red,
        Method::Head | Method::Options => Color::Magenta,
    };
    Span::styled(format!("{:<6}", method.as_str()), Style::new().fg(color))
}

pub(crate) fn status_style(code: u16) -> Style {
    Style::new().fg(match code {
        200..=299 => Color::Green,
        300..=399 => Color::Cyan,
        400..=499 => Color::Yellow,
        _ => Color::Red,
    })
}
