//! Text editor rendering with syntax highlighting.

use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::Line;
use ratatui::widgets::Paragraph;
use ratatui_textarea::TextArea;

use crate::tui::syntax_highlight::graphql;
use crate::tui::syntax_highlight::json::highlight_line;

use super::super::layout::*;

pub(crate) enum Syntax {
    HeaderName,
    Invalid,
    Plain,
    /// An `@path` to a file.
    File,
    Json,
    Graphql,
}

/// Draws a `TextArea`'s lines ourselves (it has no syntax-highlight hook) and places
/// the terminal cursor when the editor has focus.
pub(crate) fn render_editor(
    frame: &mut Frame,
    ta: &TextArea<'static>,
    area: Rect,
    focused: bool,
    syntax: Syntax,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let cursor = ta.cursor();
    let (row, col) = (cursor.0, cursor.1);
    let (row_scroll, col_scroll) = editor_scroll((row, col), area);

    let lines: Vec<Line> = match syntax {
        // Block strings span lines, so the document is highlighted as a whole.
        Syntax::Graphql => graphql::highlight_lines(ta.lines().iter().map(String::as_str)),
        _ => ta
            .lines()
            .iter()
            .map(|l| match syntax {
                Syntax::Json => highlight_line(l),
                Syntax::HeaderName => Line::styled(l.clone(), Style::new().fg(Color::Cyan)),
                Syntax::Invalid => Line::styled(l.clone(), Style::new().fg(Color::Red)),
                Syntax::File => Line::styled(l.clone(), Style::new().fg(Color::Green)),
                Syntax::Plain | Syntax::Graphql => Line::raw(l.clone()),
            })
            .collect(),
    };
    frame.render_widget(
        Paragraph::new(lines).scroll((row_scroll as u16, col_scroll as u16)),
        area,
    );
    if focused {
        frame.set_cursor_position(Position::new(
            area.x + (col - col_scroll) as u16,
            area.y + (row - row_scroll) as u16,
        ));
    }
}
