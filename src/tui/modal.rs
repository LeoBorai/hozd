//! The big reading panel for a response body.

use std::cell::{Cell, RefCell};

use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthChar;

use super::{html, json};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Json,
    Html,
    Text,
}

impl Kind {
    pub fn detect(text: &str) -> Kind {
        if json::try_pretty(text).is_some() {
            Kind::Json
        } else if html::looks_like_html(text) {
            Kind::Html
        } else {
            Kind::Text
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Kind::Json => "JSON",
            Kind::Html => "HTML",
            Kind::Text => "text",
        }
    }
}

struct Wrapped {
    width: usize,
    formatted: bool,
    lines: Vec<Line<'static>>,
}

/// A response body open for reading. Starts as received; `F` toggles a formatted view.
pub struct Modal {
    raw: String,
    formatted: Option<String>,
    show_formatted: bool,
    kind: Kind,
    pub scroll: usize,
    /// Set by the renderer, which knows the size: how far the text can scroll and how many
    /// rows a page is.
    max_scroll: Cell<usize>,
    page: Cell<usize>,
    wrapped: RefCell<Option<Wrapped>>,
}

impl Modal {
    pub fn new(raw: String) -> Self {
        let kind = Kind::detect(&raw);
        Self {
            raw,
            formatted: None,
            show_formatted: false,
            kind,
            scroll: 0,
            max_scroll: Cell::new(0),
            page: Cell::new(1),
            wrapped: RefCell::new(None),
        }
    }

    pub fn kind(&self) -> Kind {
        self.kind
    }

    pub fn is_formatted(&self) -> bool {
        self.show_formatted
    }

    /// The text as currently shown (and as copied).
    pub fn displayed(&self) -> &str {
        match (&self.formatted, self.show_formatted) {
            (Some(text), true) => text,
            _ => &self.raw,
        }
    }

    /// Switches between the received text and its formatted form. `Err` explains why the
    /// body cannot be formatted.
    pub fn toggle_format(&mut self) -> Result<(), String> {
        if self.show_formatted {
            self.show_formatted = false;
        } else {
            if self.formatted.is_none() {
                self.formatted = Some(match self.kind {
                    Kind::Json => json::try_pretty(&self.raw).unwrap_or_else(|| self.raw.clone()),
                    Kind::Html => html::format(&self.raw),
                    Kind::Text => return Err("not JSON or HTML, nothing to format".into()),
                });
            }
            self.show_formatted = true;
        }
        self.scroll = 0;
        *self.wrapped.borrow_mut() = None;
        Ok(())
    }

    pub fn scroll_by(&mut self, delta: isize) {
        self.scroll = self
            .scroll
            .saturating_add_signed(delta)
            .min(self.max_scroll.get());
    }

    pub fn scroll_page(&mut self, direction: isize) {
        self.scroll_by(direction * self.page.get().max(1) as isize);
    }

    pub fn scroll_to_end(&mut self) {
        self.scroll = self.max_scroll.get();
    }

    /// Runs `f` on the highlighted text wrapped to `width` columns. The wrapped lines are
    /// cached, so a large body is only processed again when the width or the view changes.
    pub fn with_lines<R>(&self, width: usize, f: impl FnOnce(&[Line<'static>]) -> R) -> R {
        let mut cache = self.wrapped.borrow_mut();
        let stale = cache
            .as_ref()
            .is_none_or(|w| w.width != width || w.formatted != self.show_formatted);
        if stale {
            let lines = self
                .displayed()
                .lines()
                .map(|l| match self.kind {
                    Kind::Json => json::highlight_line(l),
                    Kind::Html => html::highlight_line(l),
                    Kind::Text => Line::raw(l.to_owned()),
                })
                .flat_map(|l| wrap_line(l, width))
                .collect();
            *cache = Some(Wrapped {
                width,
                formatted: self.show_formatted,
                lines,
            });
        }
        f(&cache.as_ref().expect("filled above").lines)
    }

    /// Records the viewport so scrolling stops at the end of the text.
    pub fn set_viewport(&self, total_rows: usize, height: usize) {
        self.max_scroll.set(total_rows.saturating_sub(height));
        self.page.set(height.saturating_sub(1).max(1));
    }
}

/// Splits a styled line into rows of at most `width` display columns, keeping styles.
pub fn wrap_line(line: Line<'static>, width: usize) -> Vec<Line<'static>> {
    if width == 0 {
        return vec![line];
    }
    let mut rows: Vec<Vec<Span<'static>>> = vec![Vec::new()];
    let mut used = 0;

    for span in line.spans {
        let style = span.style;
        let mut chunk = String::new();
        for c in span.content.chars() {
            let c_width = if c == '\t' { 4 } else { c.width().unwrap_or(0) };
            if used + c_width > width && used > 0 {
                if !chunk.is_empty() {
                    rows.last_mut()
                        .expect("never empty")
                        .push(Span::styled(std::mem::take(&mut chunk), style));
                }
                rows.push(Vec::new());
                used = 0;
            }
            if c == '\t' {
                chunk.push_str("    ");
            } else {
                chunk.push(c);
            }
            used += c_width;
        }
        if !chunk.is_empty() {
            rows.last_mut()
                .expect("never empty")
                .push(Span::styled(chunk, style));
        }
    }
    rows.into_iter()
        .map(|spans| Line::from(spans).style(line.style))
        .collect()
}

#[cfg(test)]
mod tests {
    use ratatui::style::{Color, Style};

    use super::*;

    fn text(lines: &[Line<'static>]) -> Vec<String> {
        lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect()
    }

    #[test]
    fn detects_the_kind() {
        assert_eq!(Kind::detect("{\"a\":1}"), Kind::Json);
        assert_eq!(Kind::detect("[1,2]"), Kind::Json);
        assert_eq!(Kind::detect("<html><p>x</p></html>"), Kind::Html);
        assert_eq!(Kind::detect("42"), Kind::Text);
        assert_eq!(Kind::detect("hello"), Kind::Text);
    }

    #[test]
    fn f_formats_json_and_toggles_back() {
        let mut m = Modal::new("{\"a\":[1,2]}".into());
        assert_eq!(m.displayed(), "{\"a\":[1,2]}");
        m.toggle_format().unwrap();
        assert!(m.is_formatted());
        assert_eq!(m.displayed(), "{\n  \"a\": [\n    1,\n    2\n  ]\n}");
        m.toggle_format().unwrap();
        assert_eq!(m.displayed(), "{\"a\":[1,2]}");
    }

    #[test]
    fn f_formats_html() {
        let mut m = Modal::new("<div><p>hi</p></div>".into());
        m.toggle_format().unwrap();
        assert_eq!(m.displayed(), "<div>\n  <p>\n    hi\n  </p>\n</div>");
    }

    #[test]
    fn other_text_is_not_formattable() {
        let mut m = Modal::new("just words".into());
        let err = m.toggle_format().unwrap_err();
        assert!(err.contains("nothing to format"), "{err}");
        assert!(!m.is_formatted());
        assert_eq!(m.displayed(), "just words");
    }

    #[test]
    fn wrapping_keeps_styles_and_respects_width() {
        let red = Style::new().fg(Color::Red);
        let line = Line::from(vec![
            Span::raw("ab"),
            Span::styled("cdef", red),
            Span::raw("g"),
        ]);
        let rows = wrap_line(line, 3);
        assert_eq!(text(&rows), ["abc", "def", "g"]);
        assert_eq!(rows[0].spans[1].style, red);
        assert_eq!(rows[1].spans[0].style, red);
        assert_eq!(rows[2].spans[0].style, Style::new());
    }

    #[test]
    fn wrapping_counts_wide_characters_as_two_columns() {
        let rows = wrap_line(Line::raw("\u{4e2d}\u{6587}\u{5b57}"), 4);
        assert_eq!(text(&rows), ["\u{4e2d}\u{6587}", "\u{5b57}"]);
        assert_eq!(text(&wrap_line(Line::raw(""), 5)), [""]);
    }

    #[test]
    fn scrolling_stops_at_the_end_and_the_start() {
        let mut m = Modal::new((0..100).map(|i| format!("line {i}\n")).collect());
        let total = m.with_lines(20, |l| l.len());
        assert_eq!(total, 100);
        m.set_viewport(total, 10);
        m.scroll_by(-5);
        assert_eq!(m.scroll, 0);
        m.scroll_by(1000);
        assert_eq!(m.scroll, 90);
        m.scroll_to_end();
        assert_eq!(m.scroll, 90);
        m.scroll_to_end();
        m.scroll_page(-1);
        assert_eq!(
            m.scroll, 81,
            "a page is the viewport minus one row of overlap"
        );
    }

    #[test]
    fn wrapped_lines_are_recomputed_when_width_or_view_changes() {
        let mut m = Modal::new("{\"key\":\"a long value that wraps\"}".into());
        let narrow = m.with_lines(10, |l| l.len());
        let wide = m.with_lines(80, |l| l.len());
        assert!(narrow > wide);
        m.toggle_format().unwrap();
        assert_eq!(m.with_lines(80, |l| l.len()), 3);
    }
}
