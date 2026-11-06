//! The reading panel popup.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};

use super::super::layout::*;
use super::*;

pub(crate) fn draw_modal(frame: &mut Frame, modal: &crate::tui::modal::Modal, screen: Rect) {
    let area = modal_area(screen);
    frame.render_widget(Clear, area);
    let view = if modal.is_formatted() {
        "formatted"
    } else {
        "as received"
    };
    let title = Line::from(vec![
        Span::styled(
            match modal.kind() {
                crate::tui::modal::Kind::Log => format!(" {} ", modal.title()),
                kind => format!(" {} - {} ({view}) ", modal.title(), kind.label()),
            },
            Style::new().add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "| F format | j/k PgUp/PgDn scroll | drag select+copy | y copy | Esc close ",
            Style::new().fg(Color::DarkGray),
        ),
    ]);
    let outer = block_titled(title, true);
    let inner = outer.inner(area);
    frame.render_widget(outer, area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    modal.with_lines(inner.width as usize, |lines| {
        modal.set_viewport(lines.len(), inner.height as usize);
        let start = modal.top().min(lines.len());
        let end = (start + inner.height as usize).min(lines.len());
        let visible: Vec<Line> = lines[start..end]
            .iter()
            .enumerate()
            .map(|(i, line)| match modal.selected_cols(start + i) {
                Some((from, to)) => crate::tui::modal::reverse_cols(line.clone(), from, to),
                None => line.clone(),
            })
            .collect();
        frame.render_widget(Paragraph::new(visible), inner);
    });
}
