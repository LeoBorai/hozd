//! The center panel's Metadata tab: title and description of the selected project,
//! directory or request.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Paragraph, Tabs};

use crate::tui::app::{App, CenterTab, Focus, ProjectRowKind};

use super::super::layout::metadata_areas;

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
    let Some(row) = app.selected_project_row() else {
        let outer = block("Metadata", false);
        let inner = outer.inner(area);
        frame.render_widget(outer, area);
        frame.render_widget(
            Paragraph::new(Line::styled(
                "select a project, directory or request",
                Style::new().fg(Color::DarkGray),
            )),
            inner,
        );
        return;
    };
    let what = match row.kind {
        ProjectRowKind::Project { .. } => "Project",
        ProjectRowKind::Directory { .. } => "Directory",
        ProjectRowKind::Request { .. } => "Request",
    };
    let (title, description) = metadata_areas(area);

    let title_block = block(
        &numbered(app, Focus::MetaTitle, &format!("Title ({what})")),
        app.focus == Focus::MetaTitle,
    );
    let inner = title_block.inner(title);
    frame.render_widget(title_block, title);
    render_editor(
        frame,
        &app.meta_title,
        inner,
        app.focus == Focus::MetaTitle,
        Syntax::Plain,
    );

    let desc_block = block(
        &numbered(app, Focus::MetaDescription, "Description"),
        app.focus == Focus::MetaDescription,
    );
    let inner = desc_block.inner(description);
    frame.render_widget(desc_block, description);
    render_editor(
        frame,
        &app.meta_desc,
        inner,
        app.focus == Focus::MetaDescription,
        Syntax::Plain,
    );
}
