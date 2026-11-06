//! The sending spinner.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;
use ratatui::widgets::Paragraph;

pub(crate) const SPINNER_COLORS: [Color; 4] =
    [Color::Cyan, Color::Green, Color::Yellow, Color::Red];

/// `text` with the spinner colors cycling along it, shifting with `tick`.
pub(crate) fn rainbow_spans(text: &str, tick: usize) -> Vec<Span<'static>> {
    text.chars()
        .enumerate()
        .map(|(i, c)| {
            let color = SPINNER_COLORS[(i + tick) % SPINNER_COLORS.len()];
            Span::styled(
                c.to_string(),
                Style::new().fg(color).add_modifier(Modifier::BOLD),
            )
        })
        .collect()
}

/// Wheel cells in clockwise order, as (column, row) on a 3x3 grid, each column 2 cells wide.
pub(crate) const WHEEL: [(u16, u16); 8] = [
    (0, 0),
    (1, 0),
    (2, 0),
    (2, 1),
    (2, 2),
    (1, 2),
    (0, 2),
    (0, 1),
];

/// A spinning wheel of four color quadrants (cyan, green, yellow, red) centered in `area`.
pub(crate) fn draw_spinner(frame: &mut Frame, area: Rect, tick: usize) {
    const WIDTH: u16 = 5;
    const HEIGHT: u16 = 3;
    if area.width < WIDTH || area.height < HEIGHT {
        return;
    }
    let left = area.x + (area.width - WIDTH) / 2;
    let top = area.y + (area.height - HEIGHT) / 2;
    for (i, (col, row)) in WHEEL.iter().enumerate() {
        let color = SPINNER_COLORS[((i + tick) % 8) / 2];
        frame.render_widget(
            Paragraph::new(Span::styled("●", Style::new().fg(color))),
            Rect::new(left + col * 2, top + row, 1, 1),
        );
    }
}
