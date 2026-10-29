use std::collections::VecDeque;
use std::time::{SystemTime, UNIX_EPOCH};

use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};

/// Entries kept; the oldest are dropped first.
const CAPACITY: usize = 2000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Info,
    /// A line of the connection trace; its first character (`*`, `>` or `<`) says which.
    Detail,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// `HH:MM:SS`, UTC.
    pub time: String,
    pub level: Level,
    pub message: String,
}

/// A bounded in-memory log shown in the left panel.
#[derive(Debug, Default)]
pub struct Log {
    entries: VecDeque<Entry>,
}

impl Log {
    pub fn push(&mut self, level: Level, message: impl Into<String>) {
        if self.entries.len() == CAPACITY {
            self.entries.pop_front();
        }
        self.entries.push_back(Entry {
            time: clock(),
            level,
            message: message.into(),
        });
    }

    pub fn entries(&self) -> impl DoubleEndedIterator<Item = &Entry> + ExactSizeIterator {
        self.entries.iter()
    }

    /// One line per entry, oldest first, as copied from the reading panel.
    pub fn to_text(&self) -> String {
        self.entries
            .iter()
            .map(|e| {
                let level = match e.level {
                    Level::Info => "INFO ",
                    Level::Detail => "TRACE",
                    Level::Error => "ERROR",
                };
                format!("{} {level} {}", e.time, e.message)
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

fn clock() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let day = secs % 86_400;
    format!("{:02}:{:02}:{:02}", day / 3600, day / 60 % 60, day % 60)
}

/// The color of a log message: errors red, trace lines by direction (`>` sent, `<`
/// received, anything else dim).
pub fn message_color(level: Level, message: &str) -> Color {
    match level {
        Level::Info => Color::Reset,
        Level::Error => Color::Red,
        Level::Detail => match message.chars().next() {
            Some('>') => Color::Cyan,
            Some('<') => Color::Green,
            _ => Color::DarkGray,
        },
    }
}

/// Width of `HH:MM:SS LEVEL ` in [`Log::to_text`].
const PREFIX: usize = 15;

/// Colors one line of [`Log::to_text`]: `HH:MM:SS LEVEL message`.
pub fn highlight_line(line: &str) -> Line<'static> {
    let level = line
        .get(9..14)
        .and_then(|l| match l.trim_end() {
            "INFO" => Some(Level::Info),
            "ERROR" => Some(Level::Error),
            "TRACE" => Some(Level::Detail),
            _ => None,
        })
        .filter(|_| line.is_char_boundary(PREFIX));
    let Some(level) = level else {
        return Line::raw(line.to_owned());
    };
    let (prefix, message) = line.split_at(PREFIX);
    Line::from(vec![
        Span::styled(prefix.to_owned(), Style::new().fg(Color::DarkGray)),
        Span::styled(
            message.to_owned(),
            Style::new().fg(message_color(level, message)),
        ),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modal_text_lines_are_colored_by_level_and_direction() {
        let mut log = Log::default();
        log.push(Level::Detail, "> GET / HTTP/2");
        log.push(Level::Detail, "< HTTP/2 200");
        log.push(Level::Error, "boom");
        let text = log.to_text();
        let colors: Vec<Color> = text
            .lines()
            .map(|l| highlight_line(l).spans[1].style.fg.unwrap())
            .collect();
        assert_eq!(colors, [Color::Cyan, Color::Green, Color::Red]);
        assert_eq!(highlight_line("plain").spans.len(), 1);
    }
}
