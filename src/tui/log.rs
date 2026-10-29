use std::collections::VecDeque;
use std::time::{SystemTime, UNIX_EPOCH};

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
