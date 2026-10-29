//! The request Headers tab: one row per header with the name and the value in inputs side
//! by side. Enter on the last value appends a row; rows left empty are not sent.
//!
//! Headers shown read-only (history previews) are rendered as `Name: value` lines.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui_textarea::{CursorMove, Input, TextArea};

use crate::client::Header;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Col {
    Name,
    Value,
}

pub struct Row {
    pub name: TextArea<'static>,
    pub value: TextArea<'static>,
}

impl Row {
    fn new(name: &str, value: &str) -> Self {
        Self {
            name: single_line(name),
            value: single_line(value),
        }
    }

    pub fn name_text(&self) -> String {
        self.name.lines().join("")
    }

    pub fn value_text(&self) -> String {
        self.value.lines().join("")
    }

    fn is_blank(&self) -> bool {
        self.name_text().trim().is_empty() && self.value_text().trim().is_empty()
    }

    /// `Ok(None)` for a blank row, `Err` when the row cannot be a header.
    pub fn header(&self) -> Result<Option<Header>, String> {
        let (name, value) = (self.name_text(), self.value_text());
        let name = name.trim();
        if name.is_empty() {
            return if value.trim().is_empty() {
                Ok(None)
            } else {
                Err("the header name is empty".into())
            };
        }
        if let Some(bad) = name.chars().find(|c| !is_token_char(*c)) {
            return Err(format!("invalid character `{bad}` in the header name"));
        }
        Ok(Some(Header::new(name, value.trim())))
    }
}

fn single_line(text: &str) -> TextArea<'static> {
    let mut ta = TextArea::new(vec![text.to_owned()]);
    ta.move_cursor(CursorMove::End);
    ta
}

fn cursor_col(ta: &TextArea<'static>) -> usize {
    ta.cursor().1
}

fn len(ta: &TextArea<'static>) -> usize {
    ta.lines()[0].chars().count()
}

/// The editable list of headers (metadata in gRPC) and which input has the cursor.
pub struct HeaderEditor {
    pub rows: Vec<Row>,
    pub row: usize,
    pub col: Col,
}

impl Default for HeaderEditor {
    fn default() -> Self {
        Self::from_headers(&[])
    }
}

impl HeaderEditor {
    /// One row per header, plus an empty one to type into.
    pub fn from_headers(headers: &[Header]) -> Self {
        let mut rows: Vec<Row> = headers
            .iter()
            .map(|h| Row::new(&h.name, &h.value))
            .collect();
        rows.push(Row::new("", ""));
        Self {
            row: rows.len() - 1,
            rows,
            col: Col::Name,
        }
    }

    /// The valid headers; invalid rows are skipped (they are shown in red while editing).
    pub fn parse_lenient(&self) -> Vec<Header> {
        self.rows
            .iter()
            .filter_map(|r| r.header().ok().flatten())
            .collect()
    }

    /// All headers, or the first invalid row with its 1-based number.
    pub fn parse(&self) -> Result<Vec<Header>, String> {
        let mut headers = Vec::new();
        for (i, row) in self.rows.iter().enumerate() {
            match row.header() {
                Ok(Some(h)) => headers.push(h),
                Ok(None) => {}
                Err(reason) => return Err(format!("header {}: {reason}", i + 1)),
            }
        }
        Ok(headers)
    }

    /// `Name: value` per non-blank row.
    pub fn lines(&self) -> Vec<String> {
        self.rows
            .iter()
            .filter(|r| !r.is_blank())
            .map(|r| format!("{}: {}", r.name_text(), r.value_text()))
            .collect()
    }

    pub fn cell(&self) -> &TextArea<'static> {
        let row = &self.rows[self.row];
        match self.col {
            Col::Name => &row.name,
            Col::Value => &row.value,
        }
    }

    fn cell_mut(&mut self) -> &mut TextArea<'static> {
        let row = &mut self.rows[self.row];
        match self.col {
            Col::Name => &mut row.name,
            Col::Value => &mut row.value,
        }
    }

    /// Puts the cursor in `col` of `row`, `x` characters in.
    pub fn select(&mut self, row: usize, col: Col, x: u16) {
        self.row = row.min(self.rows.len() - 1);
        self.col = col;
        self.cell_mut().move_cursor(CursorMove::Jump(0, x));
    }

    /// Appends an empty row and moves to its name.
    pub fn add_row(&mut self) {
        self.rows.push(Row::new("", ""));
        self.row = self.rows.len() - 1;
        self.col = Col::Name;
    }

    fn remove_row(&mut self) {
        self.rows.remove(self.row);
        if self.rows.is_empty() {
            self.rows.push(Row::new("", ""));
        }
        self.row = self.row.min(self.rows.len() - 1);
        self.col = Col::Name;
    }

    fn enter_cell(&mut self, row: usize, col: Col, at_end: bool) {
        self.row = row;
        self.col = col;
        self.cell_mut().move_cursor(if at_end {
            CursorMove::End
        } else {
            CursorMove::Head
        });
    }

    pub fn key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match (key.code, ctrl) {
            (KeyCode::Char('d'), true) => self.remove_row(),
            (KeyCode::Enter, _) => match self.col {
                Col::Name => self.enter_cell(self.row, Col::Value, true),
                Col::Value if self.row + 1 < self.rows.len() => {
                    self.enter_cell(self.row + 1, Col::Name, true)
                }
                // the last row: append, unless it is still empty
                Col::Value if !self.rows[self.row].is_blank() => self.add_row(),
                Col::Value => {}
            },
            (KeyCode::Up, _) if self.row > 0 => self.enter_cell(self.row - 1, self.col, true),
            (KeyCode::Down, _) if self.row + 1 < self.rows.len() => {
                self.enter_cell(self.row + 1, self.col, true)
            }
            (KeyCode::Up | KeyCode::Down, _) => {}
            (KeyCode::Right, false)
                if self.col == Col::Name && cursor_col(self.cell()) >= len(self.cell()) =>
            {
                self.enter_cell(self.row, Col::Value, false)
            }
            (KeyCode::Left, false) if self.col == Col::Value && cursor_col(self.cell()) == 0 => {
                self.enter_cell(self.row, Col::Name, true)
            }
            _ => {
                self.cell_mut().input(Input::from(key));
            }
        }
    }
}

/// Whether the name of a row is shown as invalid.
pub fn name_is_invalid(row: &Row) -> bool {
    row.header().is_err()
}

pub fn to_lines(headers: &[Header]) -> Vec<String> {
    headers
        .iter()
        .map(|h| format!("{}: {}", h.name, h.value))
        .collect()
}

/// `Ok(None)` for a blank line, `Err` when the line is not `Name: value`.
pub fn parse_line(line: &str) -> Result<Option<Header>, String> {
    if line.trim().is_empty() {
        return Ok(None);
    }
    let (name, value) = line
        .split_once(':')
        .ok_or_else(|| "expected `Name: value`".to_owned())?;
    let name = name.trim();
    if name.is_empty() {
        return Err("the header name is empty".into());
    }
    if let Some(bad) = name.chars().find(|c| !is_token_char(*c)) {
        return Err(format!("invalid character `{bad}` in the header name"));
    }
    Ok(Some(Header::new(name, value.trim())))
}

fn is_token_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || "!#$%&'*+-.^_`|~".contains(c)
}

/// All headers, or the first invalid line with its 1-based number.
pub fn parse(lines: &[String]) -> Result<Vec<Header>, String> {
    let mut headers = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        match parse_line(line) {
            Ok(Some(h)) => headers.push(h),
            Ok(None) => {}
            Err(reason) => return Err(format!("header line {}: {reason}", i + 1)),
        }
    }
    Ok(headers)
}

/// Valid headers only; invalid lines are skipped (they are shown in red while editing).
pub fn parse_lenient(lines: &[String]) -> Vec<Header> {
    lines
        .iter()
        .filter_map(|l| parse_line(l).ok().flatten())
        .collect()
}

pub fn highlight_line(line: &str) -> Line<'static> {
    match parse_line(line) {
        Ok(Some(_)) => {
            let (name, value) = line.split_once(':').expect("valid header has a colon");
            Line::from(vec![
                Span::styled(name.to_owned(), Style::new().fg(Color::Cyan)),
                Span::styled(":", Style::new().fg(Color::DarkGray)),
                Span::raw(value.to_owned()),
            ])
        }
        Ok(None) => Line::raw(line.to_owned()),
        Err(_) => Line::styled(line.to_owned(), Style::new().fg(Color::Red)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_name_and_trimmed_value() {
        assert_eq!(
            parse_line("Content-Type:  application/json ").unwrap(),
            Some(Header::new("Content-Type", "application/json"))
        );
        // only the first colon splits, so values may contain colons
        assert_eq!(
            parse_line("Authorization: Bearer a:b").unwrap(),
            Some(Header::new("Authorization", "Bearer a:b"))
        );
        assert_eq!(
            parse_line("X-Empty:").unwrap(),
            Some(Header::new("X-Empty", ""))
        );
        assert_eq!(parse_line("   ").unwrap(), None);
    }

    #[test]
    fn rejects_malformed_lines() {
        assert!(parse_line("no colon here").is_err());
        assert!(parse_line(": value").is_err());
        assert!(parse_line("Bad Name: v").is_err());
    }

    #[test]
    fn parse_reports_the_first_bad_line_number() {
        let lines = [
            "A: 1".to_owned(),
            "".to_owned(),
            "oops".to_owned(),
            "B: 2".to_owned(),
        ];
        let err = parse(&lines).unwrap_err();
        assert!(err.contains("line 3"), "{err}");
        assert_eq!(
            parse_lenient(&lines),
            vec![Header::new("A", "1"), Header::new("B", "2")]
        );
    }

    #[test]
    fn round_trips_and_keeps_order_and_duplicates() {
        let headers = vec![
            Header::new("Set-Cookie", "a=1"),
            Header::new("X-A", "b"),
            Header::new("Set-Cookie", "b=2"),
        ];
        assert_eq!(parse(&to_lines(&headers)).unwrap(), headers);
    }

    #[test]
    fn invalid_lines_render_red_valid_ones_styled() {
        assert_eq!(highlight_line("oops").style.fg, Some(Color::Red));
        let ok = highlight_line("Accept: */*");
        assert_eq!(ok.spans[0].style.fg, Some(Color::Cyan));
        let text: String = ok.spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text, "Accept: */*");
    }
}
