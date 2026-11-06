//! The method / operation picker popup.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, List, ListItem, ListState, Paragraph};

use crate::client::Kind;

use crate::tui::app::{App, PickerTag};

use super::super::layout::*;
use super::*;

pub(crate) fn draw_picker(frame: &mut Frame, app: &App, screen: Rect) {
    let Some(picker) = &app.picker else {
        return;
    };
    let area = picker_area(screen);
    frame.render_widget(Clear, area);
    let title = Line::from(vec![
        Span::styled(
            if app.kind == Kind::Graphql {
                " Pick an operation "
            } else {
                " Pick a method "
            },
            Style::new().add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "| type to filter | Up/Down | Enter | Esc ",
            Style::new().fg(Color::DarkGray),
        ),
    ]);
    let outer = block_titled(title, true);
    let inner = outer.inner(area);
    frame.render_widget(outer, area);
    let list_area = picker_list_area(area);
    if inner.height == 0 {
        return;
    }
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("> ", Style::new().fg(Color::Green)),
            Span::raw(picker.filter.clone()),
        ])),
        Rect::new(inner.x, inner.y, inner.width, 1),
    );

    let matches = app.picker_matches();
    if matches.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::styled(
                if app.kind == Kind::Graphql {
                    "no operation matches"
                } else {
                    "no method matches"
                },
                Style::new().fg(Color::DarkGray),
            )),
            list_area,
        );
        return;
    }
    let map = picker_row_map(app);
    // A header row is always followed by the first method of its service.
    let rows: Vec<ListItem> = map
        .iter()
        .enumerate()
        .map(|(row, entry)| match entry {
            Some(i) => ListItem::new(Line::from(vec![
                Span::raw("  "),
                Span::raw(matches[*i].name.clone()),
                Span::styled(
                    format!("  [{}]", matches[*i].tag.label()),
                    Style::new().fg(tag_color(matches[*i].tag)),
                ),
            ])),
            None => {
                let service = map
                    .get(row + 1)
                    .copied()
                    .flatten()
                    .map(|i| matches[i].group.clone())
                    .unwrap_or_default();
                ListItem::new(Line::styled(
                    service,
                    Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD),
                ))
            }
        })
        .collect();
    let selected_row = map.iter().position(|r| *r == Some(picker.selected));
    let mut state = ListState::default()
        .with_selected(selected_row)
        .with_offset(list_offset(selected_row, list_area.height));
    frame.render_stateful_widget(
        List::new(rows).highlight_style(
            Style::new()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        ),
        list_area,
        &mut state,
    );
}

/// Queries read (green), mutations write (yellow), gRPC calls are tagged by their kind.
pub(crate) fn tag_color(tag: PickerTag) -> Color {
    match tag {
        PickerTag::Query => Color::Green,
        PickerTag::Mutation => Color::Yellow,
        PickerTag::Call(_) => Color::Magenta,
    }
}
