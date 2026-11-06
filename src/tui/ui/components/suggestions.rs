//! Path completion list under the schema box.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Clear, List, ListItem, ListState};

use crate::tui::app::App;

use super::super::layout::*;
use super::*;

/// The most suggestion rows shown at once.
pub(crate) const SUGGESTION_ROWS: u16 = 6;

/// The suggestion list: right under the Schema box (`schema` is its outer rect), as wide
/// as it, over whatever is below.
pub fn suggestions_area(schema: Rect, count: usize) -> Rect {
    let rows = (count as u16).min(SUGGESTION_ROWS);
    Rect::new(schema.x, schema.bottom(), schema.width, rows + 2)
}

/// `text` cut from the left to fit `width` columns, so the end of a long path (the part
/// being completed) stays visible.
pub(crate) fn tail(text: &str, width: usize) -> String {
    let count = text.chars().count();
    if count <= width || width == 0 {
        return text.to_owned();
    }
    let kept: String = text.chars().skip(count - (width - 1)).collect();
    format!("…{kept}")
}

pub(crate) fn draw_suggestions(frame: &mut Frame, app: &App, schema: Rect) {
    let items = &app.suggestions.items;
    let area = suggestions_area(schema, items.len());
    let area = area.intersection(frame.area());
    if area.height < 3 {
        return;
    }
    frame.render_widget(Clear, area);
    let title = Line::styled(
        " Tab accept | Up/Down | Enter ",
        Style::new().fg(Color::DarkGray),
    );
    let outer = block_titled(title, true);
    let inner = outer.inner(area);
    frame.render_widget(outer, area);
    let rows: Vec<ListItem> = items
        .iter()
        .map(|s| {
            let style = if s.dir {
                Style::new().fg(Color::Cyan)
            } else {
                Style::new()
            };
            ListItem::new(Line::styled(tail(&s.text, inner.width as usize), style))
        })
        .collect();
    let mut state = ListState::default()
        .with_selected(app.suggestions.sel)
        .with_offset(list_offset(app.suggestions.sel, inner.height));
    frame.render_stateful_widget(
        List::new(rows).highlight_style(
            Style::new()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        ),
        inner,
        &mut state,
    );
}
