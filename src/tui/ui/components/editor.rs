//! Text editor rendering with syntax highlighting.

use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
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
    let lines = match ta.selection_range().filter(|_| focused) {
        Some((start, end)) => apply_selection(lines, start, end),
        None => lines,
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

/// Selected text: dark on light blue, distinct from the plain and focused-cell backgrounds.
pub(crate) const SELECTION: Style = Style::new().bg(Color::LightBlue).fg(Color::Black);

/// Paints the `[start, end)` selection (row, char column) over the highlighted lines.
fn apply_selection(
    lines: Vec<Line<'static>>,
    start: (usize, usize),
    end: (usize, usize),
) -> Vec<Line<'static>> {
    lines
        .into_iter()
        .enumerate()
        .map(|(row, line)| {
            if row < start.0 || row > end.0 {
                return line;
            }
            let from = if row == start.0 { start.1 } else { 0 };
            let to = if row == end.0 { end.1 } else { usize::MAX };
            if from >= to {
                return line;
            }
            let mut spans: Vec<Span<'static>> = Vec::new();
            let mut col = 0;
            for span in line.spans {
                let base = line.style.patch(span.style);
                let mut run = String::new();
                let mut run_sel = false;
                for c in span.content.chars() {
                    let sel = col >= from && col < to;
                    if sel != run_sel && !run.is_empty() {
                        let style = if run_sel { base.patch(SELECTION) } else { base };
                        spans.push(Span::styled(std::mem::take(&mut run), style));
                    }
                    run_sel = sel;
                    run.push(c);
                    col += 1;
                }
                if !run.is_empty() {
                    let style = if run_sel { base.patch(SELECTION) } else { base };
                    spans.push(Span::styled(run, style));
                }
            }
            Line::from(spans)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_marks_only_selected_chars() {
        let out = apply_selection(vec![Line::raw("abcd")], (0, 1), (0, 3));
        let marked: String = out[0]
            .spans
            .iter()
            .filter(|s| s.style.bg == Some(Color::LightBlue))
            .map(|s| s.content.as_ref())
            .collect();
        assert_eq!(marked, "bc");
    }
}
