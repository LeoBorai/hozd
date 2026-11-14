//! The center panel's Metadata tab: title and description of the selected project,
//! directory or request.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Tabs, Wrap};

use crate::tui::app::{App, CenterTab, ProjectRowKind};

use super::*;

/// The Request / Metadata tab row.
pub(crate) fn draw_center_tabs(frame: &mut Frame, app: &App, area: Rect) {
    let selected = match app.center_tab {
        CenterTab::Request => 0,
        CenterTab::Metadata => 1,
    };
    frame.render_widget(
        Tabs::new(["Request", "Metadata"])
            .select(selected)
            .highlight_style(
                Style::new()
                    .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
                    .fg(Color::Yellow),
            )
            .divider("|"),
        area,
    );
}

pub(crate) fn draw_metadata(frame: &mut Frame, app: &App, area: Rect) {
    let outer = block("Metadata", false);
    let inner = outer.inner(area);
    frame.render_widget(outer, area);

    let label = |text: &str| {
        Line::from(Span::styled(
            text.to_owned(),
            Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        ))
    };
    let lines = match app.selected_project_row() {
        None => vec![Line::styled(
            "select a project, directory or request",
            Style::new().fg(Color::DarkGray),
        )],
        Some(row) => {
            let what = match row.kind {
                ProjectRowKind::Project { .. } => "Project",
                ProjectRowKind::Directory { .. } => "Directory",
                ProjectRowKind::Request { .. } => "Request",
            };
            let description = match row.description.as_deref().filter(|d| !d.is_empty()) {
                Some(d) => Line::raw(d.to_owned()),
                None => Line::styled("none", Style::new().fg(Color::DarkGray)),
            };
            vec![
                label(&format!("Title ({what})")),
                Line::raw(row.name),
                Line::raw(""),
                label("Description"),
                description,
            ]
        }
    };
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}
