//! The status bar.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::client::Kind;

use crate::tui::app::App;

use super::super::layout::*;

/// Both fit a 120-column terminal; the README has the full key tables.
pub const HELP_REST: &str = "Tab focus | Ctrl+G kind | j/k move | Enter load | Ctrl+S send | Ctrl+N new | Ctrl+P method | ? keys";
pub const HELP_GRPC: &str = "Ctrl+G kind | Ctrl+L schema | Ctrl+P method | Ctrl+N new | Ctrl+S send | Ctrl+X cancel | ? keys";

pub const HELP_GRAPHQL: &str = "Ctrl+G kind | Ctrl+L schema | Ctrl+P operation | Ctrl+N new | Ctrl+S send | Ctrl+T tabs | ? keys";

pub const HELP_WS: &str = "Ctrl+G kind | Ctrl+S connect, then send body | Ctrl+X close | Ctrl+N new | Ctrl+T headers | ? keys";

pub(crate) fn draw_status(frame: &mut Frame, app: &App, area: Rect) {
    let text = match &app.status {
        Some(message) => Line::styled(message.clone(), Style::new().fg(Color::Red)),
        None if app.notice.is_some() => Line::styled(
            app.notice.clone().unwrap_or_default(),
            Style::new().fg(Color::Green),
        ),
        None => Line::styled(
            match app.kind {
                Kind::Http => HELP_REST,
                Kind::Grpc => HELP_GRPC,
                Kind::Ws => HELP_WS,
                Kind::Graphql => HELP_GRAPHQL,
            },
            Style::new().fg(Color::DarkGray),
        ),
    };
    // The link and the version tag stay in the bottom right corner whatever the message says.
    let (link, tag) = status_tags(area);
    let left = Rect::new(area.x, area.y, link.x.saturating_sub(area.x), area.height);
    frame.render_widget(Paragraph::new(text), left);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::raw(" "),
            Span::styled(
                CONTRIBUTE.trim(),
                Style::new()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::UNDERLINED),
            ),
            Span::raw(" "),
        ])),
        link,
    );
    frame.render_widget(
        Paragraph::new(Line::styled(
            format!(" {VERSION} "),
            Style::new().fg(Color::Yellow),
        )),
        tag,
    );
}

/// The link to the source, clickable in the status bar.
pub const REPO_URL: &str = "https://github.com/LeoBorai/hozd";
pub(crate) const CONTRIBUTE: &str = " Contribute ";

/// The status bar line of a screen.
pub(crate) fn status_area(screen: Rect) -> Rect {
    main_area(screen).1
}

/// The rects of the "Contribute" link and the version tag in the status bar.
pub(crate) fn status_tags(status: Rect) -> (Rect, Rect) {
    let tag_w = (VERSION.len() as u16 + 2).min(status.width);
    let link_w = (CONTRIBUTE.len() as u16).min(status.width - tag_w);
    let tag = Rect::new(status.right() - tag_w, status.y, tag_w, status.height);
    let link = Rect::new(tag.x - link_w, status.y, link_w, status.height);
    (link, tag)
}

/// Where the "Contribute" link sits on `screen`.
pub fn contribute_area(screen: Rect) -> Rect {
    status_tags(status_area(screen)).0
}

/// Columns the status bar leaves for its message.
pub fn status_text_width(screen_width: u16) -> usize {
    let (link, _) = status_tags(status_area(Rect::new(0, 0, screen_width, 2)));
    link.x as usize
}

/// The release tag shown in the status bar, e.g. `v0.1.4`.
pub const VERSION: &str = concat!("v", env!("CARGO_PKG_VERSION"));
